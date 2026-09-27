use crate::{
    ConnectionState, InterfaceBackend, IpPrefix, MAX_FIREWALL_RULES, NetError, NetworkState, Rule,
    RuleAction, RuleMatch, SocketDomain, Verdict,
};
use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

const NLMSG_DONE: u16 = 3;
const NLM_F_MULTI: u16 = 2;
const NLM_F_DUMP: u16 = 0x300;
const RTM_NEWLINK: u16 = 16;
const RTM_DELLINK: u16 = 17;
const RTM_GETLINK: u16 = 18;
const RTM_NEWADDR: u16 = 20;
const RTM_DELADDR: u16 = 21;
const RTM_GETADDR: u16 = 22;
const RTM_NEWROUTE: u16 = 24;
const RTM_DELROUTE: u16 = 25;
const RTM_GETROUTE: u16 = 26;
const SOCK_DIAG_BY_FAMILY: u16 = 20;

impl NetworkState {
    pub fn netlink_send(&mut self, socket: u64, request: &[u8]) -> Result<usize, NetError> {
        let mut offset = 0usize;
        while offset < request.len() {
            if request.len() - offset < 16 {
                return Err(NetError::InvalidArgument);
            }
            let length =
                u32::from_ne_bytes(request[offset..offset + 4].try_into().unwrap()) as usize;
            if length < 16 || length > request.len() - offset {
                return Err(NetError::InvalidArgument);
            }
            let aligned = (length + 3) & !3;
            if aligned > request.len() - offset {
                return Err(NetError::InvalidArgument);
            }
            self.netlink_send_one(socket, &request[offset..offset + aligned])?;
            offset += aligned;
        }
        if offset == 0 {
            return Err(NetError::InvalidArgument);
        }
        Ok(request.len())
    }

    fn netlink_send_one(&mut self, socket: u64, request: &[u8]) -> Result<usize, NetError> {
        let state = self.sockets.get(&socket).ok_or(NetError::NotFound)?;
        if request.len() < 16 {
            return Err(NetError::InvalidArgument);
        }
        let length = u32::from_ne_bytes(request[..4].try_into().unwrap()) as usize;
        if length < 16 || length > request.len() {
            return Err(NetError::InvalidArgument);
        }
        let message_type = u16::from_ne_bytes(request[4..6].try_into().unwrap());
        let flags = u16::from_ne_bytes(request[6..8].try_into().unwrap());
        let sequence = u32::from_ne_bytes(request[8..12].try_into().unwrap());
        let namespace_id = state.namespace;
        let domain = state.domain;
        let net_admin = state.net_admin;
        let mut replies = Vec::new();
        match (domain, message_type) {
            (SocketDomain::RouteNetlink, RTM_GETLINK) if flags & NLM_F_DUMP != 0 => {
                let namespace = self
                    .namespaces
                    .get(&namespace_id)
                    .ok_or(NetError::NotFound)?;
                for interface in namespace.interfaces.values() {
                    replies.push(link_message(interface, sequence));
                }
            }
            (SocketDomain::RouteNetlink, RTM_GETADDR) if flags & NLM_F_DUMP != 0 => {
                let namespace = self
                    .namespaces
                    .get(&namespace_id)
                    .ok_or(NetError::NotFound)?;
                for interface in namespace.interfaces.values() {
                    for address in &interface.addresses {
                        replies.push(address_message(interface.id, address, sequence));
                    }
                }
            }
            (SocketDomain::RouteNetlink, RTM_GETROUTE) if flags & NLM_F_DUMP != 0 => {
                let namespace = self
                    .namespaces
                    .get(&namespace_id)
                    .ok_or(NetError::NotFound)?;
                for route in &namespace.routes {
                    replies.push(route_message(route, sequence));
                }
            }
            (SocketDomain::RouteNetlink, RTM_NEWLINK) if net_admin => {
                self.update_link(namespace_id, &request[16..length])?;
            }
            (SocketDomain::RouteNetlink, RTM_DELLINK) if net_admin => {
                self.delete_link(namespace_id, &request[16..length])?;
            }
            (SocketDomain::RouteNetlink, RTM_NEWADDR | RTM_DELADDR) if net_admin => {
                update_address(
                    self.namespaces
                        .get_mut(&namespace_id)
                        .ok_or(NetError::NotFound)?,
                    &request[16..length],
                    message_type == RTM_NEWADDR,
                )?;
            }
            (SocketDomain::RouteNetlink, RTM_NEWROUTE | RTM_DELROUTE) if net_admin => {
                update_route(
                    self.namespaces
                        .get_mut(&namespace_id)
                        .ok_or(NetError::NotFound)?,
                    &request[16..length],
                    message_type == RTM_NEWROUTE,
                )?;
            }
            (
                SocketDomain::RouteNetlink,
                RTM_NEWLINK | RTM_DELLINK | RTM_NEWADDR | RTM_DELADDR | RTM_NEWROUTE | RTM_DELROUTE,
            ) => {
                return Err(NetError::PermissionDenied);
            }
            (SocketDomain::SockDiagNetlink, SOCK_DIAG_BY_FAMILY) if flags & NLM_F_DUMP != 0 => {
                if length < 24 || !matches!(request[16], 2 | 10) {
                    return Err(NetError::InvalidArgument);
                }
                let family = request[16];
                let protocol = request[17];
                if !matches!(protocol, 6 | 17) {
                    return Err(NetError::Unsupported);
                }
                let states = u32::from_ne_bytes(request[20..24].try_into().unwrap());
                for socket in self.sockets.values().filter(|socket| {
                    socket.namespace == namespace_id
                        && matches!(socket.domain, SocketDomain::Inet4 | SocketDomain::Inet6)
                        && (socket.domain == SocketDomain::Inet4) == (family == 2)
                        && ((protocol == 6 && matches!(socket.kind, crate::SocketKind::Stream))
                            || (protocol == 17
                                && matches!(socket.kind, crate::SocketKind::Datagram)))
                }) {
                    let state = diagnostic_state(socket);
                    if states == u32::MAX || states & (1u32 << state) != 0 {
                        replies.push(diagnostic_message(socket, state, sequence));
                    }
                }
            }
            (SocketDomain::SockDiagNetlink, _) => return Err(NetError::Unsupported),
            (SocketDomain::NetfilterNetlink, _) if !net_admin => {
                return Err(NetError::PermissionDenied);
            }
            (SocketDomain::NetfilterNetlink, value) => {
                self.netfilter_message(socket, namespace_id, value, &request[16..length])?;
            }
            _ => return Err(NetError::Unsupported),
        }
        replies.push(message(NLMSG_DONE, 0, sequence, &[]));
        let state = self.sockets.get_mut(&socket).ok_or(NetError::NotFound)?;
        if state.queued_packets.len() + replies.len() > 256 {
            return Err(NetError::WouldBlock);
        }
        state.queued_packets.extend(replies);
        Ok(length)
    }

