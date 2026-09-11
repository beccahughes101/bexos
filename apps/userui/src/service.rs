use bexos_dioxus_guest::exports::bexos::wasm::lifecycle::Guest;
use std::cell::RefCell;
pub struct Service;
thread_local! {static STATE:RefCell<crate::desktop::Desktop>=RefCell::new(Default::default());}
impl Guest for Service {
    fn dispatch(_: u32) {
        STATE.with_borrow_mut(|s| {
            if let Err(e) = s.tick() {
                if s.error != e {
                    eprintln!("shell UI: {e}");
                }
                s.error = e;
                s.dirty = true;
            }
        });
    }
    fn checkpoint() -> Vec<u8> {
        STATE.with_borrow(|s| s.checkpoint())
    }
    fn restore(bytes: Vec<u8>) -> Result<(), ()> {
        let state = crate::desktop::Desktop::restore(&bytes).map_err(|_| ())?;
        STATE.with_borrow_mut(|s| *s = state);
        Ok(())
    }
    fn activate() {}
    fn abort() {}
}
