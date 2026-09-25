#![no_main]

mod state;
use bexos_component::{Channel, Startup};

fn main(startup_channel: u64) -> ! {
    let control = Channel(startup_channel);
    let startup = Startup::receive(control).unwrap_or_else(|_| bexos_component::exit());
    let state = if startup.migration_target {
        bexos_component::live_migration::receive::<state::ExampleState>(control, startup.migration_generation)
            .unwrap_or_else(|_| bexos_component::exit())
    } else {
        let handle = startup.driver_resources.first().map(|resource| resource.handle).unwrap_or_else(|| bexos_component::exit());
        Startup::ready(control).unwrap_or_else(|_| bexos_component::exit());
        state::ExampleState::new(control, startup.migration, Some(handle))
    };
    state::serve(state)
}

bexos_component::std_entry!(main);
