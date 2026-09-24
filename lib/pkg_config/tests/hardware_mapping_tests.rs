use bexos_pkg_client::{ArtifactKind, ArtifactQuery};
use bexos_pkg_config::{Config, HardwareIdentity, HardwareSelector, Mapping};

fn mapping(name: &str, hardware: HardwareSelector, priority: u32) -> Mapping {
    Mapping {
        name: name.into(),
        query: ArtifactQuery {
            registry_host: "registry.example".into(),
            repository: "drivers".into(),
            tag: name.into(),
            kind: ArtifactKind::Driver,
            expected_digest: None,
        },
        hardware: Some(hardware),
        priority,
    }
}

fn identity() -> HardwareIdentity {
    HardwareIdentity {
        pci_segment: 0,
        pci_bus: 2,
        pci_device: 3,
        pci_function: 1,
        pci_vendor_id: 0x8086,
        pci_device_id: 0x1539,
        pci_class: 2,
        pci_subclass: 0,
        pci_prog_if: 0,
    }
}

#[test]
fn exact_identity_beats_priority_at_less_specific_tiers() {
    let config = Config {
        mappings: vec![
            mapping(
                "class",
                HardwareSelector {
                    pci_class: Some(2),
                    ..Default::default()
                },
                999,
            ),
            mapping(
                "device",
                HardwareSelector {
                    pci_vendor_id: Some(0x8086),
                    pci_device_id: Some(0x1539),
                    ..Default::default()
                },
                999,
            ),
            mapping(
                "exact",
                HardwareSelector {
                    pci_segment: Some(0),
                    pci_bus: Some(2),
                    pci_device: Some(3),
                    pci_function: Some(1),
                    ..Default::default()
                },
                0,
            ),
        ],
        ..Default::default()
    };
    assert_eq!(config.driver_mapping(identity()).unwrap().name, "exact");
}

#[test]
fn priority_and_name_make_same_tier_resolution_deterministic() {
    let selector = HardwareSelector {
        pci_vendor_id: Some(0x8086),
        pci_device_id: Some(0x1539),
        ..Default::default()
    };
    let config = Config {
        mappings: vec![
            mapping("z-lower", selector.clone(), 4),
            mapping("z-tie", selector.clone(), 9),
            mapping("a-tie", selector, 9),
        ],
        ..Default::default()
    };
    assert_eq!(config.driver_mapping(identity()).unwrap().name, "a-tie");
}

#[test]
fn nonmatching_identity_is_not_resolved() {
    let config = Config {
        mappings: vec![mapping(
            "other",
            HardwareSelector {
                pci_vendor_id: Some(0x1234),
                pci_device_id: Some(0x5678),
                ..Default::default()
            },
            1,
        )],
        ..Default::default()
    };
    assert!(config.driver_mapping(identity()).is_none());
}
