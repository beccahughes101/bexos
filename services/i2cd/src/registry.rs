use bexos_i2c_spi::{
    BusKind,
    topology::{ControllerConfig, DeviceProperty, Peripheral},
};
use bexos_userspace::Channel;
use hardware_manager_fidl::{
    DeviceNodeInfo, DeviceProperty as WireProperty, DeviceRegistryRegisterDeviceNodeRequest,
    DeviceRegistryRegisterDeviceNodeResponse, FidlDecode, FidlEncode, HandleRef, HardwareResource,
    HardwareResourceKind, WireVector,
};

pub fn register_controller(
    registry: Channel,
    controller: &ControllerConfig,
) -> hardware_manager_fidl::Status {
    let properties = properties(&controller.properties);
    let request = DeviceRegistryRegisterDeviceNodeRequest {
        info: DeviceNodeInfo {
            node_id: controller.node_id,
            bus: hardware_bus(controller.bus),
            has_parent: false,
            parent_node_id: 0,
            properties: WireVector::from_slice(&properties),
        },
        resources: WireVector::from_slice(&[]),
    };
    send_register(registry, &request, &[])
}

pub fn register_peripheral(
    registry: Channel,
    controller_node_id: u64,
    peripheral: &Peripheral,
    endpoint: u64,
) -> hardware_manager_fidl::Status {
    let properties = properties(&peripheral.properties);
    let resources = [HardwareResource {
        kind: HardwareResourceKind::BusControl,
        resource_id: peripheral.node_id,
        base: peripheral.node_id,
        length: 1,
        flags: controller_node_id,
        resource: HandleRef { raw: endpoint },
    }];
    let request = DeviceRegistryRegisterDeviceNodeRequest {
        info: DeviceNodeInfo {
            node_id: peripheral.node_id,
            bus: hardware_manager_fidl::BusType::I2c,
            has_parent: true,
            parent_node_id: controller_node_id,
            properties: WireVector::from_slice(&properties),
        },
        resources: WireVector::from_slice(&resources),
    };
    send_register(registry, &request, &[endpoint])
}

fn send_register(
    registry: Channel,
    request: &DeviceRegistryRegisterDeviceNodeRequest<'_>,
    raw_handles: &[u64],
) -> hardware_manager_fidl::Status {
    let mut bytes = [0; 2048];
    let mut handles = [HandleRef { raw: 0 }; 1];
    bytes[..8].copy_from_slice(&1u64.to_le_bytes());
    let Ok(encoded) = request.encode(&mut bytes[8..], &mut handles) else {
        return hardware_manager_fidl::Status::ErrInvalidArgs;
    };
    if registry
        .send(&bytes[..8 + encoded.bytes], raw_handles)
        .is_err()
    {
        return hardware_manager_fidl::Status::ErrPeerClosed;
    }
    match registry.try_recv() {
        Ok(reply) => {
            let refs: alloc::vec::Vec<_> = reply
                .handles
                .iter()
                .map(|raw| HandleRef { raw: *raw })
                .collect();
            DeviceRegistryRegisterDeviceNodeResponse::decode(&reply.bytes, &refs)
                .map(|reply| reply.status)
                .unwrap_or(hardware_manager_fidl::Status::ErrInvalidArgs)
        }
        Err(_) => hardware_manager_fidl::Status::ErrTimedOut,
    }
}

fn hardware_bus(bus: BusKind) -> hardware_manager_fidl::BusType {
    match bus {
        BusKind::I2c => hardware_manager_fidl::BusType::I2c,
        BusKind::Spi => hardware_manager_fidl::BusType::Spi,
    }
}

fn properties(properties: &[DeviceProperty]) -> alloc::vec::Vec<WireProperty<'_>> {
    properties
        .iter()
        .map(|property| WireProperty {
            key: property.key.as_str(),
            value: property.value,
        })
        .collect()
}
