#![no_std]
#![no_main]

fn main(startup_channel: u64) -> ! {
    let control = bexos_userspace::Channel(startup_channel);
    let _startup = bexos_userspace::Startup::receive(control).unwrap();
    bexos_userspace::Startup::ready(control).unwrap();
    bexos_userspace::log("sdk-fixture-native: out-of-tree service started\n");
    loop {
        bexos_userspace::yield_now();
    }
}

bexos_userspace::entry!(main);
