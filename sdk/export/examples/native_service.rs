#![no_std]
#![no_main]

fn main(startup_channel: u64) -> ! {
    if bexos_app::service_ready(startup_channel).is_err() {
        bexos_app::log("example-native-service: readiness failed\n");
        bexos_app::exit();
    }
    bexos_app::log("example-native-service: started\n");
    loop {
        bexos_app::yield_now();
    }
}

bexos_app::entry!(main);
