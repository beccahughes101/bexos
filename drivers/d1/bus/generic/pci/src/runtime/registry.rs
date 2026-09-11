use alloc::vec::Vec;
use bexos_userspace::{Channel, Memory};
use hardware_manager_fidl::*;
struct Handles(Vec<u64>);
impl Drop for Handles {
    fn drop(&mut self) {
        for h in &self.0 {
            let _ = Memory::close(*h);
        }
    }
}
pub fn register(channel: Channel, node: &bexos_d1_pci::DeviceNodeInfo) -> Result<(), ()> {
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
        let handle = Memory::physical(bar.base, bar.size).map_err(|_| ())?;
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
    let request = DeviceRegistryRegisterDeviceNodeRequest {
        info: DeviceNodeInfo {
            node_id: node.node_id,
            bus: BusType::Pci,
            has_parent: false,
            parent_node_id: 0,
            properties: WireVector::from_slice(&properties),
        },
        resources: WireVector::from_slice(&resources),
    };
    let mut bytes = [0; 2048];
    let mut handles = [HandleRef { raw: 0 }; 16];
    bytes[..8].copy_from_slice(&1u64.to_le_bytes());
    let n = request
        .encode(&mut bytes[8..], &mut handles)
        .map_err(|_| ())?;
    channel
        .send(&bytes[..8 + n.bytes], &owned.0)
        .map_err(|_| ())?;
    owned.0.clear();
    Ok(())
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
