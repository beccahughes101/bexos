#![no_std]

pub const ABI_VERSION: u32 = 1;
pub const VECTOR_BATCH_SIZE: usize = 256;
pub const MAX_PACKET_BYTES: usize = 9216;
pub const MAX_CONFIG_BYTES: usize = 65_536;
pub const MAX_FIREWALL_RULES: usize = 128;
pub const MAX_TRACKED_FLOWS: usize = 4096;
pub const MAX_NAT_MAPPINGS: usize = 4096;

pub const IMPORT_MODULE: &str = "bexos:net/extension@1.0.0";
pub const IMPORT_MONOTONIC_NS: &str = "monotonic-ns";
pub const EXPORT_ABI_VERSION: &str = "bexos_extension_abi_version";
pub const EXPORT_BUFFER_PTR: &str = "bexos_extension_buffer_ptr";
pub const EXPORT_BUFFER_CAPACITY: &str = "bexos_extension_buffer_capacity";
pub const EXPORT_CONFIGURE: &str = "bexos_extension_configure";
pub const EXPORT_PROCESS: &str = "bexos_extension_process_vector";
pub const EXPORT_CHECKPOINT_PTR: &str = "bexos_extension_checkpoint_ptr";
pub const EXPORT_CHECKPOINT_LEN: &str = "bexos_extension_checkpoint_len";

#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Hook {
    #[default]
    Bridge = 1,
    PreRouting = 2,
    Firewall = 3,
    PostRouting = 4,
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Direction {
    #[default]
    PhysicalIngress = 1,
    VirtualIngress = 2,
}

#[repr(u16)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ActionCode {
    Drop = 0,
    #[default]
    Pass = 1,
    Rewrite = 2,
    Redirect = 3,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PacketDescriptor {
    pub packet_offset: u32,
    pub length: u32,
    pub ingress_interface: u64,
    pub egress_interface: u64,
    pub table_id: u32,
    pub flow_hash: u32,
    pub l2_offset: u16,
    pub l3_offset: u16,
    pub l4_offset: u16,
    pub vlan_id: u16,
    pub source_zone: u16,
    pub destination_zone: u16,
    pub direction: u8,
    pub ip_version: u8,
    pub protocol: u8,
    pub fragment_flags: u8,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PacketAction {
    pub code: u16,
    pub flags: u16,
    pub redirect_interface: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct VectorHeader {
    pub abi_version: u32,
    pub hook: u8,
    pub reserved: [u8; 3],
    pub count: u32,
    pub descriptors_offset: u32,
    pub actions_offset: u32,
    pub packets_offset: u32,
    pub packets_length: u32,
}

impl VectorHeader {
    pub fn checked_layout(count: usize, packet_bytes: usize, capacity: usize) -> Option<Self> {
        if count > VECTOR_BATCH_SIZE || packet_bytes > VECTOR_BATCH_SIZE * MAX_PACKET_BYTES {
            return None;
        }
        let descriptors_offset = core::mem::size_of::<Self>();
        let actions_offset = descriptors_offset
            .checked_add(count.checked_mul(core::mem::size_of::<PacketDescriptor>())?)?;
        let packets_offset =
            actions_offset.checked_add(count.checked_mul(core::mem::size_of::<PacketAction>())?)?;
        let end = packets_offset.checked_add(packet_bytes)?;
        if end > capacity || end > u32::MAX as usize {
            return None;
        }
        Some(Self {
            abi_version: ABI_VERSION,
            hook: Hook::Bridge as u8,
            reserved: [0; 3],
            count: count as u32,
            descriptors_offset: descriptors_offset as u32,
            actions_offset: actions_offset as u32,
            packets_offset: packets_offset as u32,
            packets_length: packet_bytes as u32,
        })
    }
}

pub fn valid_action(code: u16) -> bool {
    code <= ActionCode::Redirect as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vector_layout_is_bounded() {
        let layout = VectorHeader::checked_layout(256, 256 * 64, 64 * 1024).unwrap();
        assert_eq!(layout.count, 256);
        assert!(VectorHeader::checked_layout(257, 0, usize::MAX).is_none());
        assert!(VectorHeader::checked_layout(1, 64, 8).is_none());
    }
}
