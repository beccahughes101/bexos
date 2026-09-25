#![no_main]

mod state;

unsafe extern "C" { fn example_c_increment(value: u64) -> u64; }

fn main(startup_channel: u64) -> ! {
    use bexos_component::{Channel, Startup};
    let control = Channel(startup_channel);
    let startup = Startup::receive(control).unwrap_or_else(|_| bexos_component::exit());
    let state = if startup.migration_target {
        bexos_component::live_migration::receive::<state::ExampleState>(control, startup.migration_generation)
            .unwrap_or_else(|_| bexos_component::exit())
    } else {
        let mut message = String::from("example-native-service: structured startup accepted value=");
        message.push_str(if unsafe { example_c_increment(40) } == 42 { "42\n" } else { "error\n" });
        bexos_component::log(&message);
        Startup::ready(control).unwrap_or_else(|_| bexos_component::exit());
        state::ExampleState::new(control, startup.migration, startup.resources.first().copied())
    };
    state::serve(state)
}

bexos_component::std_entry!(main);
