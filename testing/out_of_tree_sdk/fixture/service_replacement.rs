#![no_main]

mod state;

use bexos_component::{Channel, Startup};

fn main(startup_channel: u64) -> ! {
    let control = Channel(startup_channel);
    let startup = Startup::receive(control).unwrap_or_else(|_| bexos_component::exit());
    if !startup.migration_target {
        bexos_component::exit();
    }
    let state = bexos_component::live_migration::receive::<state::FixtureState>(
        control,
        startup.migration_generation,
    )
    .unwrap_or_else(|_| bexos_component::exit());
    bexos_component::log("sdk-fixture-service: replacement active with continuity\n");
    state::serve(state)
}

bexos_component::std_entry!(main);
