#![no_std]
#![no_main]
use bexos_d1_uart::{Pl011Mmio, Pl011Uart};
use bexos_userspace::service_control::{ControlState, serve};
use bexos_userspace::{Channel, Memory, Startup};
bexos_userspace::entry!(run);
fn run(channel: u64) -> ! {
    let channel = Channel(channel);
    let startup = Startup::receive(channel).unwrap();
    if startup.migration_target {
        match bexos_userspace::live_migration::receive::<ControlState>(
            channel,
            startup.migration_generation,
        ) {
            Ok(state) => serve(state),
            Err(_) => bexos_userspace::exit(),
        }
    }
    let h = Memory::physical(0x0900_0000, 4096).unwrap();
    let va = Memory::map(h, 4096, 6).unwrap();
    let mut uart = Pl011Uart::new(unsafe { Pl011Mmio::new(va as usize) });
    uart.init().unwrap();
    uart.write_str_poll("pl011: EL0 driver ready\n").unwrap();
    Startup::ready(channel).unwrap();
    serve(ControlState {
        manager: channel,
        migration: startup.migration,
        mapping: Some((h, va, 4096)),
        requests: 0,
    });
}
