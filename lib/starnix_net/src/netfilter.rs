use crate::{IpPrefix, NetError, NetworkState};
use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr};

pub const MAX_FIREWALL_RULES: usize = 512;
pub const MAX_CONNTRACK: usize = 4096;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Verdict {
    Accept,
    Drop,
    Reject,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectionState {
    New,
    Established,
    Related,
    Invalid,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Packet {
    pub input_interface: Option<u64>,
    pub output_interface: Option<u64>,
    pub source: IpAddr,
    pub destination: IpAddr,
    pub protocol: u8,
    pub source_port: u16,
    pub destination_port: u16,
    pub state: ConnectionState,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuleMatch {
    pub input_interface: Option<u64>,
    pub output_interface: Option<u64>,
    pub source: Option<IpPrefix>,
    pub destination: Option<IpPrefix>,
    pub protocol: Option<u8>,
    pub source_ports: Option<(u16, u16)>,
    pub destination_ports: Option<(u16, u16)>,
    pub states: Vec<ConnectionState>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RuleAction {
    Verdict(Verdict),
    Snat { address: IpAddr, port: Option<u16> },
    Dnat { address: IpAddr, port: Option<u16> },
    Masquerade,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Rule {
    pub matcher: RuleMatch,
    pub action: RuleAction,
    pub packets: u64,
    pub bytes: u64,
}

impl Rule {
    fn matches(&self, packet: &Packet) -> bool {
        self.matcher
            .input_interface
            .is_none_or(|value| Some(value) == packet.input_interface)
            && self
                .matcher
                .output_interface
                .is_none_or(|value| Some(value) == packet.output_interface)
            && self
                .matcher
                .source
                .as_ref()
                .is_none_or(|prefix| prefix.contains(packet.source))
            && self
                .matcher
                .destination
                .as_ref()
                .is_none_or(|prefix| prefix.contains(packet.destination))
            && self
                .matcher
                .protocol
                .is_none_or(|value| value == packet.protocol)
            && self
                .matcher
                .source_ports
                .is_none_or(|(start, end)| (start..=end).contains(&packet.source_port))
            && self
                .matcher
                .destination_ports
                .is_none_or(|(start, end)| (start..=end).contains(&packet.destination_port))
            && (self.matcher.states.is_empty() || self.matcher.states.contains(&packet.state))
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct Flow {
    pub source: IpAddr,
    pub destination: IpAddr,
    pub protocol: u8,
    pub source_port: u16,
    pub destination_port: u16,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Translation {
    pub source: IpAddr,
    pub destination: IpAddr,
    pub source_port: u16,
    pub destination_port: u16,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Netfilter {
    pub generation: u64,
    pub rules: Vec<Rule>,
    pub conntrack: BTreeMap<Flow, Translation>,
    pub default_verdict: Verdict,
}

impl Default for Netfilter {
    fn default() -> Self {
        Self {
            generation: 0,
            rules: Vec::new(),
            conntrack: BTreeMap::new(),
            default_verdict: Verdict::Accept,
        }
    }
}

impl Netfilter {
    pub fn replace_atomic(
        &mut self,
        expected_generation: u64,
        rules: Vec<Rule>,
        default_verdict: Verdict,
    ) -> Result<u64, NetError> {
        if expected_generation != self.generation
            || rules.len() > MAX_FIREWALL_RULES
            || rules.iter().any(|rule| {
                rule.matcher
                    .source_ports
                    .is_some_and(|(start, end)| start > end)
                    || rule
                        .matcher
                        .destination_ports
                        .is_some_and(|(start, end)| start > end)
                    || rule.matcher.states.len() > 4
            })
        {
            return Err(NetError::InvalidArgument);
        }
        self.rules = rules;
        self.default_verdict = default_verdict;
        self.generation = self
            .generation
            .checked_add(1)
            .ok_or(NetError::ResourceExhausted)?;
        Ok(self.generation)
    }

    pub fn evaluate(&mut self, packet: &mut Packet, bytes: usize) -> Verdict {
        let flow = Flow {
            source: packet.source,
            destination: packet.destination,
            protocol: packet.protocol,
            source_port: packet.source_port,
            destination_port: packet.destination_port,
        };
        if let Some(translation) = self.conntrack.get(&flow) {
            packet.source = translation.source;
            packet.destination = translation.destination;
            packet.source_port = translation.source_port;
            packet.destination_port = translation.destination_port;
            packet.state = ConnectionState::Established;
        }
        let original = Translation {
            source: packet.source,
            destination: packet.destination,
            source_port: packet.source_port,
            destination_port: packet.destination_port,
        };
        let mut terminal_verdict = None;
        for rule in &mut self.rules {
            if !rule.matches(packet) {
                continue;
            }
            rule.packets = rule.packets.saturating_add(1);
            rule.bytes = rule.bytes.saturating_add(bytes as u64);
            match rule.action {
                RuleAction::Verdict(verdict) => {
                    terminal_verdict = Some(verdict);
                    break;
                }
                RuleAction::Snat { address, port } => {
                    packet.source = address;
                    packet.source_port = port.unwrap_or(packet.source_port);
                }
                RuleAction::Dnat { address, port } => {
                    packet.destination = address;
                    packet.destination_port = port.unwrap_or(packet.destination_port);
                }
                // The namespace selects its egress address before evaluation, so
                // masquerade is the identity source translation recorded in
                // conntrack. The backend may change the physical address, but
                // that policy is deliberately outside the guest namespace.
                RuleAction::Masquerade => {}
            }
        }
        let verdict = terminal_verdict.unwrap_or(self.default_verdict);
        if verdict == Verdict::Accept {
            self.remember_translation(flow, packet, &original);
        }
        verdict
    }

    fn remember_translation(&mut self, flow: Flow, packet: &Packet, original: &Translation) {
        let translated = Translation {
            source: packet.source,
            destination: packet.destination,
            source_port: packet.source_port,
            destination_port: packet.destination_port,
        };
        if &translated == original {
            return;
        }
        let reverse_flow = Flow {
            source: translated.destination,
            destination: translated.source,
            protocol: flow.protocol,
            source_port: translated.destination_port,
            destination_port: translated.source_port,
        };
        let reverse_translation = Translation {
            source: original.destination,
            destination: original.source,
            source_port: original.destination_port,
            destination_port: original.source_port,
        };
        self.insert_conntrack(flow, translated);
        self.insert_conntrack(reverse_flow, reverse_translation);
    }

    fn insert_conntrack(&mut self, flow: Flow, translation: Translation) {
        if self.conntrack.len() >= MAX_CONNTRACK && !self.conntrack.contains_key(&flow) {
            if let Some(oldest) = self.conntrack.keys().next().cloned() {
                self.conntrack.remove(&oldest);
            }
        }
        self.conntrack.insert(flow, translation);
    }
}

impl NetworkState {
    /// Translate the bounded legacy iptables `IPT_SO_SET_REPLACE` payload into
    /// the same namespace-local rules used by nftables. Unsupported matches or
    /// targets reject the complete replacement with no state change.
    pub fn replace_iptables_ipv4(&mut self, socket: u64, bytes: &[u8]) -> Result<(), NetError> {
        let state = self.sockets.get(&socket).ok_or(NetError::NotFound)?;
        if !state.net_admin {
            return Err(NetError::PermissionDenied);
        }
        let namespace_id = state.namespace;
        let interfaces = self
            .namespaces
            .get(&namespace_id)
            .ok_or(NetError::NotFound)?
            .interfaces
            .values()
            .map(|interface| (interface.name.clone(), interface.id))
            .collect::<BTreeMap<_, _>>();
        let (rules, default_verdict) = parse_iptables_replace(bytes, &interfaces)?;
        let netfilter = &mut self
            .namespaces
            .get_mut(&namespace_id)
            .ok_or(NetError::NotFound)?
            .netfilter;
        netfilter.replace_atomic(netfilter.generation, rules, default_verdict)?;
        Ok(())
    }
}

fn parse_iptables_replace(
    bytes: &[u8],
    interfaces: &BTreeMap<String, u64>,
) -> Result<(Vec<Rule>, Verdict), NetError> {
    const HEADER: usize = 96;
    const ENTRY: usize = 112;
    if bytes.len() < HEADER {
        return Err(NetError::InvalidArgument);
    }
    let name = c_name(&bytes[..32])?;
    if !matches!(name, "filter" | "nat") {
        return Err(NetError::Unsupported);
    }
    let entries = native_u32(bytes, 36)? as usize;
    let size = native_u32(bytes, 40)? as usize;
    if entries > MAX_FIREWALL_RULES || size > 1024 * 1024 || HEADER + size != bytes.len() {
        return Err(NetError::InvalidArgument);
    }
    let mut rules = Vec::with_capacity(entries);
    let mut cursor = HEADER;
    while cursor < bytes.len() {
        if bytes.len() - cursor < ENTRY {
            return Err(NetError::InvalidArgument);
        }
        let target_offset = usize::from(native_u16(bytes, cursor + 88)?);
        let next_offset = usize::from(native_u16(bytes, cursor + 90)?);
        if target_offset < ENTRY
            || next_offset < target_offset + 32
            || cursor + next_offset > bytes.len()
            || next_offset & 7 != 0
        {
            return Err(NetError::InvalidArgument);
        }
        let entry = &bytes[cursor..cursor + next_offset];
        let mut matcher = RuleMatch {
            input_interface: interface_match(entry, 16, 48, interfaces)?,
            output_interface: interface_match(entry, 32, 64, interfaces)?,
            source: masked_ipv4(&entry[0..4], &entry[8..12])?,
            destination: masked_ipv4(&entry[4..8], &entry[12..16])?,
            protocol: None,
            source_ports: None,
            destination_ports: None,
            states: Vec::new(),
        };
        let protocol = u16::from_ne_bytes(entry[80..82].try_into().unwrap());
        let flags = entry[82];
        let inverse = entry[83];
        if flags != 0 || inverse != 0 {
            return Err(NetError::Unsupported);
        }
        if protocol != 0 {
            matcher.protocol = Some(u8::try_from(protocol).map_err(|_| NetError::Unsupported)?);
        }
        let mut match_cursor = ENTRY;
        while match_cursor < target_offset {
            if target_offset - match_cursor < 32 {
                return Err(NetError::InvalidArgument);
            }
            let length = usize::from(native_u16(entry, match_cursor)?);
            if length < 32 || match_cursor + length > target_offset || length & 7 != 0 {
                return Err(NetError::InvalidArgument);
            }
            let extension = c_name(&entry[match_cursor + 2..match_cursor + 31])?;
            let data = &entry[match_cursor + 32..match_cursor + length];
            match extension {
                "tcp" | "udp" => {
                    if data.len() < 8
                        || data
                            .get(8..)
                            .is_some_and(|tail| tail.iter().any(|x| *x != 0))
                    {
                        return Err(NetError::Unsupported);
                    }
                    matcher.source_ports = Some((
                        u16::from_ne_bytes(data[0..2].try_into().unwrap()),
                        u16::from_ne_bytes(data[2..4].try_into().unwrap()),
                    ));
                    matcher.destination_ports = Some((
                        u16::from_ne_bytes(data[4..6].try_into().unwrap()),
                        u16::from_ne_bytes(data[6..8].try_into().unwrap()),
                    ));
                    let required_protocol = if extension == "tcp" { 6 } else { 17 };
                    if matcher
                        .protocol
                        .is_some_and(|value| value != required_protocol)
                    {
                        return Err(NetError::InvalidArgument);
                    }
                    matcher.protocol = Some(required_protocol);
                }
                "conntrack" | "state" => {
                    // libxt_conntrack revisions vary. The first u16 is the
                    // supported state mask for the common state-only form.
                    if data.len() < 2 || data[2..].iter().any(|byte| *byte != 0) {
                        return Err(NetError::Unsupported);
                    }
                    let states = u16::from_ne_bytes(data[..2].try_into().unwrap());
                    for (bit, state) in [
                        (1, ConnectionState::Invalid),
                        (2, ConnectionState::Established),
                        (4, ConnectionState::Related),
                        (8, ConnectionState::New),
                    ] {
                        if states & bit != 0 {
                            matcher.states.push(state);
                        }
                    }
                    if matcher.states.is_empty() || states & !0xf != 0 {
                        return Err(NetError::Unsupported);
                    }
                }
                _ => return Err(NetError::Unsupported),
            }
            match_cursor += length;
        }
        let target_size = usize::from(native_u16(entry, target_offset)?);
        if target_size < 32 || target_offset + target_size > next_offset {
            return Err(NetError::InvalidArgument);
        }
        let target_name = c_name(&entry[target_offset + 2..target_offset + 31])?;
        let target_data = &entry[target_offset + 32..target_offset + target_size];
        let action = match target_name {
            "" => {
                if target_data.len() < 4 {
                    return Err(NetError::InvalidArgument);
                }
                match i32::from_ne_bytes(target_data[..4].try_into().unwrap()) {
                    -2 => RuleAction::Verdict(Verdict::Accept),
                    -1 => RuleAction::Verdict(Verdict::Drop),
                    _ => return Err(NetError::Unsupported),
                }
            }
            "REJECT" => RuleAction::Verdict(Verdict::Reject),
            "MASQUERADE" => RuleAction::Masquerade,
            "SNAT" | "DNAT" => {
                // Revision-zero IPv4 NAT target: one bounded address/port
                // range. Multi-range and randomized extensions fail closed.
                if target_data.len() < 20 || native_u32(target_data, 0)? != 1 {
                    return Err(NetError::Unsupported);
                }
                let flags = native_u32(target_data, 4)?;
                if flags & !0x3 != 0 || target_data[8..12] != target_data[12..16] {
                    return Err(NetError::Unsupported);
                }
                let address = IpAddr::V4(Ipv4Addr::new(
                    target_data[8],
                    target_data[9],
                    target_data[10],
                    target_data[11],
                ));
                let minimum = u16::from_be_bytes(target_data[16..18].try_into().unwrap());
                let maximum = u16::from_be_bytes(target_data[18..20].try_into().unwrap());
                if minimum != maximum {
                    return Err(NetError::Unsupported);
                }
                let port = (flags & 0x2 != 0).then_some(minimum);
                if target_name == "SNAT" {
                    RuleAction::Snat { address, port }
                } else {
                    RuleAction::Dnat { address, port }
                }
            }
            _ => return Err(NetError::Unsupported),
        };
        rules.push(Rule {
            matcher,
            action,
            packets: 0,
            bytes: 0,
        });
        cursor += next_offset;
    }
    if cursor != bytes.len() || rules.len() != entries {
        return Err(NetError::InvalidArgument);
    }
    Ok((rules, Verdict::Accept))
}

fn masked_ipv4(address: &[u8], mask: &[u8]) -> Result<Option<IpPrefix>, NetError> {
    let mask = u32::from_be_bytes(mask.try_into().unwrap());
    if mask == 0 {
        return Ok(None);
    }
    let prefix = mask.leading_ones() as u8;
    if mask != u32::MAX.checked_shl(u32::from(32 - prefix)).unwrap_or(0) {
        return Err(NetError::Unsupported);
    }
    Ok(Some(IpPrefix::new(
        IpAddr::V4(Ipv4Addr::new(
            address[0], address[1], address[2], address[3],
        )),
        prefix,
    )?))
}

fn interface_match(
    entry: &[u8],
    name_offset: usize,
    mask_offset: usize,
    interfaces: &BTreeMap<String, u64>,
) -> Result<Option<u64>, NetError> {
    let name = c_name(&entry[name_offset..name_offset + 16])?;
    let mask = &entry[mask_offset..mask_offset + 16];
    if name.is_empty() && mask.iter().all(|byte| *byte == 0) {
        return Ok(None);
    }
    let required = name.len() + 1;
    if mask[..required].iter().any(|byte| *byte != 0xff)
        || mask[required..].iter().any(|byte| *byte != 0)
    {
        return Err(NetError::Unsupported);
    }
    interfaces
        .get(name)
        .copied()
        .map(Some)
        .ok_or(NetError::NotFound)
}

fn c_name(bytes: &[u8]) -> Result<&str, NetError> {
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    std::str::from_utf8(&bytes[..end]).map_err(|_| NetError::InvalidArgument)
}

fn native_u16(bytes: &[u8], offset: usize) -> Result<u16, NetError> {
    bytes
        .get(offset..offset + 2)
        .ok_or(NetError::InvalidArgument)
        .map(|value| u16::from_ne_bytes(value.try_into().unwrap()))
}

fn native_u32(bytes: &[u8], offset: usize) -> Result<u32, NetError> {
    bytes
        .get(offset..offset + 4)
        .ok_or(NetError::InvalidArgument)
        .map(|value| u32::from_ne_bytes(value.try_into().unwrap()))
}
