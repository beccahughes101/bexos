use bexos_dioxus_guest::{View, exports::bexos::wasm::lifecycle::Guest, scene::SceneBatch};
use std::cell::RefCell;
struct Probe;
#[derive(Default)]
struct State {
    view: Option<View>,
    inputs: u32,
    announced: bool,
    painted_inputs: Option<u32>,
}
thread_local! { static STATE:RefCell<State>=RefCell::new(State::default()); }
fn main() {}
impl Guest for Probe {
    fn dispatch(_: u32) {
        STATE.with_borrow_mut(|s| {
            if s.view.is_none() {
                s.view = View::open(640, 420).ok();
            }
            let Some(v) = &s.view else { return };
            if let Ok(events) = v.input() {
                for e in events {
                    if e.kind == 1 || e.kind == 2 {
                        s.inputs = s.inputs.saturating_add(1);
                        eprintln!("sysui-probe: input received");
                    }
                }
            }
            if s.painted_inputs == Some(s.inputs) {
                return;
            }
            let frame = SceneBatch {
                width: 640,
                height: 420,
                clear_rgba: [100, (s.inputs % 128) as u8, 60, 255],
                commands: vec![],
            };
            if v.submit(&frame).is_ok() {
                s.painted_inputs = Some(s.inputs);
                if !s.announced {
                    s.announced = true;
                    eprintln!("sysui-probe: first frame submitted");
                }
            }
        });
    }
    fn checkpoint() -> Vec<u8> {
        STATE.with_borrow(|s| {
            let mut b = s.inputs.to_le_bytes().to_vec();
            b.extend(s.view.as_ref().map_or(0, |v| v.id()).to_le_bytes());
            b
        })
    }
    fn restore(b: Vec<u8>) -> Result<(), ()> {
        if b.len() != 8 {
            return Err(());
        }
        STATE.with_borrow_mut(|s| {
            s.painted_inputs = None;
            s.inputs = u32::from_le_bytes(b[..4].try_into().unwrap());
            let id = u32::from_le_bytes(b[4..].try_into().unwrap());
            s.view = (id != 0).then(|| View::adopt(id));
        });
        Ok(())
    }
    fn activate() {}
    fn abort() {}
}
bexos_dioxus_guest::export!(Probe with_types_in bexos_dioxus_guest);
