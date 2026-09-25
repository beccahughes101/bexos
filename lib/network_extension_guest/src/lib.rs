#![no_std]

use bexos_network_extension_abi::{
    ABI_VERSION, PacketAction, PacketDescriptor, VECTOR_BATCH_SIZE, VectorHeader,
};

pub const VECTOR_BUFFER_CAPACITY: usize = core::mem::size_of::<VectorHeader>()
    + VECTOR_BATCH_SIZE * core::mem::size_of::<PacketDescriptor>()
    + VECTOR_BATCH_SIZE * core::mem::size_of::<PacketAction>()
    + VECTOR_BATCH_SIZE * bexos_network_extension_abi::MAX_PACKET_BYTES;

#[derive(Clone, Copy)]
pub struct Tuple {
    pub version: u8,
    pub protocol: u8,
    pub source: [u8; 16],
    pub destination: [u8; 16],
    pub source_port: u16,
    pub destination_port: u16,
    pub fragment: bool,
}

#[derive(Clone, Copy)]
pub struct Fragment {
    pub id: u32,
    pub offset: u16,
    pub more: bool,
}

pub struct VectorView {
    base: *mut u8,
    header: VectorHeader,
}

impl VectorView {
    /// `base..base+capacity` must be the module-owned exchange buffer.
    pub unsafe fn parse(base: *mut u8, capacity: usize) -> Option<Self> {
        let header = unsafe { core::ptr::read_unaligned(base.cast::<VectorHeader>()) };
        if header.abi_version != ABI_VERSION || header.count as usize > VECTOR_BATCH_SIZE {
            return None;
        }
        VectorHeader::checked_layout(
            header.count as usize,
            header.packets_length as usize,
            capacity,
        )
        .filter(|expected| {
            expected.descriptors_offset == header.descriptors_offset
                && expected.actions_offset == header.actions_offset
                && expected.packets_offset == header.packets_offset
        })?;
        Some(Self { base, header })
    }

    pub fn count(&self) -> usize {
        self.header.count as usize
    }

    pub fn hook(&self) -> u8 {
        self.header.hook
    }

    pub unsafe fn descriptor(&self, index: usize) -> Option<PacketDescriptor> {
        if index >= self.count() {
            return None;
        }
        let offset = self.header.descriptors_offset as usize
            + index * core::mem::size_of::<PacketDescriptor>();
        Some(unsafe { core::ptr::read_unaligned(self.base.add(offset).cast::<PacketDescriptor>()) })
    }

    pub unsafe fn set_action(&mut self, index: usize, action: PacketAction) -> bool {
        if index >= self.count() {
            return false;
        }
        let offset =
            self.header.actions_offset as usize + index * core::mem::size_of::<PacketAction>();
        unsafe {
            core::ptr::write_unaligned(self.base.add(offset).cast::<PacketAction>(), action);
        }
        true
    }

    pub unsafe fn packet_mut(&mut self, descriptor: PacketDescriptor) -> Option<&mut [u8]> {
        let start = self
            .header
            .packets_offset
            .checked_add(descriptor.packet_offset)? as usize;
        let end = start.checked_add(descriptor.length as usize)?;
        let packet_end = self.header.packets_offset as usize + self.header.packets_length as usize;
        if end > packet_end {
            return None;
        }
        Some(unsafe { core::slice::from_raw_parts_mut(self.base.add(start), end - start) })
    }
}

pub fn tuple(packet: &[u8], descriptor: PacketDescriptor) -> Option<Tuple> {
    let l3 = descriptor.l3_offset as usize;
    let l4 = descriptor.l4_offset as usize;
    let mut value = Tuple {
        version: descriptor.ip_version,
        protocol: descriptor.protocol,
        source: [0; 16],
        destination: [0; 16],
        source_port: 0,
        destination_port: 0,
        fragment: descriptor.fragment_flags != 0,
    };
    match descriptor.ip_version {
        4 if packet.len() >= l3 + 20 => {
            value.source[..4].copy_from_slice(&packet[l3 + 12..l3 + 16]);
            value.destination[..4].copy_from_slice(&packet[l3 + 16..l3 + 20]);
        }
        6 if packet.len() >= l3 + 40 => {
            value.source.copy_from_slice(&packet[l3 + 8..l3 + 24]);
            value.destination.copy_from_slice(&packet[l3 + 24..l3 + 40]);
        }
        _ => return None,
    }
    if matches!(descriptor.protocol, 6 | 17) && packet.len() >= l4 + 4 {
        value.source_port = u16::from_be_bytes([packet[l4], packet[l4 + 1]]);
        value.destination_port = u16::from_be_bytes([packet[l4 + 2], packet[l4 + 3]]);
    } else if matches!(descriptor.protocol, 1 | 58) && packet.len() >= l4 + 6 {
        value.source_port = u16::from_be_bytes([packet[l4 + 4], packet[l4 + 5]]);
    }
    Some(value)
}

pub fn fragment(packet: &[u8], descriptor: PacketDescriptor) -> Option<Fragment> {
    if descriptor.fragment_flags == 0 {
        return None;
    }
    let l3 = descriptor.l3_offset as usize;
    match descriptor.ip_version {
        4 if packet.len() >= l3 + 20 => {
            let word = u16::from_be_bytes([packet[l3 + 6], packet[l3 + 7]]);
            Some(Fragment {
                id: u32::from(u16::from_be_bytes([packet[l3 + 4], packet[l3 + 5]])),
                offset: word & 0x1fff,
                more: word & 0x2000 != 0,
            })
        }
        6 if packet.len() >= l3 + 48 => {
            let mut next = packet[l3 + 6];
            let mut offset = l3 + 40;
            for _ in 0..8 {
                match next {
                    44 if packet.len() >= offset + 8 => {
                        let word = u16::from_be_bytes([packet[offset + 2], packet[offset + 3]]);
                        return Some(Fragment {
                            id: u32::from_be_bytes(packet[offset + 4..offset + 8].try_into().ok()?),
                            offset: (word & 0xfff8) >> 3,
                            more: word & 1 != 0,
                        });
                    }
                    0 | 43 | 60 if packet.len() >= offset + 2 => {
                        let length = (usize::from(packet[offset + 1]) + 1) * 8;
                        next = packet[offset];
                        offset = offset.checked_add(length)?;
                    }
                    51 if packet.len() >= offset + 2 => {
                        let length = (usize::from(packet[offset + 1]) + 2) * 4;
                        next = packet[offset];
                        offset = offset.checked_add(length)?;
                    }
                    _ => return None,
                }
            }
            None
        }
        _ => None,
    }
}

pub fn replace_checksum_word(checksum: &mut [u8], old: u16, new: u16) {
    let current = u32::from(!u16::from_be_bytes([checksum[0], checksum[1]]));
    let mut sum = current + u32::from(!old) + u32::from(new);
    while sum > 0xffff {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    checksum.copy_from_slice(&(!(sum as u16)).to_be_bytes());
}

pub fn replace_address(checksum: &mut [u8], old: &[u8], new: &[u8]) {
    for (before, after) in old.chunks_exact(2).zip(new.chunks_exact(2)) {
        replace_checksum_word(
            checksum,
            u16::from_be_bytes([before[0], before[1]]),
            u16::from_be_bytes([after[0], after[1]]),
        );
    }
}
