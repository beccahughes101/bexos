use bexos_appd::{
    HardwareAccessTier, HardwareResourceKind, HardwareResourceLease,
    hardware_resources::duplicate_startup_resources,
};
use bexos_kernel_core::{
    ipc::Capability,
    kernel_services::{RIGHT_DUPLICATE, RIGHT_MAP, RIGHT_READ, RIGHT_TRANSFER, RIGHT_WRITE},
};
use kernel_fidl::Status;

fn lease(kind: HardwareResourceKind, handle: u64, rights: u32) -> HardwareResourceLease {
    HardwareResourceLease {
        kind,
        resource_id: handle,
        base: 0,
        length: 4096,
        flags: 0,
        capability: Capability {
            object_id: handle,
            rights,
        },
    }
}

#[test]
fn iommu_startup_keeps_registry_capability_and_does_not_request_vmo_rights() {
    let domain_rights = RIGHT_TRANSFER | RIGHT_READ | RIGHT_WRITE | RIGHT_DUPLICATE;
    let resource = lease(HardwareResourceKind::IommuDomain, 147, domain_rights);
    for access in [HardwareAccessTier::Direct, HardwareAccessTier::Isolated] {
        let grants = duplicate_startup_resources(
            &[resource.clone()],
            access,
            |handle, rights| {
                assert_eq!(handle, 147);
                assert_eq!(rights & RIGHT_MAP, 0);
                if rights & !domain_rights != 0 {
                    return Err(Status::ErrAccessDenied);
                }
                Ok(200)
            },
            |_| panic!("successful grant was closed"),
        )
        .unwrap();
        assert_eq!(grants[0].handle, 200);
        assert_eq!(resource.capability.object_id, 147);
    }
}

#[test]
fn duplicate_failure_closes_partial_grants_without_transferring_registry_originals() {
    let resources = [
        lease(HardwareResourceKind::Mmio, 10, 63),
        lease(HardwareResourceKind::IommuDomain, 11, 39),
    ];
    let mut closed = Vec::new();
    let result = duplicate_startup_resources(
        &resources,
        HardwareAccessTier::Direct,
        |handle, _| {
            if handle == 10 {
                Ok(100)
            } else {
                Err(Status::ErrAccessDenied)
            }
        },
        |handle| closed.push(handle),
    );
    assert!(matches!(result, Err(Status::ErrAccessDenied)));
    assert_eq!(closed, [100]);
    assert_eq!(resources[0].capability.object_id, 10);
    assert_eq!(resources[1].capability.object_id, 11);
}

#[test]
fn driver_replacement_retargets_registry_process_without_changing_channels() {
    use bexos_appd::{DeviceNodeState, DeviceRegistry};
    let mut registry = DeviceRegistry::new();
    for id in [7, 8] {
        registry
            .register_device_node(super::nvme_device_node(id))
            .unwrap();
        registry
            .begin_binding(id, "driver".into(), "driver".into())
            .unwrap();
        registry
            .bind_active(
                id,
                Some(Capability {
                    object_id: id + 10,
                    rights: 128,
                }),
                Some(Capability {
                    object_id: id + 20,
                    rights: 3,
                }),
                Some(Capability {
                    object_id: id + 30,
                    rights: 3,
                }),
            )
            .unwrap();
    }
    registry.replace_process_handle(17, 99);
    for node in registry.nodes() {
        let DeviceNodeState::Active(binding) = &node.state else {
            panic!("lost active binding")
        };
        let id = node.info.node_id;
        assert_eq!(
            binding.process_handle.unwrap(),
            Capability {
                object_id: if id == 7 { 99 } else { 18 },
                rights: 128
            }
        );
        assert_eq!(binding.manager_channel.unwrap().object_id, id + 20);
        assert_eq!(binding.lifecycle_channel.unwrap().object_id, id + 30);
    }
}
