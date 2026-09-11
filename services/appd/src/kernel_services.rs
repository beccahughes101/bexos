use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;
use bexos_kernel_core::ipc::Capability;

use crate::{
    AppdBroker, BindError, CapabilityMetadata, ExposedService, Lifecycle, Metadata, Visibility,
};

pub const KERNEL_PROVIDER_PACKAGE: &str = "bexos.kernel:kernel";
pub const SYSTEM_PRIVILEGED_PERMISSION: &str = "BEXOS_SYSTEM_PRIVILEGED";

pub const CHANNEL_CONTROL_ENDPOINT_ID: u64 = 0x4b43_0001;
pub const SOCKET_CONTROL_ENDPOINT_ID: u64 = 0x4b43_000a;
pub const VIRTUAL_MEMORY_ENDPOINT_ID: u64 = 0x4b43_0002;
pub const TASK_CONTROL_ENDPOINT_ID: u64 = 0x4b43_0003;
pub const SYSTEM_PRIVILEGED_ENDPOINT_ID: u64 = 0x4b43_0004;
pub const CLOCK_ENDPOINT_ID: u64 = 0x4b43_0008;
pub const PROFILE_PROVIDER_ENDPOINT_ID: u64 = 0x4b43_000b;

pub fn publish_kernel_services(broker: &mut AppdBroker) -> Result<(), BindError> {
    publish(
        broker,
        "bexos.kernel.ChannelControl",
        "ChannelControl",
        None,
        kernel_capabilities("ChannelControl"),
        CHANNEL_CONTROL_ENDPOINT_ID,
    )?;
    publish(
        broker,
        "bexos.kernel.SocketControl",
        "SocketControl",
        None,
        kernel_capabilities("SocketControl"),
        SOCKET_CONTROL_ENDPOINT_ID,
    )?;
    publish(
        broker,
        "bexos.kernel.VirtualMemory",
        "VirtualMemory",
        None,
        kernel_capabilities("VirtualMemory"),
        VIRTUAL_MEMORY_ENDPOINT_ID,
    )?;
    publish(
        broker,
        "bexos.kernel.TaskControl",
        "TaskControl",
        None,
        kernel_capabilities("TaskControl"),
        TASK_CONTROL_ENDPOINT_ID,
    )?;
    publish(
        broker,
        "bexos.kernel.Clock",
        "Clock",
        None,
        kernel_capabilities("Clock"),
        CLOCK_ENDPOINT_ID,
    )?;
    publish(
        broker,
        "bexos.kernel.ProfileProvider",
        "ProfileProvider",
        None,
        kernel_capabilities("ProfileProvider"),
        PROFILE_PROVIDER_ENDPOINT_ID,
    )?;
    publish(
        broker,
        "bexos.kernel.SystemPrivileged",
        "SystemPrivileged",
        None,
        kernel_capabilities("SystemPrivileged"),
        SYSTEM_PRIVILEGED_ENDPOINT_ID,
    )
}

fn publish(
    broker: &mut AppdBroker,
    name: &str,
    protocol: &str,
    bind_permission: Option<&str>,
    capabilities: Vec<CapabilityMetadata>,
    endpoint_id: u64,
) -> Result<(), BindError> {
    broker.publish_interface(
        KERNEL_PROVIDER_PACKAGE,
        ExposedService {
            name: name.to_string(),
            protocol: protocol.to_string(),
            lifecycle: Lifecycle::Singleton,
            visibility: Visibility::Public,
            bind_permission: bind_permission.map(str::to_string),
            metadata: vec![Metadata {
                key: "provider".to_string(),
                value: "kernel".to_string(),
            }],
            capabilities,
            ..ExposedService::default()
        },
        Capability {
            object_id: endpoint_id,
            rights: 0b11,
        },
    )
}

fn kernel_capabilities(protocol: &str) -> Vec<CapabilityMetadata> {
    kernel_fidl::CAPABILITY_BINDINGS
        .iter()
        .filter(|binding| binding.protocol == protocol)
        .map(|binding| CapabilityMetadata {
            capability: binding.capability.to_string(),
            permission: binding.permission.map(str::to_string),
            method_ordinals: binding
                .methods
                .iter()
                .map(|method| method.ordinal)
                .collect(),
        })
        .collect()
}