    pub fn netlink_recv(&mut self, socket: u64) -> Result<Vec<u8>, NetError> {
        let state = self.sockets.get_mut(&socket).ok_or(NetError::NotFound)?;
        if state.queued_packets.is_empty() {
            return Err(NetError::WouldBlock);
        }
        Ok(state.queued_packets.remove(0))
    }
}

impl NetworkState {
    fn netfilter_message(
        &mut self,
        socket: u64,
        namespace_id: u64,
        message_type: u16,
        body: &[u8],
    ) -> Result<(), NetError> {
        const NFNL_MSG_BATCH_BEGIN: u16 = 0x10;
        const NFNL_MSG_BATCH_END: u16 = 0x11;
        const NFT_MSG_NEWTABLE: u8 = 0;
        const NFT_MSG_DELTABLE: u8 = 2;
        const NFT_MSG_NEWCHAIN: u8 = 3;
        const NFT_MSG_DELCHAIN: u8 = 5;
        const NFT_MSG_NEWRULE: u8 = 6;
        const NFT_MSG_DELRULE: u8 = 8;

        if message_type == NFNL_MSG_BATCH_BEGIN {
            if !body.is_empty() && body.len() < 4 {
                return Err(NetError::InvalidArgument);
            }
            let current = self
                .namespaces
                .get(&namespace_id)
                .ok_or(NetError::NotFound)?
                .netfilter
                .clone();
            let state = self.sockets.get_mut(&socket).ok_or(NetError::NotFound)?;
            if state.netfilter_batch.is_some() {
                return Err(NetError::InvalidArgument);
            }
            state.netfilter_batch = Some(current);
            return Ok(());
        }
        if message_type == NFNL_MSG_BATCH_END {
            let staged = self
                .sockets
                .get_mut(&socket)
                .ok_or(NetError::NotFound)?
                .netfilter_batch
                .take()
                .ok_or(NetError::InvalidArgument)?;
            let current = &mut self
                .namespaces
                .get_mut(&namespace_id)
                .ok_or(NetError::NotFound)?
                .netfilter;
            current.replace_atomic(current.generation, staged.rules, staged.default_verdict)?;
            return Ok(());
        }
        if message_type >> 8 != 10 || body.len() < 4 {
            self.abort_netfilter_batch(socket);
            return Err(NetError::Unsupported);
        }
        let operation = message_type as u8;
        let family = body[0];
        if !matches!(family, 0 | 2 | 10) {
            self.abort_netfilter_batch(socket);
            return Err(NetError::Unsupported);
        }
        let attrs = match attributes(&body[4..]) {
            Ok(attrs) => attrs,
            Err(error) => {
                self.abort_netfilter_batch(socket);
                return Err(error);
            }
        };
        let interface_names = self
            .namespaces
            .get(&namespace_id)
            .ok_or(NetError::NotFound)?
            .interfaces
            .values()
            .map(|interface| (interface.name.clone(), interface.id))
            .collect::<BTreeMap<_, _>>();
        let mut standalone = None;
        let staged = if let Some(batch) = self
            .sockets
            .get_mut(&socket)
            .ok_or(NetError::NotFound)?
            .netfilter_batch
            .as_mut()
        {
            batch
        } else {
            standalone = Some(
                self.namespaces
                    .get(&namespace_id)
                    .ok_or(NetError::NotFound)?
                    .netfilter
                    .clone(),
            );
            standalone.as_mut().unwrap()
        };
        let result = match operation {
            NFT_MSG_NEWTABLE => Ok(()),
            NFT_MSG_DELTABLE => {
                staged.rules.clear();
                staged.default_verdict = Verdict::Accept;
                Ok(())
            }
            NFT_MSG_NEWCHAIN => update_nft_chain(staged, &attrs),
            NFT_MSG_DELCHAIN => {
                // This bounded implementation owns one flattened base chain.
                // Deleting it removes its rules atomically.
                staged.rules.clear();
                staged.default_verdict = Verdict::Accept;
                Ok(())
            }
            NFT_MSG_NEWRULE => {
                if staged.rules.len() >= MAX_FIREWALL_RULES {
                    Err(NetError::ResourceExhausted)
                } else {
                    parse_nft_rule(family, &attrs, &interface_names)
                        .map(|rule| staged.rules.push(rule))
                }
            }
            // Handle-based deletion needs stable Linux rule handles, which
            // this bounded flattened representation intentionally does not
            // invent. Fail the complete transaction instead of deleting the
            // wrong rule.
            NFT_MSG_DELRULE => Err(NetError::Unsupported),
            // GETTABLE/GETCHAIN/GETRULE/GETGEN are accepted as bounded
            // snapshots. NLMSG_DONE is sufficient for an empty query and the
            // rules remain inspectable through the namespace snapshot.
            1 | 4 | 7 | 16 | 17 => Ok(()),
            _ => Err(NetError::Unsupported),
        };
        if let Err(error) = result {
            self.abort_netfilter_batch(socket);
            return Err(error);
        }
        if let Some(staged) = standalone {
            let current = &mut self
                .namespaces
                .get_mut(&namespace_id)
                .ok_or(NetError::NotFound)?
                .netfilter;
            current.replace_atomic(current.generation, staged.rules, staged.default_verdict)?;
        }
        Ok(())
    }

