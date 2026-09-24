extern crate alloc;
#[path = "../src/runtime/state.rs"]
mod state;
use bexos_userspace::{Channel, live_migration::State, service_control::ControlState};
use state::{Pending, Runtime};
#[test]
fn pci_hotplug_state_retains_allocator_endpoints_and_outstanding_registration() {
    let source = Runtime {
        control: ControlState {
            manager: Channel(1),
            migration: Some(Channel(2)),
            mapping: Some((3, 4096, 0x10_0000)),
            requests: 9,
        },
        registry: Channel(4),
        cursor: bexos_d1_pci::RootBusConfig::qemu_virt().mmio_base + 0x4000,
        root: bexos_d1_pci::RootBusConfig::qemu_virt(),
        next_poll_ms: 123,
        known: vec![0x400],
        ports: vec![],
        pending: Some(Pending {
            node: 0x500,
            added: true,
            control: Some(Channel(6)),
            interrupt: Some(7),
        }),
        power: vec![Channel(5)],
        device_controls: vec![],
        extended: true,
    };
    assert!(!source.quiescence_ready());
    let mut target = Runtime::empty();
    for key in source.keys() {
        let bytes = source.encode_record(key).unwrap().unwrap();
        target.adopt_record(key, Some(&bytes)).unwrap();
        assert_eq!(target.encode_record(key).unwrap().unwrap(), bytes);
    }
    target.validate().unwrap();
    assert_eq!(
        format!("{:?}", target.resources()),
        format!("{:?}", source.resources())
    );
    assert_eq!(target.cursor, source.cursor);
    assert!(!target.quiescence_ready());
    let bytes = source.encode_record(1).unwrap().unwrap();
    for length in 0..bytes.len() {
        assert!(target.adopt_record(1, Some(&bytes[..length])).is_err());
    }
    assert_eq!(target.encode_record(1).unwrap().unwrap(), bytes);
}
#[test]
fn legacy_control_record_remains_identical_until_activation() {
    let legacy = ControlState {
        manager: Channel(1),
        migration: Some(Channel(2)),
        mapping: None,
        requests: 44,
    };
    let bytes = legacy.encode_record(0).unwrap().unwrap();
    let mut target = Runtime::empty();
    target.adopt_record(0, Some(&bytes)).unwrap();
    target.validate().unwrap();
    assert_eq!(target.keys(), vec![0]);
    assert_eq!(target.encode_record(0).unwrap().unwrap(), bytes);
}

#[test]
fn downstream_windows_and_allocator_survive_replacement_and_reject_aliases() {
    use bexos_d1_pci::{ECAM_SIZE, RootBusConfig, RootPort, address_from_node};
    let mut source = Runtime::empty();
    source.control = ControlState {
        manager: Channel(1),
        migration: Some(Channel(2)),
        mapping: Some((3, 4096, ECAM_SIZE)),
        requests: 0,
    };
    source.registry = Channel(4);
    let base = RootBusConfig::qemu_virt().mmio_base;
    source.cursor = base + 0x100_0000;
    source.ports.push(RootPort {
        address: address_from_node(0x400).unwrap(),
        secondary: 1,
        base,
        limit: source.cursor,
        cursor: base + 0x4000,
    });
    source.known.push(0x10000);
    source.extended = true;
    source.validate().unwrap();
    let mut target = Runtime::empty();
    for key in source.keys() {
        let bytes = source.encode_record(key).unwrap().unwrap();
        target.adopt_record(key, Some(&bytes)).unwrap();
        assert_eq!(target.encode_record(key).unwrap().unwrap(), bytes);
    }
    target.validate().unwrap();
    assert_eq!(target.ports, source.ports);
    let before = target.encode_record(1).unwrap();
    source.ports.push(source.ports[0]);
    assert!(
        target
            .adopt_record(1, source.encode_record(1).unwrap().as_deref())
            .is_err()
    );
    assert_eq!(target.encode_record(1).unwrap(), before);
    target.control.mapping.as_mut().unwrap().2 = 0x10_0000;
    assert!(target.validate().is_err());
}
