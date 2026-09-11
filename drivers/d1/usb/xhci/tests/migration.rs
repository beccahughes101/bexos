use bexos_d1_usb_xhcid::Runtime;
use bexos_userspace::{Channel, live_migration::State};

#[test]
fn controller_channels_buffers_and_markers_survive_replacement() {
    let mut old = Runtime::new(Channel(1), Some(Channel(2)), None);
    old.bus = Some(Channel(3));
    old.bus_events = Some(Channel(4));
    old.completions.push(Channel(5));
    old.queued_transfers.push(77);
    old.hardware = Some(bexos_d1_usb_xhcid::Hardware::adopt_markers(
        10,
        0x1000,
        0x1000,
        11,
        12,
        bexos_usb_host::xhci::CapabilityRegisters {
            caplength: 0x40,
            max_slots: 8,
            max_interrupters: 1,
            max_ports: 4,
            scratchpads: 1,
            doorbell_offset: 0x1000,
            runtime_offset: 0x2000,
        },
        9,
        [0xa000, 0xb000, 0xc000],
    ));
    let mut new = Runtime::empty();
    for key in old.keys() {
        new.adopt_record(key, old.encode_record(key).unwrap().as_deref())
            .unwrap();
    }
    new.finish_adoption().unwrap();
    assert_eq!(new.bus.unwrap().0, 3);
    assert_eq!(new.bus_events.unwrap().0, 4);
    assert_eq!(new.completions[0].0, 5);
    assert_eq!(new.activation_markers(), [0xa000, 0xb000, 0xc000]);
    assert_eq!(new.queued_transfers, [77]);
}