    fn abort_netfilter_batch(&mut self, socket: u64) {
        if let Some(state) = self.sockets.get_mut(&socket) {
            state.netfilter_batch = None;
        }
    }

    fn delete_link(&mut self, namespace_id: u64, body: &[u8]) -> Result<(), NetError> {
        if body.len() < 16 {
            return Err(NetError::InvalidArgument);
        }
        let index = i32::from_ne_bytes(body[4..8].try_into().unwrap());
        if index <= 0 {
            return Err(NetError::InvalidArgument);
        }
        let interface = self
            .namespaces
            .get(&namespace_id)
            .and_then(|namespace| namespace.interfaces.get(&(index as u64)))
            .cloned()
            .ok_or(NetError::NotFound)?;
        if matches!(
            interface.backend,
            InterfaceBackend::Loopback
                | InterfaceBackend::Direct { .. }
                | InterfaceBackend::L2 { .. }
        ) {
            return Err(NetError::PermissionDenied);
        }
        let peer = match interface.backend {
            InterfaceBackend::Veth {
                peer_namespace,
                peer_interface,
            } => Some((peer_namespace, peer_interface)),
            _ => None,
        };
        let namespace = self
            .namespaces
            .get_mut(&namespace_id)
            .ok_or(NetError::NotFound)?;
        namespace.interfaces.remove(&(index as u64));
        namespace
            .routes
            .retain(|route| route.interface_id != index as u64);
        for other in namespace.interfaces.values_mut() {
            if other.bridge == Some(index as u64) {
                other.bridge = None;
            }
        }
        if let Some((peer_namespace, peer_interface)) = peer {
            if let Some(namespace) = self.namespaces.get_mut(&peer_namespace) {
                namespace.interfaces.remove(&peer_interface);
                namespace
                    .routes
                    .retain(|route| route.interface_id != peer_interface);
            }
        }
        Ok(())
    }

    fn update_link(&mut self, namespace_id: u64, body: &[u8]) -> Result<(), NetError> {
        if body.len() < 16 {
            return Err(NetError::InvalidArgument);
        }
        let index = i32::from_ne_bytes(body[4..8].try_into().unwrap());
        let flags = u32::from_ne_bytes(body[8..12].try_into().unwrap());
        let change = u32::from_ne_bytes(body[12..16].try_into().unwrap());
        let attrs = attributes(&body[16..])?;
        if index == 0 {
            return self.create_link(namespace_id, &attrs);
        }
        if index < 0 {
            return Err(NetError::InvalidArgument);
        }
        let name = attrs
            .iter()
            .find(|(kind, _)| *kind == 3)
            .map(|(_, value)| string_attribute(value))
            .transpose()?;
        let move_to = attrs
            .iter()
            .find(|(kind, _)| *kind == 28)
            .map(|(_, value)| self.namespace_from_fd(value))
            .transpose()?;
        if let Some(target) = move_to {
            self.move_interface_unchecked(index as u64, namespace_id, target, name)?;
            return Ok(());
        }
        let namespace = self
            .namespaces
            .get_mut(&namespace_id)
            .ok_or(NetError::NotFound)?;
        if let Some(name) = name {
            if namespace
                .interfaces
                .values()
                .any(|other| other.id != index as u64 && other.name == name)
            {
                return Err(NetError::AlreadyExists);
            }
        }
        let master = attrs
            .iter()
            .find(|(kind, _)| *kind == 10)
            .map(|(_, value)| {
                if value.len() != 4 {
                    return Err(NetError::InvalidArgument);
                }
                let bridge = u32::from_ne_bytes((*value).try_into().unwrap()) as u64;
                if bridge != 0
                    && (!matches!(
                        namespace.interfaces.get(&bridge).map(|item| &item.backend),
                        Some(InterfaceBackend::Bridge)
                    ) || bridge == index as u64)
                {
                    return Err(NetError::InvalidArgument);
                }
                Ok(bridge)
            })
            .transpose()?;
        let interface = namespace
            .interfaces
            .get_mut(&(index as u64))
            .ok_or(NetError::NotFound)?;
        if let Some(name) = name {
            interface.name = name.into();
        }
        if change & 1 != 0 {
            interface.up = flags & 1 != 0;
        }
        for (kind, value) in attrs {
            match kind {
                4 => {
                    if value.len() != 4 {
                        return Err(NetError::InvalidArgument);
                    }
                    let mtu = u32::from_ne_bytes(value.try_into().unwrap());
                    if mtu < 68 || mtu > interface.mtu {
                        return Err(NetError::InvalidArgument);
                    }
                    interface.mtu = mtu;
                }
                10 => interface.bridge = master.filter(|bridge| *bridge != 0),
                _ => {}
            }
        }
        Ok(())
    }

