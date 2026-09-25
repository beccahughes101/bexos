#![no_main]

mod state;

use bexos_component::{Channel, Startup};
use state::FixtureState;

unsafe extern "C" {
    fn sdk_c_increment(value: u64) -> u64;
}

fn main(startup_channel: u64) -> ! {
    let control = Channel(startup_channel);
    let startup = Startup::receive(control).unwrap_or_else(|_| bexos_component::exit());
    let state = if startup.migration_target {
        bexos_component::live_migration::receive::<FixtureState>(
            control,
            startup.migration_generation,
        )
        .unwrap_or_else(|_| bexos_component::exit())
    } else {
        let value = unsafe { sdk_c_increment(40) };
        let mut message =
            String::from("sdk-fixture-service: BootFS std/libc+C service started value=");
        message.push_str(if value == 42 { "42\n" } else { "unexpected\n" });
        bexos_component::log(&message);
        Startup::ready(control).unwrap_or_else(|_| bexos_component::exit());
        FixtureState::new(
            control,
            startup.migration,
            startup.resources.first().copied(),
        )
    };
    state::serve(state)
}

bexos_component::std_entry!(main);
