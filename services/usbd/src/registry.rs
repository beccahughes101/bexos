use crate::topology::Interface;
use alloc::vec::Vec;
use bexos_usb_host::InterfaceClass;
use bexos_userspace::Channel;
use hardware_manager_fidl::{
    BusType, DeviceNodeInfo, DeviceProperty, DeviceRegistryRegisterDeviceNodeRequest,
    DeviceRegistryRegisterDeviceNodeResponse, DeviceRegistryUnregisterDeviceNodeRequest,
    DeviceRegistryUnregisterDeviceNodeResponse, FidlDecode, FidlEncode, HandleRef,
    HardwareResource, HardwareResourceKind, Status, WireVector,
};

pub fn register_interface(registry: Channel, interface: &Interface, channel: u64) -> Status {
    let properties = properties(interface);
    let resources = [HardwareResource {
        kind: HardwareResourceKind::BusControl,
        resource_id: interface.id,
        base: interface.device_id,
        length: 1,
        flags: interface.generation,
        resource: HandleRef { raw: channel },
    }];
    let request = DeviceRegistryRegisterDeviceNodeRequest {
        info: DeviceNodeInfo {
            node_id: interface.id,
            bus: BusType::Usb,
            has_parent: true,
            parent_node_id: interface.device_id,
            properties: WireVector::from_slice(&properties),
        },
        resources: WireVector::from_slice(&resources),
    };
    call_registry_register(registry, &request, &[channel])
}

pub fn unregister_interface(registry: Channel, node_id: u64) -> Status {
    let mut bytes = [0; 64];
    bytes[..8].copy_from_slice(&2u64.to_le_bytes());
    let encoded = match (DeviceRegistryUnregisterDeviceNodeRequest { node_id })
        .encode(&mut bytes[8..], &mut [])
    {
        Ok(encoded) => encoded,
        Err(_) => return Status::ErrInvalidArgs,
    };
    if registry.send(&bytes[..8 + encoded.bytes], &[]).is_err() {
        return Status::ErrPeerClosed;
    }
    match registry.try_recv() {
        Ok(reply) => DeviceRegistryUnregisterDeviceNodeResponse::decode(&reply.bytes, &[])
            .map(|response| response.status)
            .unwrap_or(Status::ErrInvalidArgs),
        Err(_) => Status::ErrTimedOut,
    }
}

fn call_registry_register(
    registry: Channel,
    request: &DeviceRegistryRegisterDeviceNodeRequest<'_>,
    handles: &[u64],
) -> Status {
    let mut bytes = [0; 2048];
    let mut encoded_handles = [HandleRef { raw: 0 }; 4];
    bytes[..8].copy_from_slice(&1u64.to_le_bytes());
    let encoded = match request.encode(&mut bytes[8..], &mut encoded_handles) {
        Ok(encoded) => encoded,
        Err(_) => return Status::ErrInvalidArgs,
    };
    if registry.send(&bytes[..8 + encoded.bytes], handles).is_err() {
        return Status::ErrPeerClosed;
    }
    match registry.try_recv() {
        Ok(reply) => DeviceRegistryRegisterDeviceNodeResponse::decode(&reply.bytes, &[])
            .map(|response| response.status)
            .unwrap_or(Status::ErrInvalidArgs),
        Err(_) => Status::ErrTimedOut,
    }
}

fn properties(interface: &Interface) -> Vec<DeviceProperty<'static>> {
    let class = match interface.class {
        InterfaceClass::Hub => 1,
        InterfaceClass::BootKeyboard => 2,
        InterfaceClass::BootMouse => 3,
        InterfaceClass::MassStorageBulkOnly => 4,
    };
    alloc::vec![
        DeviceProperty {
            key: "usb.device_id",
            value: interface.device_id as u32,
        },
        DeviceProperty {
            key: "usb.interface_id",
            value: interface.id as u32,
        },
        DeviceProperty {
            key: "usb.interface_number",
            value: interface.number as u32,
        },
        DeviceProperty {
            key: "usb.class",
            value: interface.class_code as u32,
        },
        DeviceProperty {
            key: "usb.subclass",
            value: interface.subclass as u32,
        },
        DeviceProperty {
            key: "usb.protocol",
            value: interface.protocol as u32,
        },
        DeviceProperty {
            key: "usb.policy_class",
            value: class,
        },
    ]
}