    fn create_link(&mut self, namespace_id: u64, attrs: &[(u16, &[u8])]) -> Result<(), NetError> {
        let name = attrs
            .iter()
            .find(|(kind, _)| *kind == 3)
            .map(|(_, value)| string_attribute(value))
            .transpose()?
            .ok_or(NetError::InvalidArgument)?;
        let link_info = attrs
            .iter()
            .find(|(kind, _)| *kind == 18)
            .ok_or(NetError::Unsupported)?;
        let nested = attributes(link_info.1)?;
        let kind = nested
            .iter()
            .find(|(kind, _)| *kind == 1)
            .map(|(_, value)| string_attribute(value))
            .transpose()?
            .ok_or(NetError::InvalidArgument)?;
        match kind {
            "bridge" => {
                self.create_bridge_unchecked(namespace_id, name)?;
                Ok(())
            }
            "veth" => {
                let data = nested
                    .iter()
                    .find(|(kind, _)| *kind == 2)
                    .ok_or(NetError::InvalidArgument)?;
                let peer = attributes(data.1)?
                    .into_iter()
                    .find(|(kind, _)| *kind == 1)
                    .ok_or(NetError::InvalidArgument)?;
                if peer.1.len() < 16 {
                    return Err(NetError::InvalidArgument);
                }
                let peer_attrs = attributes(&peer.1[16..])?;
                let peer_name = peer_attrs
                    .iter()
                    .find(|(kind, _)| *kind == 3)
                    .map(|(_, value)| string_attribute(value))
                    .transpose()?
                    .ok_or(NetError::InvalidArgument)?;
                let peer_namespace = peer_attrs
                    .iter()
                    .find(|(kind, _)| *kind == 28)
                    .map(|(_, value)| self.namespace_from_fd(value))
                    .transpose()?
                    .unwrap_or(namespace_id);
                self.create_veth_unchecked(namespace_id, name, peer_namespace, peer_name)?;
                Ok(())
            }
            _ => Err(NetError::Unsupported),
        }
    }

    fn namespace_from_fd(&self, value: &[u8]) -> Result<u64, NetError> {
        if value.len() != 4 {
            return Err(NetError::InvalidArgument);
        }
        let fd = u32::from_ne_bytes(value.try_into().unwrap()) as u64;
        self.namespace_fds
            .get(&fd)
            .copied()
            .ok_or(NetError::NotFound)
    }
}

fn string_attribute(bytes: &[u8]) -> Result<&str, NetError> {
    let bytes = bytes.strip_suffix(&[0]).unwrap_or(bytes);
    let value = std::str::from_utf8(bytes).map_err(|_| NetError::InvalidArgument)?;
    if value.is_empty() || value.len() > 15 || value.contains('/') {
        return Err(NetError::InvalidArgument);
    }
    Ok(value)
}

fn update_address(
    namespace: &mut crate::Namespace,
    body: &[u8],
    add: bool,
) -> Result<(), NetError> {
    if body.len() < 8 {
        return Err(NetError::InvalidArgument);
    }
    let family = body[0];
    let prefix_len = body[1];
    let interface_id = u32::from_ne_bytes(body[4..8].try_into().unwrap()) as u64;
    let address = attributes(&body[8..])?
        .into_iter()
        .find(|(kind, _)| matches!(kind, 1 | 2))
        .and_then(|(_, bytes)| decode_ip(family, bytes))
        .ok_or(NetError::InvalidArgument)?;
    let prefix = crate::IpPrefix::new(address, prefix_len)?;
    let interface = namespace
        .interfaces
        .get_mut(&interface_id)
        .ok_or(NetError::NotFound)?;
    if add {
        if interface.addresses.contains(&prefix) {
            return Err(NetError::AlreadyExists);
        }
        if interface.addresses.len() >= 16 {
            return Err(NetError::ResourceExhausted);
        }
        interface.addresses.push(prefix);
    } else {
        let index = interface
            .addresses
            .iter()
            .position(|candidate| candidate == &prefix)
            .ok_or(NetError::NotFound)?;
        interface.addresses.remove(index);
    }
    Ok(())
}

