use alloc::collections::{BTreeMap, VecDeque};
use alloc::vec::Vec;

pub const MAX_FRAME_SIZE: usize = 9216;
pub const MIN_FRAME_SIZE: usize = 14;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PortPolicy {
    pub port_id: u64,
    pub physical_interface: u64,
    pub source_mac: [u8; 6],
    pub vlan_id: Option<u16>,
    pub tagged: bool,
    pub rx_capacity: usize,
    pub tx_capacity: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueuedFrame {
    pub generation: u64,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Port {
    pub policy: PortPolicy,
    pub rx: VecDeque<QueuedFrame>,
    pub delivered_rx: VecDeque<QueuedFrame>,
    pub staged_tx: VecDeque<QueuedFrame>,
    pub committed_generation: u64,
    pub dropped_rx: u64,
    pub dropped_tx: u64,
    pub spoof_rejections: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SwitchError {
    InvalidPort,
    InvalidFrame,
    InvalidVlan,
    AlreadyExists,
    NotFound,
    QueueFull,
    Spoofed,
    StaleGeneration,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct VirtualSwitch {
    pub(crate) ports: BTreeMap<u64, Port>,
}

impl VirtualSwitch {
    pub fn create_port(&mut self, policy: PortPolicy) -> Result<(), SwitchError> {
        if policy.port_id == 0
            || policy.source_mac[0] & 1 != 0
            || policy.source_mac == [0; 6]
            || policy.vlan_id.is_some_and(|vlan| vlan == 0 || vlan > 4094)
            || policy.rx_capacity == 0
            || policy.tx_capacity == 0
            || policy.rx_capacity > 4096
            || policy.tx_capacity > 4096
        {
            return Err(SwitchError::InvalidPort);
        }
        if self.ports.contains_key(&policy.port_id) {
            return Err(SwitchError::AlreadyExists);
        }
        self.ports.insert(
            policy.port_id,
            Port {
                policy,
                rx: VecDeque::new(),
                delivered_rx: VecDeque::new(),
                staged_tx: VecDeque::new(),
                committed_generation: 0,
                dropped_rx: 0,
                dropped_tx: 0,
                spoof_rejections: 0,
            },
        );
        Ok(())
    }

    pub fn remove_port(&mut self, port_id: u64) -> Result<Port, SwitchError> {
        self.ports.remove(&port_id).ok_or(SwitchError::NotFound)
    }

    pub fn port(&self, port_id: u64) -> Option<&Port> {
        self.ports.get(&port_id)
    }

    /// Copies a physical RX frame into each eligible port queue. No port ever
    /// maps the physical driver's RX pool, preserving cross-stack isolation.
    pub fn ingress(&mut self, physical_interface: u64, bytes: &[u8]) -> usize {
        let Ok(header) = FrameHeader::parse(bytes) else {
            return 0;
        };
        let fanout = header.destination[0] & 1 != 0 || header.destination == [0xff; 6];
        let mut delivered = 0;
        for port in self.ports.values_mut().filter(|port| {
            port.policy.physical_interface == physical_interface
                && ingress_vlan_matches(&port.policy, header.vlan_id)
                && (fanout || port.policy.source_mac == header.destination)
        }) {
            if port.rx.len().saturating_add(port.delivered_rx.len()) >= port.policy.rx_capacity {
                port.dropped_rx = port.dropped_rx.saturating_add(1);
                continue;
            }
            let bytes = if port.policy.vlan_id.is_some() && !port.policy.tagged {
                strip_vlan(bytes).unwrap_or_else(|| bytes.to_vec())
            } else {
                bytes.to_vec()
            };
            port.rx.push_back(QueuedFrame {
                generation: port.committed_generation.saturating_add(1),
                bytes,
            });
            delivered += 1;
        }
        delivered
    }

    pub fn receive(&mut self, port_id: u64) -> Result<Option<QueuedFrame>, SwitchError> {
        let port = self.ports.get_mut(&port_id).ok_or(SwitchError::NotFound)?;
        if port.delivered_rx.len() >= port.policy.rx_capacity {
            return Err(SwitchError::QueueFull);
        }
        let Some(frame) = port.rx.pop_front() else {
            return Ok(None);
        };
        port.delivered_rx.push_back(frame.clone());
        Ok(Some(frame))
    }

    /// Validate source identity and stage outbound bytes. Staged data does not
    /// reach the physical NIC until the corresponding backend journal commits.
    pub fn stage_egress(&mut self, port_id: u64, bytes: &[u8]) -> Result<(), SwitchError> {
        let header = FrameHeader::parse(bytes)?;
        let port = self.ports.get_mut(&port_id).ok_or(SwitchError::NotFound)?;
        if header.source != port.policy.source_mac
            || !egress_vlan_matches(&port.policy, header.vlan_id)
        {
            port.spoof_rejections = port.spoof_rejections.saturating_add(1);
            return Err(SwitchError::Spoofed);
        }
        if port.staged_tx.len() >= port.policy.tx_capacity {
            port.dropped_tx = port.dropped_tx.saturating_add(1);
            return Err(SwitchError::QueueFull);
        }
        let bytes = if let Some(vlan) = port.policy.vlan_id.filter(|_| !port.policy.tagged) {
            add_vlan(bytes, vlan)?
        } else {
            bytes.to_vec()
        };
        port.staged_tx.push_back(QueuedFrame {
            generation: port.committed_generation.saturating_add(1),
            bytes,
        });
        Ok(())
    }

    pub fn commit_generation(
        &mut self,
        port_id: u64,
        generation: u64,
    ) -> Result<Vec<QueuedFrame>, SwitchError> {
        let port = self.ports.get_mut(&port_id).ok_or(SwitchError::NotFound)?;
        if generation < port.committed_generation {
            return Err(SwitchError::StaleGeneration);
        }
        port.committed_generation = generation;
        port.delivered_rx
            .retain(|frame| frame.generation > generation);
        let mut committed = Vec::new();
        while port
            .staged_tx
            .front()
            .is_some_and(|frame| frame.generation <= generation)
        {
            committed.push(port.staged_tx.pop_front().expect("front exists"));
        }
        Ok(committed)
    }

    /// Recovery replays ingress after the last committed transaction and
    /// discards outbound frames that were never journal-committed.
    pub fn recover_port(&mut self, port_id: u64, committed: u64) -> Result<(), SwitchError> {
        let port = self.ports.get_mut(&port_id).ok_or(SwitchError::NotFound)?;
        if committed > port.committed_generation {
            return Err(SwitchError::StaleGeneration);
        }
        port.committed_generation = committed;
        let mut replay = port
            .delivered_rx
            .drain(..)
            .filter(|frame| frame.generation > committed)
            .collect::<VecDeque<_>>();
        replay.append(&mut port.rx);
        port.rx = replay;
        port.staged_tx.retain(|frame| frame.generation <= committed);
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct FrameHeader {
    destination: [u8; 6],
    source: [u8; 6],
    vlan_id: Option<u16>,
}

impl FrameHeader {
    fn parse(bytes: &[u8]) -> Result<Self, SwitchError> {
        if !(MIN_FRAME_SIZE..=MAX_FRAME_SIZE).contains(&bytes.len()) {
            return Err(SwitchError::InvalidFrame);
        }
        let destination = bytes[0..6].try_into().unwrap();
        let source = bytes[6..12].try_into().unwrap();
        let ether_type = u16::from_be_bytes([bytes[12], bytes[13]]);
        let vlan_id = if matches!(ether_type, 0x8100 | 0x88a8) {
            if bytes.len() < 18 {
                return Err(SwitchError::InvalidFrame);
            }
            let vlan = u16::from_be_bytes([bytes[14], bytes[15]]) & 0x0fff;
            if vlan == 0 || vlan > 4094 {
                return Err(SwitchError::InvalidVlan);
            }
            Some(vlan)
        } else {
            None
        };
        Ok(Self {
            destination,
            source,
            vlan_id,
        })
    }
}

fn ingress_vlan_matches(policy: &PortPolicy, frame_vlan: Option<u16>) -> bool {
    match (policy.vlan_id, policy.tagged, frame_vlan) {
        (None, false, None) => true,
        (Some(expected), true, Some(actual)) => expected == actual,
        (Some(expected), false, Some(actual)) => expected == actual,
        _ => false,
    }
}

fn egress_vlan_matches(policy: &PortPolicy, frame_vlan: Option<u16>) -> bool {
    match (policy.vlan_id, policy.tagged, frame_vlan) {
        (None, false, None) | (Some(_), false, None) => true,
        (Some(expected), true, Some(actual)) => expected == actual,
        _ => false,
    }
}

fn strip_vlan(bytes: &[u8]) -> Option<Vec<u8>> {
    if bytes.len() < 18 {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() - 4);
    out.extend_from_slice(&bytes[..12]);
    out.extend_from_slice(&bytes[16..]);
    Some(out)
}

fn add_vlan(bytes: &[u8], vlan: u16) -> Result<Vec<u8>, SwitchError> {
    if bytes.len().saturating_add(4) > MAX_FRAME_SIZE {
        return Err(SwitchError::InvalidFrame);
    }
    let mut out = Vec::with_capacity(bytes.len() + 4);
    out.extend_from_slice(&bytes[..12]);
    out.extend_from_slice(&0x8100u16.to_be_bytes());
    out.extend_from_slice(&vlan.to_be_bytes());
    out.extend_from_slice(&bytes[12..]);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(dst: [u8; 6], src: [u8; 6], vlan: Option<u16>) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&dst);
        bytes.extend_from_slice(&src);
        if let Some(vlan) = vlan {
            bytes.extend_from_slice(&0x8100u16.to_be_bytes());
            bytes.extend_from_slice(&vlan.to_be_bytes());
            bytes.extend_from_slice(&0x0800u16.to_be_bytes());
        } else {
            bytes.extend_from_slice(&0x0800u16.to_be_bytes());
        }
        bytes.resize(64, 0);
        bytes
    }

    fn policy(id: u64, mac: [u8; 6]) -> PortPolicy {
        PortPolicy {
            port_id: id,
            physical_interface: 9,
            source_mac: mac,
            vlan_id: None,
            tagged: false,
            rx_capacity: 2,
            tx_capacity: 2,
        }
    }

    #[test]
    fn unicast_and_broadcast_are_isolated_copies() {
        let mut switch = VirtualSwitch::default();
        let a = [2, 0, 0, 0, 0, 1];
        let b = [2, 0, 0, 0, 0, 2];
        switch.create_port(policy(1, a)).unwrap();
        switch.create_port(policy(2, b)).unwrap();
        assert_eq!(switch.ingress(9, &frame(a, b, None)), 1);
        assert_eq!(switch.ingress(9, &frame([0xff; 6], a, None)), 2);
        assert_eq!(switch.port(1).unwrap().rx.len(), 2);
        assert_eq!(switch.port(2).unwrap().rx.len(), 1);
    }

    #[test]
    fn rejects_spoofing_and_only_releases_committed_egress() {
        let mut switch = VirtualSwitch::default();
        let mac = [2, 0, 0, 0, 0, 1];
        switch.create_port(policy(1, mac)).unwrap();
        assert_eq!(
            switch.stage_egress(1, &frame([0xff; 6], [4; 6], None)),
            Err(SwitchError::Spoofed)
        );
        switch
            .stage_egress(1, &frame([0xff; 6], mac, None))
            .unwrap();
        assert!(switch.commit_generation(1, 0).unwrap().is_empty());
        assert_eq!(switch.commit_generation(1, 1).unwrap().len(), 1);
    }

    #[test]
    fn access_ports_translate_and_isolate_vlans() {
        let mut switch = VirtualSwitch::default();
        let mac = [2, 0, 0, 0, 0, 1];
        let mut access = policy(1, mac);
        access.vlan_id = Some(42);
        switch.create_port(access).unwrap();
        assert_eq!(
            switch.ingress(9, &frame(mac, [2, 0, 0, 0, 0, 2], Some(41))),
            0
        );
        assert_eq!(
            switch.ingress(9, &frame(mac, [2, 0, 0, 0, 0, 2], Some(42))),
            1
        );
        let received = switch.receive(1).unwrap().unwrap();
        assert_eq!(FrameHeader::parse(&received.bytes).unwrap().vlan_id, None);
        switch
            .stage_egress(1, &frame([0xff; 6], mac, None))
            .unwrap();
        let sent = switch.commit_generation(1, 1).unwrap();
        assert_eq!(
            FrameHeader::parse(&sent[0].bytes).unwrap().vlan_id,
            Some(42)
        );
    }

    #[test]
    fn recovery_replays_uncommitted_ingress_and_drops_uncommitted_egress() {
        let mut switch = VirtualSwitch::default();
        let mac = [2, 0, 0, 0, 0, 1];
        switch.create_port(policy(1, mac)).unwrap();
        assert_eq!(switch.ingress(9, &frame(mac, [2, 0, 0, 0, 0, 2], None)), 1);
        assert_eq!(switch.receive(1).unwrap().unwrap().generation, 1);
        switch
            .stage_egress(1, &frame([0xff; 6], mac, None))
            .unwrap();

        switch.recover_port(1, 0).unwrap();
        assert_eq!(switch.port(1).unwrap().staged_tx.len(), 0);
        assert_eq!(switch.receive(1).unwrap().unwrap().generation, 1);
    }
}
