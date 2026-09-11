fn main() {}
use bexos_dioxus_guest::exports::bexos::wasm::lifecycle::Guest;
struct Rejected;
impl Guest for Rejected {
    fn dispatch(_: u32) {}
    fn checkpoint() -> Vec<u8> {
        vec![]
    }
    fn restore(_: Vec<u8>) -> Result<(), ()> {
        Err(())
    }
    fn activate() {}
    fn abort() {}
}
bexos_dioxus_guest::export!(Rejected with_types_in bexos_dioxus_guest);