fn update_route(namespace: &mut crate::Namespace, body: &[u8], add: bool) -> Result<(), NetError> {
    if body.len() < 12 || !matches!(body[0], 2 | 10) {
        return Err(NetError::InvalidArgument);
    }
    let family = body[0];
    let prefix_len = body[1];
    let attrs = attributes(&body[12..])?;
    let unspecified = if family == 2 {
        "0.0.0.0".parse().unwrap()
    } else {
        "::".parse().unwrap()
    };
    let destination = attrs
        .iter()
        .find(|(kind, _)| *kind == 1)
        .and_then(|(_, value)| decode_ip(family, value))
        .unwrap_or(unspecified);
    let interface_id = attrs
        .iter()
        .find(|(kind, _)| *kind == 4)
        .filter(|(_, value)| value.len() == 4)
        .map(|(_, value)| u32::from_ne_bytes(value[..4].try_into().unwrap()) as u64)
        .ok_or(NetError::InvalidArgument)?;
    if !namespace.interfaces.contains_key(&interface_id) {
        return Err(NetError::NotFound);
    }
    let gateway = attrs
        .iter()
        .find(|(kind, _)| *kind == 5)
        .and_then(|(_, value)| decode_ip(family, value));
    let metric = attrs
        .iter()
        .find(|(kind, _)| *kind == 6)
        .filter(|(_, value)| value.len() == 4)
        .map_or(0, |(_, value)| {
            u32::from_ne_bytes(value[..4].try_into().unwrap())
        });
    let route = crate::Route {
        destination: crate::IpPrefix::new(destination, prefix_len)?,
        gateway,
        interface_id,
        metric,
    };
    if add {
        if namespace.routes.contains(&route) {
            return Err(NetError::AlreadyExists);
        }
        if namespace.routes.len() >= crate::MAX_ROUTES {
            return Err(NetError::ResourceExhausted);
        }
        namespace.routes.push(route);
    } else {
        let index = namespace
            .routes
            .iter()
            .position(|candidate| candidate == &route)
            .ok_or(NetError::NotFound)?;
        namespace.routes.remove(index);
    }
    Ok(())
}

fn attributes(mut bytes: &[u8]) -> Result<Vec<(u16, &[u8])>, NetError> {
    let mut out = Vec::new();
    while !bytes.is_empty() {
        if bytes.len() < 4 {
            return Err(NetError::InvalidArgument);
        }
        let length = u16::from_ne_bytes(bytes[..2].try_into().unwrap()) as usize;
        let kind = u16::from_ne_bytes(bytes[2..4].try_into().unwrap()) & 0x3fff;
        if length < 4 || length > bytes.len() {
            return Err(NetError::InvalidArgument);
        }
        out.push((kind, &bytes[4..length]));
        let aligned = (length + 3) & !3;
        if aligned > bytes.len() {
            return Err(NetError::InvalidArgument);
        }
        bytes = &bytes[aligned..];
    }
    Ok(out)
}

#[derive(Clone, Copy)]
enum NftSelector {
    Protocol,
    Source,
    Destination,
    SourcePort,
    DestinationPort,
    InputInterface,
    OutputInterface,
    InputName,
    OutputName,
    ConnectionState,
}

fn update_nft_chain(
    netfilter: &mut crate::Netfilter,
    attrs: &[(u16, &[u8])],
) -> Result<(), NetError> {
    // NFTA_CHAIN_POLICY. Linux uses NF_DROP=0 and NF_ACCEPT=1.
    if let Some((_, value)) = attrs.iter().find(|(kind, _)| *kind == 5) {
        netfilter.default_verdict = match be_u32(value)? {
            0 => Verdict::Drop,
            1 => Verdict::Accept,
            _ => return Err(NetError::Unsupported),
        };
    }
    Ok(())
}

