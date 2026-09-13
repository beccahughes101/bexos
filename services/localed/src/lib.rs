pub mod binding;
pub mod migration;
pub mod service;
pub mod storage;
pub mod wire;
use bexos_userspace::{
    Channel, Startup,
    live_migration::{RecordChanges, Source},
};
pub async fn main(channel: u64) -> ! {
    let control = Channel(channel);
    let startup = Startup::receive(control).expect("localed startup");
    let mut runtime = if startup.migration_target {
        bexos_userspace::live_migration::receive::<service::Runtime>(
            control,
            startup.migration_generation,
        )
        .expect("localed transplant")
    } else {
        let mut runtime = service::Runtime::default();
        runtime.control = control;
        runtime.migration = startup.migration;
        runtime.preferences = Channel(
            startup
                .service_grants
                .iter()
                .find(|g| g.protocol == "LocalePreferences")
                .expect("localed preferences grant")
                .endpoint,
        );
        let package = startup
            .namespace
            .iter()
            .find(|n| n.path == "/pkg")
            .expect("localed package namespace");
        (runtime.data, runtime.data_len) =
            storage::load(Channel(package.directory)).expect("localed CLDR data");
        runtime.data_generation = 1;
        Startup::ready(control).expect("localed ready");
        bexos_userspace::log("localed: ready\n");
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
        wire::poll_preferences(&mut runtime);
        if let Ok(message) = runtime.control.try_recv() {
            binding::accept(&mut runtime, message);
        }
        wire::poll(&mut runtime);
        wire::broadcast(&mut runtime);
        bexos_userspace::yield_now();
    }
}
