use alloc::{format, vec, vec::Vec};
use bexos_userspace::{Channel, KernelTransport, Memory, log};
use hardware_manager_fidl::*;
use kernel_fidl::{
    HandleRef as KernelHandleRef, Status as KernelStatus,
    SystemPrivilegedBexosSystemPrivilegedClient, SystemPrivilegedBindInterruptRequest,
};
struct Handles(Vec<u64>);
impl Drop for Handles {
    fn drop(&mut self) {
        for h in &self.0 {
            let _ = Memory::close(*h);
        }
    }
}
pub struct Registration {
    pub control: Channel,
    pub interrupt: u64,
}

pub fn register(channel: Channel, node: &bexos_d1_pci::DeviceNodeInfo) -> Result<Registration, ()> {
    let properties: Vec<_> = node
        .properties
        .iter()
        .map(|p| DeviceProperty {
            key: p.key,
            value: p.value,
        })
        .collect();
    let mut owned = Handles(Vec::with_capacity(6));
    let mut resources = Vec::with_capacity(6);
    for bar in &node.bars {
        let handle = Memory::physical(bar.base, bar.size).map_err(|status| {
            log(&format!(
                "pci: BAR capability failed node={} bar={} base={:#x} size={:#x} status={status:?}\n",
                node.node_id, bar.index, bar.base, bar.size
            ));
        })?;
        owned.0.push(handle);
        resources.push(HardwareResource {
            kind: HardwareResourceKind::Mmio,
            resource_id: bar.index as u64,
            base: bar.base,
            length: bar.size,
            flags: 0,
            resource: HandleRef { raw: handle },
        });
    }
    let domain = Memory::create_iommu_domain(node.node_id, 48).map_err(|status| {
        log(&format!(
            "pci: IOMMU domain failed node={} status={status:?}\n",
            node.node_id
        ));
    })?;
    owned.0.push(domain);
    resources.push(HardwareResource {
        kind: HardwareResourceKind::IommuDomain,
        resource_id: 0x1_0000_0000 | node.node_id,
        base: node.node_id,
        length: 1,
        flags: 0,
        resource: HandleRef { raw: domain },
    });
    let irq_number = interrupt_number(node.node_id);
    let irq = bind_interrupt(node.node_id, irq_number).map_err(|()| {
        log(&format!(
            "pci: interrupt bind failed node={} irq={}\n",
            node.node_id, irq_number
        ));
    })?;
    let retained_irq = Memory::duplicate(irq, 1 | 2 | 4 | 32).map_err(|status| {
        log(&format!(
            "pci: interrupt retention failed node={} irq={} status={status:?}\n",
            node.node_id, irq_number
        ));
    })?;
    let mut retained_irq_guard = Handles(vec![retained_irq]);
    owned.0.push(irq);
    resources.push(HardwareResource {
        kind: HardwareResourceKind::Interrupt,
        resource_id: 0x3_0000_0000 | node.node_id,
        base: u64::from(irq_number),
        length: 1,
        flags: 1,
        resource: HandleRef { raw: irq },
    });
    let (control, client) = Channel::pair().map_err(|status| {
        log(&format!(
            "pci: bus-control channel failed node={} status={status:?}\n",
            node.node_id
        ));
    })?;
    owned.0.push(client.0);
    resources.push(HardwareResource {
        kind: HardwareResourceKind::BusControl,
        resource_id: 0x2_0000_0000 | node.node_id,
        base: node.node_id,
        length: 1,
        flags: 0,
        resource: HandleRef { raw: client.0 },
    });
    let request = DeviceRegistryRegisterDeviceNodeRequest {
        info: DeviceNodeInfo {
            node_id: node.node_id,
            bus: BusType::Pci,
            has_parent: false,
            parent_node_id: 0,
            topological_path: &node.topological_path,
            properties: WireVector::from_slice(&properties),
        },
        resources: WireVector::from_slice(&resources),
    };
    let mut bytes = [0; 2048];
    let mut handles = [HandleRef { raw: 0 }; 16];
    bytes[..8].copy_from_slice(&1u64.to_le_bytes());
    let n = request
        .encode(&mut bytes[8..], &mut handles)
        .map_err(|error| {
            log(&format!(
                "pci: registry encode failed node={} error={error:?}\n",
                node.node_id
            ));
        })?;
    channel
        .send(&bytes[..8 + n.bytes], &owned.0)
        .map_err(|status| {
            log(&format!(
                "pci: registry channel send failed node={} status={status:?}\n",
                node.node_id
            ));
        })?;
    owned.0.clear();
    retained_irq_guard.0.clear();
    Ok(Registration {
        control,
        interrupt: retained_irq,
    })
}

fn bind_interrupt(node_id: u64, irq_number: u32) -> Result<u64, ()> {
    let mut client = SystemPrivilegedBexosSystemPrivilegedClient::new(KernelTransport(4));
    let mut request_bytes = [0; 32];
    let mut response_bytes = [0; 32];
    let mut request_handles = [KernelHandleRef { raw: 0 }; 1];
    let mut response_handles = [KernelHandleRef { raw: 0 }; 1];
    let response = client
        .bind_interrupt(
            &SystemPrivilegedBindInterruptRequest {
                irq_number,
                flags: 1,
            },
            &mut request_bytes,
            &mut request_handles,
            &mut response_bytes,
            &mut response_handles,
        )
        .map_err(|error| {
            log(&format!(
                "pci: interrupt transport failed node={node_id} irq={irq_number} error={error:?}\n"
            ));
        })?;
    if response.status != KernelStatus::Ok || response.irq_handle.raw == 0 {
        log(&format!(
            "pci: interrupt service rejected node={node_id} irq={irq_number} status={:?} handle={}\n",
            response.status, response.irq_handle.raw
        ));
        return Err(());
    }
    Ok(response.irq_handle.raw)
}

const fn interrupt_number(node_id: u64) -> u32 {
    let device = ((node_id >> 8) & 0x1f) as u32;
    #[cfg(all(bexos_guest, target_arch = "aarch64"))]
    {
        // QEMU virt routes PCI INTx A-D to GIC SPIs 3-6.  PCI functions use
        // the standard slot swizzle; firmware assigns MSI/MSI-X later through
        // PciDeviceControl when a driver requests it.
        35 + (device & 3)
    }
    #[cfg(not(all(bexos_guest, target_arch = "aarch64")))]
    {
        // ACPI _PRT/Q35 exposes the four shared PCI INTx links at GSIs 16-19.
        16 + (device & 3)
    }
}
pub fn unregister(channel: Channel, node: u64) -> Result<(), ()> {
    let mut bytes = [0; 32];
    bytes[..8].copy_from_slice(&2u64.to_le_bytes());
    let n = DeviceRegistryUnregisterDeviceNodeRequest { node_id: node }
        .encode(&mut bytes[8..], &mut [])
        .map_err(|_| ())?;
    channel.send(&bytes[..8 + n.bytes], &[]).map_err(|_| ())
}
pub fn response(message: bexos_userspace::ipc::Message) -> Result<(), ()> {
    let handles = Handles(message.handles);
    if !handles.0.is_empty() {
        return Err(());
    }
    let response =
        DeviceRegistryRegisterDeviceNodeResponse::decode(&message.bytes, &[]).map_err(|_| ())?;
    if response.status == Status::Ok {
        Ok(())
    } else {
        Err(())
    }
}