fn parse_nft_rule(
    family: u8,
    attrs: &[(u16, &[u8])],
    interface_names: &BTreeMap<String, u64>,
) -> Result<Rule, NetError> {
    let expressions = attrs
        .iter()
        .find(|(kind, _)| *kind == 4)
        .ok_or(NetError::InvalidArgument)?;
    let mut registers = BTreeMap::<u32, NftSelector>::new();
    let mut values = BTreeMap::<u32, Vec<u8>>::new();
    let mut matcher = RuleMatch {
        input_interface: None,
        output_interface: None,
        source: None,
        destination: None,
        protocol: None,
        source_ports: None,
        destination_ports: None,
        states: Vec::new(),
    };
    let mut action = None;
    let mut packets = 0;
    let mut bytes = 0;
    for (kind, expression) in attributes(expressions.1)? {
        if kind != 1 {
            return Err(NetError::Unsupported);
        }
        let expression = attributes(expression)?;
        let name = expression
            .iter()
            .find(|(kind, _)| *kind == 1)
            .map(|(_, value)| nul_string(value))
            .transpose()?
            .ok_or(NetError::InvalidArgument)?;
        let data = expression
            .iter()
            .find(|(kind, _)| *kind == 2)
            .map_or(&[][..], |(_, value)| *value);
        match name {
            "payload" => {
                let attrs = attributes(data)?;
                let register = nft_u32(&attrs, 1)?;
                let base = nft_u32(&attrs, 2)?;
                let offset = nft_u32(&attrs, 3)?;
                let length = nft_u32(&attrs, 4)?;
                let selector = match (family, base, offset, length) {
                    (2, 1, 9, 1) | (10, 1, 6, 1) => NftSelector::Protocol,
                    (2, 1, 12, 4) | (10, 1, 8, 16) => NftSelector::Source,
                    (2, 1, 16, 4) | (10, 1, 24, 16) => NftSelector::Destination,
                    (_, 2, 0, 2) => NftSelector::SourcePort,
                    (_, 2, 2, 2) => NftSelector::DestinationPort,
                    _ => return Err(NetError::Unsupported),
                };
                registers.insert(register, selector);
            }
            "meta" => {
                let attrs = attributes(data)?;
                let key = nft_u32(&attrs, 1)?;
                let register = nft_u32(&attrs, 2)?;
                let selector = match key {
                    4 => NftSelector::InputInterface,
                    5 => NftSelector::OutputInterface,
                    6 => NftSelector::InputName,
                    7 => NftSelector::OutputName,
                    16 => NftSelector::Protocol,
                    _ => return Err(NetError::Unsupported),
                };
                registers.insert(register, selector);
            }
            "ct" => {
                let attrs = attributes(data)?;
                if nft_u32(&attrs, 1)? != 0 {
                    return Err(NetError::Unsupported);
                }
                registers.insert(nft_u32(&attrs, 2)?, NftSelector::ConnectionState);
            }
            "cmp" => {
                let attrs = attributes(data)?;
                let register = nft_u32(&attrs, 1)?;
                if nft_u32(&attrs, 2)? != 0 {
                    return Err(NetError::Unsupported);
                }
                let value = nested_data(&attrs, 3)?;
                apply_nft_comparison(
                    &mut matcher,
                    *registers.get(&register).ok_or(NetError::InvalidArgument)?,
                    value,
                    family,
                    interface_names,
                )?;
            }
            "immediate" => {
                let attrs = attributes(data)?;
                let register = nft_u32(&attrs, 1)?;
                let value = nested_data(&attrs, 2)?.to_vec();
                if register == 0 {
                    action = Some(RuleAction::Verdict(match be_i32(&value)? {
                        0 => Verdict::Drop,
                        1 => Verdict::Accept,
                        // NFT_RETURN/CONTINUE are meaningful only with chain
                        // control that the flattened namespace table cannot
                        // represent safely.
                        _ => return Err(NetError::Unsupported),
                    }));
                } else {
                    values.insert(register, value);
                }
            }
            "counter" => {
                let attrs = attributes(data)?;
                bytes = attrs
                    .iter()
                    .find(|(kind, _)| *kind == 1)
                    .map(|(_, value)| be_u64(value))
                    .transpose()?
                    .unwrap_or(0);
                packets = attrs
                    .iter()
                    .find(|(kind, _)| *kind == 2)
                    .map(|(_, value)| be_u64(value))
                    .transpose()?
                    .unwrap_or(0);
            }
            "reject" => action = Some(RuleAction::Verdict(Verdict::Reject)),
            "masq" => action = Some(RuleAction::Masquerade),
            "nat" => {
                let attrs = attributes(data)?;
                let kind = nft_u32(&attrs, 1)?;
                let address_register = nft_u32(&attrs, 3)?;
                let address = decode_nft_ip(
                    family,
                    values
                        .get(&address_register)
                        .ok_or(NetError::InvalidArgument)?,
                )?;
                let port = attrs
                    .iter()
                    .find(|(attr, _)| *attr == 5)
                    .map(|(_, register)| {
                        let register = be_u32(register)?;
                        let value = values.get(&register).ok_or(NetError::InvalidArgument)?;
                        if value.len() != 2 {
                            return Err(NetError::InvalidArgument);
                        }
                        Ok(u16::from_be_bytes(value.as_slice().try_into().unwrap()))
                    })
                    .transpose()?;
                action = Some(match kind {
                    0 => RuleAction::Snat { address, port },
                    1 => RuleAction::Dnat { address, port },
                    _ => return Err(NetError::Unsupported),
                });
            }
            // Lookups, bytecode, quotas, logging, and specialist extensions
            // are deliberately rejected so a ruleset never partially applies.
            _ => return Err(NetError::Unsupported),
        }
    }
    Ok(Rule {
        matcher,
        action: action.ok_or(NetError::InvalidArgument)?,
        packets,
        bytes,
    })
}

