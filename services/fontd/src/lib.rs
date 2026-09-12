#![no_std]

extern crate alloc;

pub mod binding;
pub mod index;
pub mod migration;
pub mod parser;
pub mod resolver;
pub mod runtime;
pub mod storage;
pub mod system;
pub mod wire;

use bexos_userspace::service_binding::ServiceBinding;
use bexos_userspace::{Channel, Memory, Startup, live_migration::Source, log};
use runtime::Runtime;

pub async fn main(channel: u64) -> ! {
    let control = Channel(channel);
    log("fontd: entering startup\n");
    let startup = Startup::receive(control).expect("fontd startup");
    log("fontd: startup received\n");
    let runtime = if startup.migration_target {
        bexos_userspace::live_migration::receive::<Runtime>(control, startup.migration_generation)
            .expect("fontd transplant")
    } else {
        let mut runtime = Runtime::empty();
        runtime.control = control;
        runtime.migration = startup.migration;
        for grant in startup.service_grants {
            match grant.protocol.as_str() {
                "VfsManager" => runtime.vfsd = Channel(grant.endpoint),
                "UserManager" => runtime.usersd = Channel(grant.endpoint),
                _ => {}
            }
        }
        runtime.add_system_fonts().expect("fontd system fonts");
        Startup::ready(control).expect("fontd ready");
        log("fontd: ready\n");
        runtime
    };
    serve(runtime).await
}

async fn serve(mut runtime: Runtime) -> ! {
    let mut source = Source::new(runtime.migration);
    loop {
        if source.poll(&runtime).is_err() {
            let _ = bexos_userspace::migration::abort();
        }
        if source.quiescing() {
            bexos_userspace::yield_now();
            continue;
        }
        if let Ok(message) = runtime.control.try_recv() {
            if let (Some(endpoint), Ok(metadata)) = (
                message.handles.first().copied(),
                core::str::from_utf8(&message.bytes),
            ) {
                if let Some(binding) = ServiceBinding::parse(metadata) {
                    if let Some(client) = binding::Client::from_binding(endpoint, &binding) {
                        runtime.clients.push(client);
                        source.changed(0);
                    } else {
                        let _ = Memory::close(endpoint);
                    }
                } else {
                    let _ = Memory::close(endpoint);
                }
            }
        }
        if wire::poll(&mut runtime) {
            source.changed_keys([0, 1, 2]);
        }
        if wire::poll_users(&mut runtime) {
            source.changed_keys([0, 1, 2]);
        }
        bexos_userspace::yield_now();
    }
}
