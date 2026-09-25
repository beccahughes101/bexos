pub mod binding;
pub mod launcher;
pub mod migration;
pub mod runtime;
pub mod spec;
pub mod storage;
pub mod wire;

use bexos_userspace::{
    Channel, Memory, Startup,
    live_migration::{RecordChanges, Source},
};

fn grant(startup: &Startup, service_or_protocol: &str) -> Channel {
    Channel(
        startup
            .service_grants
            .iter()
            .find(|grant| {
                grant.service == service_or_protocol || grant.protocol == service_or_protocol
            })
            .unwrap_or_else(|| panic!("containerd missing {service_or_protocol} grant"))
            .endpoint,
    )
}

pub async fn main(channel: u64) -> ! {
    let control = Channel(channel);
    let startup = Startup::receive(control).expect("containerd startup");
    let mut runtime = if startup.migration_target {
        bexos_userspace::live_migration::receive::<runtime::Runtime>(
            control,
            startup.migration_generation,
        )
        .expect("containerd transplant")
    } else {
        let data = startup
            .namespace
            .iter()
            .find(|entry| entry.path == "/data")
            .expect("containerd /data")
            .directory;
        let opener = grant(&startup, "Opener");
        let resolver = grant(&startup, "PackageResolver").0;
        let containers = storage::load(data)
            .expect("containerd durable state")
            .into_iter()
            .map(|(id, spec)| (id, runtime::Container::cold(spec)))
            .collect();
        // Appd cannot service its Opener protocol while it is synchronously
        // waiting for this service's ready handshake.  Signal readiness first;
        // appd then registers the routed endpoint and returns to its broker
        // loop, where the command-launcher bind can complete.
        Startup::ready(control).expect("containerd ready");
        bexos_userspace::log("containerd: startup ready; binding command launcher\n");
        let launcher = launcher::bind(opener).expect("containerd command launcher");
        let _ = Memory::close(opener.0);
        let runtime = runtime::Runtime {
            control,
            migration: startup.migration,
            data,
            resolver,
            launcher: launcher.0,
            clients: Vec::new(),
            containers,
            pending: None,
            poll_tick: 0,
        };
        bexos_userspace::log("containerd: ready (offline OCI runtime)\n");
        runtime
    };
    let mut source = Source::new(runtime.migration);
    let mut changes = RecordChanges::default();
    loop {
        changes.poll(&runtime, &mut source);
        if source.poll(&runtime).is_err() {
            let _ = bexos_userspace::migration::abort();
        }
        if source.quiescing() {
            bexos_userspace::yield_now();
            continue;
        }
        wire::poll_pending(&mut runtime);
        wire::poll_processes(&mut runtime);
        if let Ok(message) = runtime.control.try_recv() {
            binding::accept(&mut runtime, message);
        }
        wire::poll_clients(&mut runtime);
        bexos_userspace::yield_now();
    }
}