fn apply_nft_comparison(
    matcher: &mut RuleMatch,
    selector: NftSelector,
    value: &[u8],
    family: u8,
    interface_names: &BTreeMap<String, u64>,
) -> Result<(), NetError> {
    match selector {
        NftSelector::Protocol => {
            if value.len() != 1 {
                return Err(NetError::InvalidArgument);
            }
            matcher.protocol = Some(value[0]);
        }
        NftSelector::Source | NftSelector::Destination => {
            let address = decode_nft_ip(family, value)?;
            let prefix = IpPrefix::new(address, if address.is_ipv4() { 32 } else { 128 })?;
            if matches!(selector, NftSelector::Source) {
                matcher.source = Some(prefix);
            } else {
                matcher.destination = Some(prefix);
            }
        }
        NftSelector::SourcePort | NftSelector::DestinationPort => {
            if value.len() != 2 {
                return Err(NetError::InvalidArgument);
            }
            let port = u16::from_be_bytes(value.try_into().unwrap());
            if matches!(selector, NftSelector::SourcePort) {
                matcher.source_ports = Some((port, port));
            } else {
                matcher.destination_ports = Some((port, port));
            }
        }
        NftSelector::InputInterface | NftSelector::OutputInterface => {
            let interface = u64::from(be_u32(value)?);
            if matches!(selector, NftSelector::InputInterface) {
                matcher.input_interface = Some(interface);
            } else {
                matcher.output_interface = Some(interface);
            }
        }
        NftSelector::InputName | NftSelector::OutputName => {
            let interface = *interface_names
                .get(nul_string(value)?)
                .ok_or(NetError::NotFound)?;
            if matches!(selector, NftSelector::InputName) {
                matcher.input_interface = Some(interface);
            } else {
                matcher.output_interface = Some(interface);
            }
        }
        NftSelector::ConnectionState => {
            let mask = be_u32(value)?;
            for (bit, state) in [
                (1, ConnectionState::Invalid),
                (2, ConnectionState::Established),
                (4, ConnectionState::Related),
                (8, ConnectionState::New),
            ] {
                if mask & bit != 0 {
                    matcher.states.push(state);
                }
            }
            if matcher.states.is_empty() || mask & !0xf != 0 {
                return Err(NetError::Unsupported);
            }
        }
    }
    Ok(())
}

fn nft_u32(attrs: &[(u16, &[u8])], kind: u16) -> Result<u32, NetError> {
    attrs
        .iter()
        .find(|(candidate, _)| *candidate == kind)
        .map(|(_, value)| be_u32(value))
        .transpose()?
        .ok_or(NetError::InvalidArgument)
}

fn nested_data<'a>(attrs: &[(u16, &'a [u8])], kind: u16) -> Result<&'a [u8], NetError> {
    let value = attrs
        .iter()
        .find(|(candidate, _)| *candidate == kind)
        .ok_or(NetError::InvalidArgument)?
        .1;
    attributes(value)?
        .into_iter()
        .find(|(candidate, _)| *candidate == 1)
        .map(|(_, value)| value)
        .ok_or(NetError::InvalidArgument)
}

fn decode_nft_ip(family: u8, value: &[u8]) -> Result<IpAddr, NetError> {
    match (family, value.len()) {
        (2, 4) | (0, 4) => Ok(IpAddr::V4(Ipv4Addr::new(
            value[0], value[1], value[2], value[3],
        ))),
        (10, 16) | (0, 16) => Ok(IpAddr::V6(Ipv6Addr::from(
            <[u8; 16]>::try_from(value).unwrap(),
        ))),
        _ => Err(NetError::InvalidArgument),
    }
}

fn be_u32(value: &[u8]) -> Result<u32, NetError> {
    if value.len() != 4 {
        return Err(NetError::InvalidArgument);
    }
    Ok(u32::from_be_bytes(value.try_into().unwrap()))
}

fn be_i32(value: &[u8]) -> Result<i32, NetError> {
    Ok(be_u32(value)? as i32)
}

fn be_u64(value: &[u8]) -> Result<u64, NetError> {
    if value.len() != 8 {
        return Err(NetError::InvalidArgument);
    }
    Ok(u64::from_be_bytes(value.try_into().unwrap()))
}

fn nul_string(value: &[u8]) -> Result<&str, NetError> {
    let value = value.strip_suffix(&[0]).unwrap_or(value);
    if value.is_empty() {
        return Err(NetError::InvalidArgument);
    }
    std::str::from_utf8(value).map_err(|_| NetError::InvalidArgument)
}

fn decode_ip(family: u8, bytes: &[u8]) -> Option<std::net::IpAddr> {
    match (family, bytes.len()) {
        (2, 4) => Some(std::net::Ipv4Addr::new(bytes[0], bytes[1], bytes[2], bytes[3]).into()),
        (10, 16) => Some(std::net::Ipv6Addr::from(<[u8; 16]>::try_from(bytes).ok()?).into()),
        _ => None,
    }
}

fn link_message(interface: &crate::Interface, sequence: u32) -> Vec<u8> {
    let mut body = vec![0; 16];
    body[0] = 0;
    body[2..4].copy_from_slice(&1u16.to_ne_bytes());
    body[4..8].copy_from_slice(&(interface.id as i32).to_ne_bytes());
    let mut flags = if interface.up { 0x1 | 0x40 } else { 0 };
    if matches!(interface.backend, InterfaceBackend::Loopback) {
        flags |= 0x8;
    }
    body[8..12].copy_from_slice(&(flags as u32).to_ne_bytes());
    body[12..16].copy_from_slice(&u32::MAX.to_ne_bytes());
    attribute(&mut body, 3, &[interface.name.as_bytes(), &[0]].concat());
    attribute(&mut body, 4, &interface.mtu.to_ne_bytes());
    if let InterfaceBackend::L2 { mac, .. } = interface.backend {
        attribute(&mut body, 1, &mac);
    }
    message(RTM_NEWLINK, NLM_F_MULTI, sequence, &body)
}

