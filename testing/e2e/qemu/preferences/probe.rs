#![no_main]
extern crate alloc;
use bexos_userspace::preferences::{self as rpc, wire as fidl};
use bexos_userspace::{Channel, Startup, log, yield_now};
use fidl::FidlDecode;
use probe_config::{ComponentConfig, LiveComponentConfig};
bexos_libc::entry!(run);
fn run(channel: u64) -> ! {
    let control = Channel(channel);
    let startup = Startup::receive(control).unwrap();
    bexos_libc::install_startup(&startup);
    if startup.arg0 == 2 {
        let config = ComponentConfig::from_startup(&startup).unwrap();
        log(&alloc::format!(
            "prefs-probe: legacy dark={} cache={}\n",
            config.dark,
            config.cache
        ));
        Startup::ready(control).unwrap();
        loop {
            yield_now();
        }
    }
    let mut live = LiveComponentConfig::from_startup(&startup).unwrap();
    let preferences = Channel(
        startup
            .service_grants
            .iter()
            .find(|g| g.protocol == "UserPreferences")
            .unwrap()
            .endpoint,
    );
    let foreign = rpc::call(
        preferences,
        1,
        &fidl::UserPreferencesGetPreferencesRequest {
            package_id: "bexos.service.timed",
        },
    )
    .unwrap();
    let handles = rpc::refs(&foreign);
    let response =
        fidl::UserPreferencesGetPreferencesResponse::decode(&foreign.bytes, &handles).unwrap();
    assert_eq!(response.status, fidl::Status::AccessDenied);
    assert!(response.config.is_empty());
    let mut verify_client = true;
    Startup::ready(control).unwrap();
    log(&alloc::format!(
        "prefs-probe: ready generation={} dark={} cache={}\n",
        live.generation(),
        live.snapshot().dark,
        live.snapshot().cache
    ));
    loop {
        if startup.arg0 != 1 {
            if let Err(error) = live.poll(
                |config| config.cache != 999,
                |config| {
                    verify_client = true;
                    log(&alloc::format!(
                        "prefs-probe: committed dark={} cache={}\n",
                        config.dark,
                        config.cache
                    ))
                },
            ) {
                log(&alloc::format!("prefs-probe: receiver {error:?}\n"));
                bexos_userspace::exit();
            }
        }
        if verify_client {
            let message = rpc::call(
                preferences,
                1,
                &fidl::UserPreferencesGetPreferencesRequest {
                    package_id: "com.example.preferences_probe",
                },
            )
            .unwrap();
            let handles = rpc::refs(&message);
            let response =
                fidl::UserPreferencesGetPreferencesResponse::decode(&message.bytes, &handles)
                    .unwrap();
            if response.status == fidl::Status::Ok {
                assert_eq!(
                    bexos_userspace::Memory::object_info(response.config[0].raw)
                        .unwrap()
                        .1
                        & 4,
                    0
                );
                let bytes = rpc::read_vmo(response.config[0].raw, response.config_len).unwrap();
                let table = bexos_userspace::config::ConfigTable::parse(&bytes).unwrap();
                assert_eq!(table.get_bool("dark").unwrap(), live.snapshot().dark);
                log(&alloc::format!(
                    "prefs-probe: retained client generation={}\n",
                    response.generation
                ));
                verify_client = false;
            } else {
                assert_eq!(response.status, fidl::Status::Busy);
            }
        }
        yield_now();
    }
}
