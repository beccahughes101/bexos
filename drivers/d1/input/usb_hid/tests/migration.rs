use bexos_usb_hid::{HidKind, Runtime};
use bexos_userspace::{Channel, live_migration::State};

#[test]
fn input_stream_state_survives_replacement() {
    let mut runtime = Runtime::new(Channel(1), Some(Channel(2)));
    runtime.interface = Some(Channel(3));
    runtime.sink = Some(Channel(4));
    runtime.node = 44;
    runtime.kind = HidKind::Mouse;
    runtime.sequence = 9;
    runtime.reports = 10;
    runtime.held_mouse.buttons = 3;
    let mut new = Runtime::empty();
    for key in runtime.keys() {
        new.adopt_record(key, runtime.encode_record(key).unwrap().as_deref())
            .unwrap();
    }
    new.finish_adoption().unwrap();
    assert_eq!(new.interface.unwrap().0, 3);
    assert_eq!(new.sink.unwrap().0, 4);
    assert_eq!(new.kind, HidKind::Mouse);
    assert_eq!(new.sequence, 9);
    assert_eq!(new.held_mouse.buttons, 3);
}
