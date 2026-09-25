use bexos_wasm_guest::exports::bexos::wasm::lifecycle::Guest;

struct Fixture;

impl Guest for Fixture {
    fn dispatch(_resource_id: u32) {}

    fn checkpoint() -> Vec<u8> {
        Vec::new()
    }

    fn restore(_checkpoint: Vec<u8>) -> Result<(), ()> {
        Ok(())
    }

    fn activate() {
        println!("sdk-fixture-wasm: out-of-tree service started");
    }

    fn abort() {}
}

fn main() {}

bexos_wasm_guest::export!(Fixture with_types_in bexos_wasm_guest);
