use alloc::vec::Vec;

use bexos_network_extension_abi::{Direction, MAX_PACKET_BYTES, PacketDescriptor};

use crate::switch::SwitchError;

pub const ETHERTYPE_IPV4: u16 = 0x0800;
pub const ETHERTYPE_ARP: u16 = 0x0806;
pub const ETHERTYPE_IPV6: u16 = 0x86dd;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IpAddress {
    V4([u8; 4]),
    V6([u8; 16]),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PacketDisposition {
    Continue,
    Drop,
    Deliver(u64),
    Transmit(u64),
}

#[derive(Clone, Debug)]
pub struct Packet {
    pub bytes: Vec<u8>,
    pub descriptor: PacketDescriptor,
    pub ether_type: u16,
    pub source_mac: [u8; 6],
    pub destination_mac: [u8; 6],
    pub source_ip: Option<IpAddress>,
    pub destination_ip: Option<IpAddress>,
    pub disposition: PacketDisposition,
    pub rewritten: bool,
}

impl Packet {
    pub fn parse(
        bytes: &[u8],
        ingress_interface: u64,
        direction: Direction,
        table_id: u32,
        source_zone: u16,
    ) -> Result<Self, SwitchError> {
        if !(14..=MAX_PACKET_BYTES).contains(&bytes.len()) {
            return Err(SwitchError::InvalidFrame);
        }
        let destination_mac = bytes[0..6].try_into().unwrap();
        let source_mac = bytes[6..12].try_into().unwrap();
        let mut ether_type = u16::from_be_bytes([bytes[12], bytes[13]]);
        let mut l3 = 14usize;
        let mut vlan_id = 0;
        if matches!(ether_type, 0x8100 | 0x88a8) {
            if bytes.len() < 18 {
                return Err(SwitchError::InvalidFrame);
            }
            vlan_id = u16::from_be_bytes([bytes[14], bytes[15]]) & 0x0fff;
            if vlan_id == 0 || vlan_id > 4094 {
                return Err(SwitchError::InvalidVlan);
            }
            ether_type = u16::from_be_bytes([bytes[16], bytes[17]]);
            l3 = 18;
        }
        let mut descriptor = PacketDescriptor {
            length: bytes.len() as u32,
            ingress_interface,
            table_id,
            l3_offset: l3 as u16,
            vlan_id,
            source_zone,
            direction: direction as u8,
            ..PacketDescriptor::default()
        };
        let (source_ip, destination_ip) = match ether_type {
            ETHERTYPE_IPV4 => {
                let (source, destination, protocol, l4, fragments) = parse_ipv4(bytes, l3)?;
                descriptor.ip_version = 4;
                descriptor.protocol = protocol;
                descriptor.l4_offset = l4 as u16;
                descriptor.fragment_flags = fragments;
                (
                    Some(IpAddress::V4(source)),
                    Some(IpAddress::V4(destination)),
                )
            }
            ETHERTYPE_IPV6 => {
                let (source, destination, protocol, l4, fragments) = parse_ipv6(bytes, l3)?;
                descriptor.ip_version = 6;
                descriptor.protocol = protocol;
                descriptor.l4_offset = l4 as u16;
                descriptor.fragment_flags = fragments;
                (
                    Some(IpAddress::V6(source)),
                    Some(IpAddress::V6(destination)),
                )
            }
            _ => (None, None),
        };
        descriptor.flow_hash = flow_hash(&descriptor, source_ip, destination_ip, bytes);
        Ok(Self {
            bytes: bytes.to_vec(),
            descriptor,
            ether_type,
            source_mac,
            destination_mac,
            source_ip,
            destination_ip,
            disposition: PacketDisposition::Continue,
            rewritten: false,
        })
    }

    pub fn decrement_hop_limit(&mut self) -> Result<(), SwitchError> {
        let l3 = usize::from(self.descriptor.l3_offset);
        match self.descriptor.ip_version {
            4 => {
                if self.bytes[l3 + 8] <= 1 {
                    return Err(SwitchError::TtlExpired);
                }
                let old = u16::from_be_bytes([self.bytes[l3 + 8], self.bytes[l3 + 9]]);
                self.bytes[l3 + 8] -= 1;
                let new = u16::from_be_bytes([self.bytes[l3 + 8], self.bytes[l3 + 9]]);
                let checksum = u16::from_be_bytes([self.bytes[l3 + 10], self.bytes[l3 + 11]]);
                let updated = checksum_update(checksum, old, new);
                self.bytes[l3 + 10..l3 + 12].copy_from_slice(&updated.to_be_bytes());
            }
            6 => {
                if self.bytes[l3 + 7] <= 1 {
                    return Err(SwitchError::TtlExpired);
                }
                self.bytes[l3 + 7] -= 1;
            }
            _ => return Err(SwitchError::InvalidFrame),
        }
        self.rewritten = true;
        Ok(())
    }

    pub fn refresh_network_metadata(&mut self) -> Result<(), SwitchError> {
        let direction = match self.descriptor.direction {
            value if value == Direction::PhysicalIngress as u8 => Direction::PhysicalIngress,
            value if value == Direction::VirtualIngress as u8 => Direction::VirtualIngress,
            _ => return Err(SwitchError::InvalidFrame),
        };
        let parsed = Self::parse(
            &self.bytes,
            self.descriptor.ingress_interface,
            direction,
            self.descriptor.table_id,
            self.descriptor.source_zone,
        )?;
        let egress = self.descriptor.egress_interface;
        let destination_zone = self.descriptor.destination_zone;
        let disposition = self.disposition;
        self.descriptor = parsed.descriptor;
        self.descriptor.egress_interface = egress;
        self.descriptor.destination_zone = destination_zone;
        self.ether_type = parsed.ether_type;
        self.source_mac = parsed.source_mac;
        self.destination_mac = parsed.destination_mac;
        self.source_ip = parsed.source_ip;
        self.destination_ip = parsed.destination_ip;
        self.disposition = disposition;
        Ok(())
    }

    pub fn rewrite_ethernet(&mut self, source: [u8; 6], destination: [u8; 6]) {
        self.bytes[..6].copy_from_slice(&destination);
        self.bytes[6..12].copy_from_slice(&source);
        self.destination_mac = destination;
        self.source_mac = source;
        self.rewritten = true;
    }

    pub fn fragment_ipv4(&self, mtu: usize) -> Result<Vec<Self>, SwitchError> {
        if self.descriptor.ip_version != 4 {
            return Err(SwitchError::MtuExceeded);
        }
        let l3 = usize::from(self.descriptor.l3_offset);
        let header_len = usize::from(self.bytes[l3] & 0x0f) * 4;
        let total_len = usize::from(u16::from_be_bytes([self.bytes[l3 + 2], self.bytes[l3 + 3]]));
        let fragment_word = u16::from_be_bytes([self.bytes[l3 + 6], self.bytes[l3 + 7]]);
        if fragment_word & 0x4000 != 0 || mtu <= header_len || l3 + total_len > self.bytes.len() {
            return Err(SwitchError::MtuExceeded);
        }
        let payload_per_fragment = ((mtu - header_len) / 8) * 8;
        if payload_per_fragment == 0 {
            return Err(SwitchError::MtuExceeded);
        }
        let payload = &self.bytes[l3 + header_len..l3 + total_len];
        let base_offset = fragment_word & 0x1fff;
        let inherited_more = fragment_word & 0x2000 != 0;
        let direction = match self.descriptor.direction {
            value if value == Direction::PhysicalIngress as u8 => Direction::PhysicalIngress,
            value if value == Direction::VirtualIngress as u8 => Direction::VirtualIngress,
            _ => return Err(SwitchError::InvalidFrame),
        };
        let mut fragments = Vec::new();
        for (index, chunk) in payload.chunks(payload_per_fragment).enumerate() {
            let mut bytes = self.bytes[..l3 + header_len].to_vec();
            bytes.extend_from_slice(chunk);
            let length = header_len + chunk.len();
            bytes[l3 + 2..l3 + 4].copy_from_slice(&(length as u16).to_be_bytes());
            let offset = base_offset.saturating_add(
                u16::try_from(index * payload_per_fragment / 8)
                    .map_err(|_| SwitchError::MtuExceeded)?,
            );
            let more = inherited_more || (index + 1) * payload_per_fragment < payload.len();
            let word = offset | if more { 0x2000 } else { 0 };
            bytes[l3 + 6..l3 + 8].copy_from_slice(&word.to_be_bytes());
            bytes[l3 + 10..l3 + 12].fill(0);
            let checksum = ipv4_checksum(&bytes[l3..l3 + header_len]);
            bytes[l3 + 10..l3 + 12].copy_from_slice(&checksum.to_be_bytes());
            let mut fragment = Self::parse(
                &bytes,
                self.descriptor.ingress_interface,
                direction,
                self.descriptor.table_id,
                self.descriptor.source_zone,
            )?;
            fragment.descriptor.egress_interface = self.descriptor.egress_interface;
            fragment.descriptor.destination_zone = self.descriptor.destination_zone;
            fragment.disposition = self.disposition;
            fragments.push(fragment);
        }
        Ok(fragments)
    }
}

fn parse_ipv4(bytes: &[u8], l3: usize) -> Result<([u8; 4], [u8; 4], u8, usize, u8), SwitchError> {
    if bytes.len() < l3 + 20 || bytes[l3] >> 4 != 4 {
        return Err(SwitchError::InvalidFrame);
    }
    let ihl = usize::from(bytes[l3] & 0x0f) * 4;
    if ihl < 20 || ihl > 60 || bytes.len() < l3 + ihl {
        return Err(SwitchError::InvalidFrame);
    }
    let total = usize::from(u16::from_be_bytes([bytes[l3 + 2], bytes[l3 + 3]]));
    if total < ihl || l3 + total > bytes.len() || ipv4_checksum(&bytes[l3..l3 + ihl]) != 0 {
        return Err(SwitchError::InvalidFrame);
    }
    let fragment = u16::from_be_bytes([bytes[l3 + 6], bytes[l3 + 7]]);
    let flags = u8::from(fragment & 0x1fff != 0) | (u8::from(fragment & 0x2000 != 0) << 1);
    Ok((
        bytes[l3 + 12..l3 + 16].try_into().unwrap(),
        bytes[l3 + 16..l3 + 20].try_into().unwrap(),
        bytes[l3 + 9],
        l3 + ihl,
        flags,
    ))
}

fn parse_ipv6(bytes: &[u8], l3: usize) -> Result<([u8; 16], [u8; 16], u8, usize, u8), SwitchError> {
    if bytes.len() < l3 + 40 || bytes[l3] >> 4 != 6 {
        return Err(SwitchError::InvalidFrame);
    }
    let payload = usize::from(u16::from_be_bytes([bytes[l3 + 4], bytes[l3 + 5]]));
    if l3 + 40 + payload > bytes.len() {
        return Err(SwitchError::InvalidFrame);
    }
    let mut next = bytes[l3 + 6];
    let mut offset = l3 + 40;
    let mut fragments = 0;
    for _ in 0..8 {
        match next {
            0 | 43 | 60 => {
                if offset + 2 > bytes.len() {
                    return Err(SwitchError::InvalidFrame);
                }
                let length = (usize::from(bytes[offset + 1]) + 1) * 8;
                if length < 8 || offset + length > bytes.len() {
                    return Err(SwitchError::InvalidFrame);
                }
                next = bytes[offset];
                offset += length;
            }
            44 => {
                if offset + 8 > bytes.len() {
                    return Err(SwitchError::InvalidFrame);
                }
                let word = u16::from_be_bytes([bytes[offset + 2], bytes[offset + 3]]);
                fragments = u8::from(word & 0xfff8 != 0) | (u8::from(word & 1 != 0) << 1);
                next = bytes[offset];
                offset += 8;
            }
            51 => {
                if offset + 2 > bytes.len() {
                    return Err(SwitchError::InvalidFrame);
                }
                let length = (usize::from(bytes[offset + 1]) + 2) * 4;
                if offset + length > bytes.len() {
                    return Err(SwitchError::InvalidFrame);
                }
                next = bytes[offset];
                offset += length;
            }
            _ => break,
        }
    }
    Ok((
        bytes[l3 + 8..l3 + 24].try_into().unwrap(),
        bytes[l3 + 24..l3 + 40].try_into().unwrap(),
        next,
        offset,
        fragments,
    ))
}

fn ipv4_checksum(bytes: &[u8]) -> u16 {
    let mut sum = 0u32;
    for pair in bytes.chunks(2) {
        sum += u32::from(u16::from_be_bytes([pair[0], *pair.get(1).unwrap_or(&0)]));
    }
    while sum > 0xffff {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

fn checksum_update(checksum: u16, old: u16, new: u16) -> u16 {
    let mut sum = u32::from(!checksum) + u32::from(!old) + u32::from(new);
    while sum > 0xffff {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

fn flow_hash(
    descriptor: &PacketDescriptor,
    source: Option<IpAddress>,
    destination: Option<IpAddress>,
    bytes: &[u8],
) -> u32 {
    let mut hash = 0x811c_9dc5u32;
    let mut feed = |slice: &[u8]| {
        for byte in slice {
            hash = (hash ^ u32::from(*byte)).wrapping_mul(0x0100_0193);
        }
    };
    match source {
        Some(IpAddress::V4(value)) => feed(&value),
        Some(IpAddress::V6(value)) => feed(&value),
        None => {}
    }
    match destination {
        Some(IpAddress::V4(value)) => feed(&value),
        Some(IpAddress::V6(value)) => feed(&value),
        None => {}
    }
    feed(&[descriptor.protocol]);
    let l4 = usize::from(descriptor.l4_offset);
    if matches!(descriptor.protocol, 6 | 17) && bytes.len() >= l4 + 4 {
        feed(&bytes[l4..l4 + 4]);
    }
    hash
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use super::*;

    #[test]
    fn ipv4_hop_decrement_updates_checksum() {
        let mut frame = vec![0u8; 14 + 20];
        frame[12..14].copy_from_slice(&ETHERTYPE_IPV4.to_be_bytes());
        frame[14] = 0x45;
        frame[16..18].copy_from_slice(&20u16.to_be_bytes());
        frame[22] = 64;
        frame[23] = 17;
        frame[26..30].copy_from_slice(&[10, 0, 0, 1]);
        frame[30..34].copy_from_slice(&[10, 0, 0, 2]);
        let checksum = ipv4_checksum(&frame[14..34]);
        frame[24..26].copy_from_slice(&checksum.to_be_bytes());
        let mut packet = Packet::parse(&frame, 1, Direction::VirtualIngress, 0, 1).unwrap();
        packet.decrement_hop_limit().unwrap();
        assert_eq!(packet.bytes[22], 63);
        assert_eq!(ipv4_checksum(&packet.bytes[14..34]), 0);
    }

    #[test]
    fn ipv4_fragmentation_honors_mtu_and_offsets() {
        let mut bytes = alloc::vec![0u8; 14 + 20 + 2000];
        bytes[..6].copy_from_slice(&[2, 0, 0, 0, 0, 2]);
        bytes[6..12].copy_from_slice(&[2, 0, 0, 0, 0, 1]);
        bytes[12..14].copy_from_slice(&ETHERTYPE_IPV4.to_be_bytes());
        bytes[14] = 0x45;
        bytes[16..18].copy_from_slice(&2020u16.to_be_bytes());
        bytes[22] = 64;
        bytes[23] = 17;
        bytes[26..30].copy_from_slice(&[10, 0, 0, 1]);
        bytes[30..34].copy_from_slice(&[10, 0, 0, 2]);
        let checksum = ipv4_checksum(&bytes[14..34]);
        bytes[24..26].copy_from_slice(&checksum.to_be_bytes());
        let packet = Packet::parse(&bytes, 1, Direction::VirtualIngress, 1, 1).unwrap();
        let fragments = packet.fragment_ipv4(1500).unwrap();
        assert_eq!(fragments.len(), 2);
        assert!(
            fragments
                .iter()
                .all(|fragment| fragment.bytes.len() <= 1514)
        );
        assert_eq!(
            u16::from_be_bytes(fragments[0].bytes[20..22].try_into().unwrap()) & 0x2000,
            0x2000
        );
        assert_eq!(
            u16::from_be_bytes(fragments[1].bytes[20..22].try_into().unwrap()) & 0x1fff,
            185
        );
    }
}