fn address_message(interface: u64, address: &crate::IpPrefix, sequence: u32) -> Vec<u8> {
    let octets = match address.address {
        std::net::IpAddr::V4(value) => value.octets().to_vec(),
        std::net::IpAddr::V6(value) => value.octets().to_vec(),
    };
    let mut body = vec![0; 8];
    body[0] = if address.address.is_ipv4() { 2 } else { 10 };
    body[1] = address.prefix_len;
    body[4..8].copy_from_slice(&(interface as u32).to_ne_bytes());
    attribute(&mut body, 1, &octets);
    attribute(&mut body, 2, &octets);
    message(RTM_NEWADDR, NLM_F_MULTI, sequence, &body)
}

fn route_message(route: &crate::Route, sequence: u32) -> Vec<u8> {
    let mut body = vec![0; 12];
    body[0] = if route.destination.address.is_ipv4() {
        2
    } else {
        10
    };
    body[1] = route.destination.prefix_len;
    body[4] = 254;
    body[7] = 1;
    body[8..12].copy_from_slice(&1u32.to_ne_bytes());
    let destination = match route.destination.address {
        std::net::IpAddr::V4(value) => value.octets().to_vec(),
        std::net::IpAddr::V6(value) => value.octets().to_vec(),
    };
    if route.destination.prefix_len != 0 {
        attribute(&mut body, 1, &destination);
    }
    attribute(&mut body, 4, &(route.interface_id as u32).to_ne_bytes());
    if let Some(gateway) = route.gateway {
        let gateway = match gateway {
            std::net::IpAddr::V4(value) => value.octets().to_vec(),
            std::net::IpAddr::V6(value) => value.octets().to_vec(),
        };
        attribute(&mut body, 5, &gateway);
    }
    attribute(&mut body, 6, &route.metric.to_ne_bytes());
    message(RTM_NEWROUTE, NLM_F_MULTI, sequence, &body)
}

fn diagnostic_state(socket: &crate::SocketState) -> u8 {
    if socket.listening {
        10 // TCP_LISTEN
    } else if socket.peer.is_some() {
        1 // TCP_ESTABLISHED; also the conventional connected UDP state
    } else {
        7 // TCP_CLOSE / unconnected datagram
    }
}

fn diagnostic_message(socket: &crate::SocketState, state: u8, sequence: u32) -> Vec<u8> {
    // Linux inet_diag_msg followed by no attributes. Address slots are always
    // 16 bytes each; IPv4 occupies their first four bytes.
    let family = if matches!(socket.domain, SocketDomain::Inet4) {
        2
    } else {
        10
    };
    let unspecified = if family == 2 {
        "0.0.0.0".parse().unwrap()
    } else {
        "::".parse().unwrap()
    };
    let local = socket.local.unwrap_or((unspecified, 0));
    let peer = socket.peer.unwrap_or((unspecified, 0));
    let mut body = vec![0; 72];
    body[0] = family;
    body[1] = state;
    body[4..6].copy_from_slice(&local.1.to_be_bytes());
    body[6..8].copy_from_slice(&peer.1.to_be_bytes());
    encode_diag_address(&mut body[8..24], local.0);
    encode_diag_address(&mut body[24..40], peer.0);
    body[44..48].copy_from_slice(&(socket.id as u32).to_ne_bytes());
    body[48..52].copy_from_slice(&((socket.id >> 32) as u32).to_ne_bytes());
    body[68..72].copy_from_slice(&(socket.id as u32).to_ne_bytes());
    message(SOCK_DIAG_BY_FAMILY, NLM_F_MULTI, sequence, &body)
}

fn encode_diag_address(out: &mut [u8], address: std::net::IpAddr) {
    match address {
        std::net::IpAddr::V4(address) => out[..4].copy_from_slice(&address.octets()),
        std::net::IpAddr::V6(address) => out.copy_from_slice(&address.octets()),
    }
}

fn message(message_type: u16, flags: u16, sequence: u32, body: &[u8]) -> Vec<u8> {
    let mut out = vec![0; 16];
    out[..4].copy_from_slice(&((16 + body.len()) as u32).to_ne_bytes());
    out[4..6].copy_from_slice(&message_type.to_ne_bytes());
    out[6..8].copy_from_slice(&flags.to_ne_bytes());
    out[8..12].copy_from_slice(&sequence.to_ne_bytes());
    out.extend_from_slice(body);
    while out.len() & 3 != 0 {
        out.push(0);
    }
    out
}

fn attribute(out: &mut Vec<u8>, kind: u16, value: &[u8]) {
    let length = 4 + value.len();
    out.extend_from_slice(&(length as u16).to_ne_bytes());
    out.extend_from_slice(&kind.to_ne_bytes());
    out.extend_from_slice(value);
    while out.len() & 3 != 0 {
        out.push(0);
    }
}
