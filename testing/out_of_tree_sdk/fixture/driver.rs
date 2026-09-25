#![no_main]

mod state;

use bexos_component::{Channel, Startup};

fn main(startup_channel: u64) -> ! {
    let control = Channel(startup_channel);
    let startup = Startup::receive(control).unwrap_or_else(|_| bexos_component::exit());
    let state = if startup.migration_target {
        bexos_component::live_migration::receive::<state::FixtureState>(
            control,
            startup.migration_generation,
        )
        .unwrap_or_else(|_| bexos_component::exit())
    } else {
        if startup.driver_resources.len() < 3 {
            bexos_component::exit();
        }
        bexos_component::log("sdk-fixture-driver: e1000e bound with structured resources\n");
        Startup::ready(control).unwrap_or_else(|_| bexos_component::exit());
        state::FixtureState::new(
            control,
            startup.migration,
            startup.driver_resources.first().map(|r| r.handle),
        )
    };
    state::serve(state)
}

bexos_component::std_entry!(main);
