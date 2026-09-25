use alloc::collections::{BTreeMap, VecDeque};
use alloc::vec::Vec;

use crate::packet::{IpAddress, Packet};
use crate::switch::SwitchError;

pub const MAX_NEIGHBORS: usize = 1024;
pub const MAX_QUEUED_PER_NEIGHBOR: usize = 64;
pub const MAX_QUEUED_PACKETS: usize = 256;
pub const REACHABLE_NS: u64 = 30_000_000_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct Key {
    interface_id: u64,
    family: u8,
    address: [u8; 16],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NeighborState {
    Incomplete,
    Reachable,
    Stale,
}

#[derive(Clone, Debug)]
pub struct Neighbor {
    pub interface_id: u64,
    pub address: IpAddress,
    pub mac: [u8; 6],
    pub state: NeighborState,
    pub updated_ns: u64,
    pub probes: u8,
    pub queued: VecDeque<Packet>,
}

#[derive(Clone, Debug, Default)]
pub struct NeighborTable {
    entries: BTreeMap<Key, Neighbor>,
}

impl NeighborTable {
    pub fn learn(
        &mut self,
        interface_id: u64,
        address: IpAddress,
        mac: [u8; 6],
        now_ns: u64,
    ) -> Result<Vec<Packet>, SwitchError> {
        if mac == [0; 6] || mac[0] & 1 != 0 {
            return Err(SwitchError::InvalidNeighbor);
        }
        let key = key(interface_id, address);
        if !self.entries.contains_key(&key) && self.entries.len() >= MAX_NEIGHBORS {
            self.evict_stale().ok_or(SwitchError::QueueFull)?;
        }
        let neighbor = self.entries.entry(key).or_insert_with(|| Neighbor {
            interface_id,
            address,
            mac,
            state: NeighborState::Reachable,
            updated_ns: now_ns,
            probes: 0,
            queued: VecDeque::new(),
        });
        neighbor.mac = mac;
        neighbor.state = NeighborState::Reachable;
        neighbor.updated_ns = now_ns;
        neighbor.probes = 0;
        Ok(neighbor.queued.drain(..).collect())
    }

    pub fn resolve(
        &mut self,
        interface_id: u64,
        address: IpAddress,
        now_ns: u64,
    ) -> Option<[u8; 6]> {
        let neighbor = self.entries.get_mut(&key(interface_id, address))?;
        if now_ns.saturating_sub(neighbor.updated_ns) > REACHABLE_NS {
            neighbor.state = NeighborState::Stale;
        }
        (neighbor.state != NeighborState::Incomplete).then_some(neighbor.mac)
    }

    pub fn queue(
        &mut self,
        interface_id: u64,
        address: IpAddress,
        packet: Packet,
        now_ns: u64,
    ) -> Result<bool, SwitchError> {
        if self
            .entries
            .values()
            .map(|entry| entry.queued.len())
            .sum::<usize>()
            >= MAX_QUEUED_PACKETS
        {
            return Err(SwitchError::QueueFull);
        }
        let key = key(interface_id, address);
        if !self.entries.contains_key(&key) && self.entries.len() >= MAX_NEIGHBORS {
            self.evict_stale().ok_or(SwitchError::QueueFull)?;
        }
        let neighbor = self.entries.entry(key).or_insert_with(|| Neighbor {
            interface_id,
            address,
            mac: [0; 6],
            state: NeighborState::Incomplete,
            updated_ns: now_ns,
            probes: 0,
            queued: VecDeque::new(),
        });
        if neighbor.queued.len() >= MAX_QUEUED_PER_NEIGHBOR {
            return Err(SwitchError::QueueFull);
        }
        neighbor.queued.push_back(packet);
        let should_probe =
            neighbor.probes == 0 || now_ns.saturating_sub(neighbor.updated_ns) >= 1_000_000_000;
        if should_probe {
            neighbor.probes = neighbor.probes.saturating_add(1);
            neighbor.updated_ns = now_ns;
        }
        Ok(should_probe)
    }

    pub fn entries(&self) -> impl Iterator<Item = &Neighbor> {
        self.entries.values()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn restore(&mut self, neighbor: Neighbor) -> Result<(), SwitchError> {
        if self.entries.len() >= MAX_NEIGHBORS
            || neighbor.queued.len() > MAX_QUEUED_PER_NEIGHBOR
            || neighbor.mac[0] & 1 != 0
        {
            return Err(SwitchError::InvalidNeighbor);
        }
        let entry_key = key(neighbor.interface_id, neighbor.address);
        if self.entries.insert(entry_key, neighbor).is_some() {
            return Err(SwitchError::AlreadyExists);
        }
        Ok(())
    }

    fn evict_stale(&mut self) -> Option<()> {
        let key = self
            .entries
            .iter()
            .filter(|(_, entry)| entry.queued.is_empty())
            .min_by_key(|(_, entry)| entry.updated_ns)
            .map(|(key, _)| *key)?;
        self.entries.remove(&key);
        Some(())
    }
}

fn key(interface_id: u64, address: IpAddress) -> Key {
    let mut bytes = [0; 16];
    let family = match address {
        IpAddress::V4(value) => {
            bytes[..4].copy_from_slice(&value);
            4
        }
        IpAddress::V6(value) => {
            bytes = value;
            6
        }
    };
    Key {
        interface_id,
        family,
        address: bytes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn learning_releases_queued_packets() {
        let mut table = NeighborTable::default();
        let packet = Packet {
            bytes: alloc::vec![0; 64],
            descriptor: Default::default(),
            ether_type: 0,
            source_mac: [0; 6],
            destination_mac: [0; 6],
            source_ip: None,
            destination_ip: None,
            disposition: crate::packet::PacketDisposition::Continue,
            rewritten: false,
        };
        assert!(
            table
                .queue(1, IpAddress::V4([1, 2, 3, 4]), packet, 1)
                .unwrap()
        );
        assert_eq!(
            table
                .learn(1, IpAddress::V4([1, 2, 3, 4]), [2, 0, 0, 0, 0, 1], 2)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            table.resolve(1, IpAddress::V4([1, 2, 3, 4]), 3),
            Some([2, 0, 0, 0, 0, 1])
        );
    }
}
