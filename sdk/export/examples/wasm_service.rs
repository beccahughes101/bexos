use bexos_wasm_guest::exports::bexos::wasm::lifecycle::Guest;

struct ExampleService;

impl Guest for ExampleService {
    fn dispatch(_resource_id: u32) {}

    fn checkpoint() -> Vec<u8> {
        Vec::new()
    }

    fn restore(_checkpoint: Vec<u8>) -> Result<(), ()> {
        Ok(())
    }

    fn activate() {
        println!("example-wasm-service: started");
    }

    fn abort() {}
}

fn main() {}

bexos_wasm_guest::export!(ExampleService with_types_in bexos_wasm_guest);
