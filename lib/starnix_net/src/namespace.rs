use crate::{Capabilities, NetError, Netfilter};
use std::collections::{BTreeMap, BTreeSet};
use std::net::IpAddr;

pub const MAX_NAMESPACES: usize = 64;
pub const MAX_INTERFACES: usize = 64;
pub const MAX_ROUTES: usize = 256;
pub const MAX_SOCKETS: usize = 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IpPrefix {
    pub address: IpAddr,
    pub prefix_len: u8,
}

impl IpPrefix {
    pub fn new(address: IpAddr, prefix_len: u8) -> Result<Self, NetError> {
        if prefix_len > if address.is_ipv4() { 32 } else { 128 } {
            return Err(NetError::InvalidArgument);
        }
        Ok(Self {
            address,
            prefix_len,
        })
    }

    pub fn contains(&self, address: IpAddr) -> bool {
        match (self.address, address) {
            (IpAddr::V4(network), IpAddr::V4(address)) => {
                let bits = self.prefix_len;
                let mask = if bits == 0 {
                    0
                } else {
                    u32::MAX << (32 - bits)
                };
                u32::from(network) & mask == u32::from(address) & mask
            }
            (IpAddr::V6(network), IpAddr::V6(address)) => {
                let bits = self.prefix_len;
                let mask = if bits == 0 {
                    0
                } else {
                    u128::MAX << (128 - bits)
                };
                u128::from(network) & mask == u128::from(address) & mask
            }
            _ => false,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostPolicy {
    pub profile: String,
    pub isolation_group: String,
    pub domain: String,
    pub physical_selector: String,
    pub vlan_id: u16,
    pub allow_raw: bool,
    pub allow_promiscuous: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InterfaceBackend {
    Loopback,
    Direct {
        provider: u64,
        lease: u64,
        table_id: u32,
        host_interface_id: u64,
        policy: HostPolicy,
    },
    L2 {
        device: u64,
        lease: u64,
        mac: [u8; 6],
        policy: HostPolicy,
    },
    Veth {
        peer_namespace: u64,
        peer_interface: u64,
    },
    Bridge,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Interface {
    pub id: u64,
    pub name: String,
    pub mtu: u32,
    pub up: bool,
    pub promiscuous: bool,
    pub addresses: Vec<IpPrefix>,
    pub bridge: Option<u64>,
    pub backend: InterfaceBackend,
}

impl Interface {
    pub(crate) fn validate(&self) -> Result<(), NetError> {
        if self.id == 0
            || self.name.is_empty()
            || self.name.len() > 15
            || self.mtu < 68
            || self.mtu > 65_536
            || self.addresses.len() > 16
            || self
                .name
                .bytes()
                .any(|byte| !(byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-')))
        {
            return Err(NetError::InvalidArgument);
        }
        if self.promiscuous
            && matches!(
                &self.backend,
                InterfaceBackend::Direct { .. }
                    | InterfaceBackend::L2 {
                        policy: HostPolicy {
                            allow_promiscuous: false,
                            ..
                        },
                        ..
                    }
            )
        {
            return Err(NetError::PermissionDenied);
        }
        Ok(())
    }

    pub fn raw_allowed(&self) -> bool {
        match &self.backend {
            InterfaceBackend::Direct { .. } => false,
            InterfaceBackend::L2 { policy, .. } => policy.allow_raw,
            InterfaceBackend::Loopback
            | InterfaceBackend::Veth { .. }
            | InterfaceBackend::Bridge => true,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Route {
    pub destination: IpPrefix,
    pub gateway: Option<IpAddr>,
    pub interface_id: u64,
    pub metric: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SocketDomain {
    Inet4,
    Inet6,
    Packet,
    RouteNetlink,
    NetfilterNetlink,
    SockDiagNetlink,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SocketKind {
    Stream,
    Datagram,
    Raw,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SocketState {
    pub id: u64,
    /// Runner-owned descriptor identities. The upper 32 bits identify the
    /// Linux process descriptor table and the lower 32 bits are the Linux fd.
    pub linux_fds: Vec<u64>,
    pub namespace: u64,
    pub domain: SocketDomain,
    pub kind: SocketKind,
    pub protocol: u16,
    pub net_admin: bool,
    pub net_raw: bool,
    pub bound_interface: Option<u64>,
    pub local: Option<(IpAddr, u16)>,
    pub peer: Option<(IpAddr, u16)>,
    pub nonblocking: bool,
    pub listening: bool,
    pub shutdown_read: bool,
    pub shutdown_write: bool,
    /// Whether recvmsg should publish destination/interface packet metadata.
    pub receive_packet_info: bool,
    /// Namespace-local multicast groups joined by this socket. Interface
    /// selection remains constrained by `bound_interface` and the namespace
    /// FIB; membership can never alter the host profile.
    pub multicast_groups: Vec<IpAddr>,
    /// Staged nftables transaction between NFNL batch begin/end messages.
    pub netfilter_batch: Option<Netfilter>,
    pub backend_control: u64,
    pub queued_packets: Vec<Vec<u8>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Namespace {
    pub id: u64,
    pub interfaces: BTreeMap<u64, Interface>,
    pub routes: Vec<Route>,
    pub dns_servers: Vec<IpAddr>,
    pub netfilter: Netfilter,
    pub owner_references: u32,
    pub fd_references: u32,
}

impl Namespace {
    fn loopback(id: u64) -> Self {
        let mut interfaces = BTreeMap::new();
        interfaces.insert(
            1,
            Interface {
                id: 1,
                name: "lo".into(),
                mtu: 65_536,
                up: true,
                promiscuous: false,
                addresses: vec![
                    IpPrefix::new("127.0.0.1".parse().unwrap(), 8).unwrap(),
                    IpPrefix::new("::1".parse().unwrap(), 128).unwrap(),
                ],
                bridge: None,
                backend: InterfaceBackend::Loopback,
            },
        );
        Self {
            id,
            interfaces,
            routes: vec![
                Route {
                    destination: IpPrefix::new("127.0.0.0".parse().unwrap(), 8).unwrap(),
                    gateway: None,
                    interface_id: 1,
                    metric: 0,
                },
                Route {
                    destination: IpPrefix::new("::1".parse().unwrap(), 128).unwrap(),
                    gateway: None,
                    interface_id: 1,
                    metric: 0,
                },
            ],
            dns_servers: Vec::new(),
            netfilter: Netfilter::default(),
            owner_references: 1,
            fd_references: 0,
        }
    }

    pub fn route(&self, destination: IpAddr, bound_interface: Option<u64>) -> Option<&Route> {
        self.routes
            .iter()
            .filter(|route| {
                bound_interface.is_none_or(|id| id == route.interface_id)
                    && route.destination.contains(destination)
                    && self
                        .interfaces
                        .get(&route.interface_id)
                        .is_some_and(|interface| interface.up)
            })
            .min_by_key(|route| (u8::MAX - route.destination.prefix_len, route.metric))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NetworkState {
    pub namespaces: BTreeMap<u64, Namespace>,
    pub sockets: BTreeMap<u64, SocketState>,
    pub namespace_fds: BTreeMap<u64, u64>,
    pub task_namespaces: BTreeMap<u32, u64>,
    pub task_capabilities: BTreeMap<u32, Capabilities>,
    pub(crate) next_namespace: u64,
    pub(crate) next_interface: u64,
    pub(crate) next_socket: u64,
    pub(crate) next_namespace_fd: u64,
}

impl NetworkState {
    pub fn offline(uid: u32) -> Self {
        let mut state = Self {
            namespaces: BTreeMap::new(),
            sockets: BTreeMap::new(),
            namespace_fds: BTreeMap::new(),
            task_namespaces: BTreeMap::new(),
            task_capabilities: BTreeMap::new(),
            next_namespace: 2,
            next_interface: 2,
            next_socket: 1,
            next_namespace_fd: 1,
        };
        state.namespaces.insert(1, Namespace::loopback(1));
        state.task_namespaces.insert(1, 1);
        state.task_capabilities.insert(
            1,
            if uid == 0 {
                Capabilities::root_network_defaults()
            } else {
                Capabilities::default()
            },
        );
        state
    }

    pub fn with_initial_interfaces(
        uid: u32,
        interfaces: Vec<(Interface, Vec<Route>)>,
    ) -> Result<Self, NetError> {
        let mut state = Self::offline(uid);
        for (interface, mut routes) in interfaces {
            let id = state.attach(1, interface)?;
            for route in &mut routes {
                route.interface_id = id;
                if route
                    .gateway
                    .is_some_and(|gateway| gateway.is_ipv4() != route.destination.address.is_ipv4())
                {
                    return Err(NetError::InvalidArgument);
                }
            }
            let namespace = state.namespaces.get_mut(&1).ok_or(NetError::NotFound)?;
            if namespace.routes.len() + routes.len() > MAX_ROUTES {
                return Err(NetError::ResourceExhausted);
            }
            namespace.routes.append(&mut routes);
        }
        Ok(state)
    }

    pub fn namespace_for_task(&self, tid: u32) -> Result<u64, NetError> {
        self.task_namespaces
            .get(&tid)
            .copied()
            .ok_or(NetError::NotFound)
    }

    pub fn create_namespace(&mut self, tid: u32) -> Result<u64, NetError> {
        self.require(tid, crate::CAP_SYS_ADMIN)?;
        if self.namespaces.len() >= MAX_NAMESPACES {
            return Err(NetError::ResourceExhausted);
        }
        let id = self.next_namespace;
        self.next_namespace = self
            .next_namespace
            .checked_add(1)
            .ok_or(NetError::ResourceExhausted)?;
        self.namespaces.insert(id, Namespace::loopback(id));
        if let Some(old) = self.task_namespaces.insert(tid, id) {
            self.release_owner(old);
        }
        Ok(id)
    }

    pub fn clone_task(
        &mut self,
        parent: u32,
        child: u32,
        new_network: bool,
    ) -> Result<(), NetError> {
        if self.task_namespaces.contains_key(&child) {
            return Err(NetError::AlreadyExists);
        }
        let namespace = if new_network {
            self.require(parent, crate::CAP_SYS_ADMIN)?;
            if self.namespaces.len() >= MAX_NAMESPACES {
                return Err(NetError::ResourceExhausted);
            }
            let id = self.next_namespace;
            self.next_namespace += 1;
            self.namespaces.insert(id, Namespace::loopback(id));
            id
        } else {
            let id = self.namespace_for_task(parent)?;
            self.namespaces
                .get_mut(&id)
                .ok_or(NetError::NotFound)?
                .owner_references += 1;
            id
        };
        self.task_namespaces.insert(child, namespace);
        self.task_capabilities.insert(
            child,
            self.task_capabilities
                .get(&parent)
                .copied()
                .unwrap_or_default(),
        );
        Ok(())
    }

    pub fn exit_task(&mut self, tid: u32) {
        if let Some(namespace) = self.task_namespaces.remove(&tid) {
            self.release_owner(namespace);
        }
        self.task_capabilities.remove(&tid);
        self.collect_namespaces();
    }

    pub fn open_namespace(&mut self, namespace: u64) -> Result<u64, NetError> {
        let value = self
            .namespaces
            .get_mut(&namespace)
            .ok_or(NetError::NotFound)?;
        value.fd_references += 1;
        let fd = self.next_namespace_fd;
        self.next_namespace_fd += 1;
        self.namespace_fds.insert(fd, namespace);
        Ok(fd)
    }

    pub fn bind_namespace_fd(&mut self, fd: u64, namespace: u64) -> Result<(), NetError> {
        if self.namespace_fds.contains_key(&fd) {
            return Err(NetError::AlreadyExists);
        }
        self.namespaces
            .get_mut(&namespace)
            .ok_or(NetError::NotFound)?
            .fd_references += 1;
        self.namespace_fds.insert(fd, namespace);
        Ok(())
    }

    pub fn clone_namespace_fd(&mut self, source: u64, target: u64) -> Result<(), NetError> {
        let namespace = *self.namespace_fds.get(&source).ok_or(NetError::NotFound)?;
        if self.namespace_fds.contains_key(&target) {
            self.close_namespace_fd(target)?;
        }
        self.bind_namespace_fd(target, namespace)
    }

    pub fn close_namespace_fd(&mut self, fd: u64) -> Result<(), NetError> {
        let namespace = self.namespace_fds.remove(&fd).ok_or(NetError::NotFound)?;
        self.namespaces
            .get_mut(&namespace)
            .ok_or(NetError::NotFound)?
            .fd_references -= 1;
        self.collect_namespaces();
        Ok(())
    }

    pub fn setns(&mut self, tid: u32, fd: u64) -> Result<(), NetError> {
        self.require(tid, crate::CAP_SYS_ADMIN)?;
        let target = *self.namespace_fds.get(&fd).ok_or(NetError::NotFound)?;
        self.namespaces
            .get_mut(&target)
            .ok_or(NetError::NotFound)?
            .owner_references += 1;
        if let Some(old) = self.task_namespaces.insert(tid, target) {
            self.release_owner(old);
        }
        self.collect_namespaces();
        Ok(())
    }

    pub fn attach(&mut self, namespace: u64, mut interface: Interface) -> Result<u64, NetError> {
        interface.validate()?;
        let target = self
            .namespaces
            .get_mut(&namespace)
            .ok_or(NetError::NotFound)?;
        if target.interfaces.len() >= MAX_INTERFACES
            || target
                .interfaces
                .values()
                .any(|existing| existing.name == interface.name)
        {
            return Err(NetError::ResourceExhausted);
        }
        let id = self.next_interface;
        self.next_interface += 1;
        interface.id = id;
        target.interfaces.insert(id, interface);
        Ok(id)
    }

    pub fn move_interface(
        &mut self,
        tid: u32,
        interface: u64,
        from: u64,
        to: u64,
        new_name: Option<&str>,
    ) -> Result<(), NetError> {
        self.require(tid, crate::CAP_NET_ADMIN)?;
        self.move_interface_unchecked(interface, from, to, new_name)
    }

    pub(crate) fn move_interface_unchecked(
        &mut self,
        interface: u64,
        from: u64,
        to: u64,
        new_name: Option<&str>,
    ) -> Result<(), NetError> {
        if from == to {
            return Err(NetError::InvalidArgument);
        }
        let mut value = self
            .namespaces
            .get_mut(&from)
            .and_then(|namespace| namespace.interfaces.remove(&interface))
            .ok_or(NetError::NotFound)?;
        if matches!(value.backend, InterfaceBackend::Loopback) {
            self.namespaces
                .get_mut(&from)
                .unwrap()
                .interfaces
                .insert(interface, value);
            return Err(NetError::PermissionDenied);
        }
        if let Some(name) = new_name {
            value.name = name.into();
        }
        value.validate()?;
        let target = self.namespaces.get_mut(&to).ok_or(NetError::NotFound)?;
        if target
            .interfaces
            .values()
            .any(|other| other.name == value.name)
        {
            self.namespaces
                .get_mut(&from)
                .unwrap()
                .interfaces
                .insert(interface, value);
            return Err(NetError::AlreadyExists);
        }
        target.interfaces.insert(interface, value);
        Ok(())
    }

    pub fn add_route(&mut self, tid: u32, namespace: u64, route: Route) -> Result<(), NetError> {
        self.require(tid, crate::CAP_NET_ADMIN)?;
        let target = self
            .namespaces
            .get_mut(&namespace)
            .ok_or(NetError::NotFound)?;
        if target.routes.len() >= MAX_ROUTES
            || !target.interfaces.contains_key(&route.interface_id)
            || route
                .gateway
                .is_some_and(|gateway| gateway.is_ipv4() != route.destination.address.is_ipv4())
        {
            return Err(NetError::InvalidArgument);
        }
        if target.routes.contains(&route) {
            return Err(NetError::AlreadyExists);
        }
        target.routes.push(route);
        Ok(())
    }

    pub fn create_veth(
        &mut self,
        tid: u32,
        left_namespace: u64,
        left_name: &str,
        right_namespace: u64,
        right_name: &str,
    ) -> Result<(u64, u64), NetError> {
        self.require(tid, crate::CAP_NET_ADMIN)?;
        self.create_veth_unchecked(left_namespace, left_name, right_namespace, right_name)
    }

    pub(crate) fn create_veth_unchecked(
        &mut self,
        left_namespace: u64,
        left_name: &str,
        right_namespace: u64,
        right_name: &str,
    ) -> Result<(u64, u64), NetError> {
        let left_id = self.next_interface;
        let right_id = left_id + 1;
        let left = Interface {
            id: left_id,
            name: left_name.into(),
            mtu: 1500,
            up: false,
            promiscuous: false,
            addresses: Vec::new(),
            bridge: None,
            backend: InterfaceBackend::Veth {
                peer_namespace: right_namespace,
                peer_interface: right_id,
            },
        };
        let right = Interface {
            id: right_id,
            name: right_name.into(),
            backend: InterfaceBackend::Veth {
                peer_namespace: left_namespace,
                peer_interface: left_id,
            },
            ..left.clone()
        };
        left.validate()?;
        right.validate()?;
        if left_namespace == right_namespace {
            let namespace = self
                .namespaces
                .get_mut(&left_namespace)
                .ok_or(NetError::NotFound)?;
            if namespace.interfaces.len() + 2 > MAX_INTERFACES
                || namespace
                    .interfaces
                    .values()
                    .any(|item| item.name == left_name || item.name == right_name)
            {
                return Err(NetError::ResourceExhausted);
            }
            namespace.interfaces.insert(left_id, left);
            namespace.interfaces.insert(right_id, right);
        } else {
            for (namespace_id, interface) in [(left_namespace, &left), (right_namespace, &right)] {
                let namespace = self
                    .namespaces
                    .get(&namespace_id)
                    .ok_or(NetError::NotFound)?;
                if namespace.interfaces.len() >= MAX_INTERFACES
                    || namespace
                        .interfaces
                        .values()
                        .any(|item| item.name == interface.name)
                {
                    return Err(NetError::ResourceExhausted);
                }
            }
            self.namespaces
                .get_mut(&left_namespace)
                .unwrap()
                .interfaces
                .insert(left_id, left);
            self.namespaces
                .get_mut(&right_namespace)
                .unwrap()
                .interfaces
                .insert(right_id, right);
        }
        self.next_interface += 2;
        Ok((left_id, right_id))
    }

    pub fn create_bridge(&mut self, tid: u32, namespace: u64, name: &str) -> Result<u64, NetError> {
        self.require(tid, crate::CAP_NET_ADMIN)?;
        self.create_bridge_unchecked(namespace, name)
    }

    pub(crate) fn create_bridge_unchecked(
        &mut self,
        namespace: u64,
        name: &str,
    ) -> Result<u64, NetError> {
        self.attach(
            namespace,
            Interface {
                id: 1,
                name: name.into(),
                mtu: 1500,
                up: false,
                promiscuous: false,
                addresses: Vec::new(),
                bridge: None,
                backend: InterfaceBackend::Bridge,
            },
        )
    }

    pub fn enslave(
        &mut self,
        tid: u32,
        namespace: u64,
        interface: u64,
        bridge: u64,
    ) -> Result<(), NetError> {
        self.require(tid, crate::CAP_NET_ADMIN)?;
        self.enslave_unchecked(namespace, interface, bridge)
    }

    pub(crate) fn enslave_unchecked(
        &mut self,
        namespace: u64,
        interface: u64,
        bridge: u64,
    ) -> Result<(), NetError> {
        let target = self
            .namespaces
            .get_mut(&namespace)
            .ok_or(NetError::NotFound)?;
        if !matches!(
            target.interfaces.get(&bridge).map(|item| &item.backend),
            Some(InterfaceBackend::Bridge)
        ) || interface == bridge
        {
            return Err(NetError::InvalidArgument);
        }
        target
            .interfaces
            .get_mut(&interface)
            .ok_or(NetError::NotFound)?
            .bridge = Some(bridge);
        Ok(())
    }

    pub fn socket(
        &mut self,
        tid: u32,
        domain: SocketDomain,
        kind: SocketKind,
        protocol: u16,
        nonblocking: bool,
    ) -> Result<u64, NetError> {
        if self.sockets.len() >= MAX_SOCKETS {
            return Err(NetError::ResourceExhausted);
        }
        let raw = matches!(domain, SocketDomain::Packet)
            || (matches!(kind, SocketKind::Raw) || matches!(protocol, 1 | 58))
                && matches!(domain, SocketDomain::Inet4 | SocketDomain::Inet6);
        if raw {
            self.require(tid, crate::CAP_NET_RAW)?;
            if !self
                .namespaces
                .values()
                .flat_map(|namespace| namespace.interfaces.values())
                .any(|interface| {
                    matches!(
                        &interface.backend,
                        InterfaceBackend::L2 { policy, .. } if policy.allow_raw
                    )
                })
            {
                return Err(NetError::PermissionDenied);
            }
        }
        let namespace = self.namespace_for_task(tid)?;
        let capabilities = self
            .task_capabilities
            .get(&tid)
            .copied()
            .unwrap_or_default();
        let id = self.next_socket;
        self.next_socket += 1;
        self.sockets.insert(
            id,
            SocketState {
                id,
                linux_fds: Vec::new(),
                namespace,
                domain,
                kind,
                protocol,
                net_admin: capabilities.has(crate::CAP_NET_ADMIN),
                net_raw: capabilities.has(crate::CAP_NET_RAW),
                bound_interface: None,
                local: None,
                peer: None,
                nonblocking,
                listening: false,
                shutdown_read: false,
                shutdown_write: false,
                receive_packet_info: false,
                multicast_groups: Vec::new(),
                netfilter_batch: None,
                backend_control: 0,
                queued_packets: Vec::new(),
            },
        );
        Ok(id)
    }

    pub fn bind_interface(&mut self, socket: u64, name: &str) -> Result<u64, NetError> {
        let value = self.sockets.get_mut(&socket).ok_or(NetError::NotFound)?;
        let namespace = self
            .namespaces
            .get(&value.namespace)
            .ok_or(NetError::NotFound)?;
        let interface = namespace
            .interfaces
            .values()
            .find(|interface| interface.name == name)
            .ok_or(NetError::NotFound)?;
        if (matches!(value.kind, SocketKind::Raw) || matches!(value.domain, SocketDomain::Packet))
            && !interface.raw_allowed()
        {
            return Err(NetError::PermissionDenied);
        }
        value.bound_interface = Some(interface.id);
        Ok(interface.id)
    }

    pub fn set_receive_packet_info(&mut self, socket: u64, enabled: bool) -> Result<(), NetError> {
        self.sockets
            .get_mut(&socket)
            .ok_or(NetError::NotFound)?
            .receive_packet_info = enabled;
        Ok(())
    }

    pub fn update_multicast_membership(
        &mut self,
        socket: u64,
        group: IpAddr,
        interface: Option<u64>,
        join: bool,
    ) -> Result<(), NetError> {
        if !group.is_multicast() {
            return Err(NetError::InvalidArgument);
        }
        let value = self.sockets.get(&socket).ok_or(NetError::NotFound)?;
        if group.is_ipv4() != matches!(value.domain, SocketDomain::Inet4) {
            return Err(NetError::InvalidArgument);
        }
        let namespace = self
            .namespaces
            .get(&value.namespace)
            .ok_or(NetError::NotFound)?;
        if let Some(interface) = interface {
            if !namespace.interfaces.contains_key(&interface)
                || value
                    .bound_interface
                    .is_some_and(|bound| bound != interface)
            {
                return Err(NetError::NetworkUnreachable);
            }
        }
        let value = self.sockets.get_mut(&socket).unwrap();
        if join {
            if value.multicast_groups.contains(&group) {
                return Err(NetError::AlreadyExists);
            }
            if value.multicast_groups.len() >= 64 {
                return Err(NetError::ResourceExhausted);
            }
            value.multicast_groups.push(group);
        } else {
            let index = value
                .multicast_groups
                .iter()
                .position(|candidate| *candidate == group)
                .ok_or(NetError::NotFound)?;
            value.multicast_groups.remove(index);
        }
        Ok(())
    }

    pub fn packet_info(&self, socket: u64) -> Result<(u64, IpAddr), NetError> {
        let value = self.sockets.get(&socket).ok_or(NetError::NotFound)?;
        let namespace = self
            .namespaces
            .get(&value.namespace)
            .ok_or(NetError::NotFound)?;
        let interface = value
            .bound_interface
            .or_else(|| {
                value.local.and_then(|local| {
                    namespace.interfaces.values().find_map(|interface| {
                        interface
                            .addresses
                            .iter()
                            .any(|address| address.address == local.0)
                            .then_some(interface.id)
                    })
                })
            })
            .unwrap_or(1);
        let address = value.local.map_or_else(
            || {
                if matches!(value.domain, SocketDomain::Inet4) {
                    IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED)
                } else {
                    IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED)
                }
            },
            |local| local.0,
        );
        Ok((interface, address))
    }

    pub fn bind_fd(&mut self, socket: u64, fd: u64) -> Result<(), NetError> {
        if self
            .sockets
            .values()
            .any(|other| other.linux_fds.contains(&fd))
        {
            return Err(NetError::AlreadyExists);
        }
        self.sockets
            .get_mut(&socket)
            .ok_or(NetError::NotFound)?
            .linux_fds
            .push(fd);
        Ok(())
    }

    pub fn socket_by_fd(&self, fd: u64) -> Option<u64> {
        self.sockets
            .values()
            .find(|socket| socket.linux_fds.contains(&fd))
            .map(|socket| socket.id)
    }

    pub fn close_fd(&mut self, fd: u64) {
        if let Some(id) = self.socket_by_fd(fd) {
            let socket = self.sockets.get_mut(&id).unwrap();
            socket.linux_fds.retain(|other| *other != fd);
            if socket.linux_fds.is_empty() {
                self.sockets.remove(&id);
            }
        }
        self.collect_namespaces();
    }

    pub fn clone_fd(&mut self, source: u64, target: u64) -> Result<(), NetError> {
        let socket = self.socket_by_fd(source).ok_or(NetError::NotFound)?;
        self.close_fd(target);
        self.bind_fd(socket, target)
    }

    pub fn clone_process_fds(&mut self, parent: u32, child: u32) -> Result<(), NetError> {
        let mut additions = Vec::new();
        for (socket, state) in &self.sockets {
            for descriptor in &state.linux_fds {
                if (*descriptor >> 32) as u32 == parent {
                    additions.push((
                        *socket,
                        (u64::from(child) << 32) | (*descriptor as u32 as u64),
                    ));
                }
            }
        }
        for (socket, descriptor) in additions {
            self.bind_fd(socket, descriptor)?;
        }
        let namespace_fds = self
            .namespace_fds
            .iter()
            .filter_map(|(descriptor, namespace)| {
                ((*descriptor >> 32) as u32 == parent).then_some((
                    (u64::from(child) << 32) | (*descriptor as u32 as u64),
                    *namespace,
                ))
            })
            .collect::<Vec<_>>();
        for (descriptor, namespace) in namespace_fds {
            self.bind_namespace_fd(descriptor, namespace)?;
        }
        Ok(())
    }

    /// Removes every descriptor reference owned by a Linux process and
    /// returns backend controls whose final descriptor was removed.
    pub fn close_process_fds(&mut self, owner: u32) -> Vec<u64> {
        let mut controls = Vec::new();
        let socket_ids = self.sockets.keys().copied().collect::<Vec<_>>();
        for socket_id in socket_ids {
            let remove = self.sockets.get(&socket_id).is_some_and(|socket| {
                socket
                    .linux_fds
                    .iter()
                    .any(|descriptor| (*descriptor >> 32) as u32 == owner)
            });
            if !remove {
                continue;
            }
            let socket = self.sockets.get_mut(&socket_id).unwrap();
            socket
                .linux_fds
                .retain(|descriptor| (*descriptor >> 32) as u32 != owner);
            if socket.linux_fds.is_empty() {
                if socket.backend_control != 0 {
                    controls.push(socket.backend_control);
                }
                self.sockets.remove(&socket_id);
            }
        }
        let namespace_fds = self
            .namespace_fds
            .keys()
            .copied()
            .filter(|descriptor| (*descriptor >> 32) as u32 == owner)
            .collect::<Vec<_>>();
        for descriptor in namespace_fds {
            let _ = self.close_namespace_fd(descriptor);
        }
        self.collect_namespaces();
        controls
    }

    pub fn selected_interface(&self, socket: u64, destination: IpAddr) -> Result<u64, NetError> {
        let value = self.sockets.get(&socket).ok_or(NetError::NotFound)?;
        self.namespaces
            .get(&value.namespace)
            .and_then(|namespace| namespace.route(destination, value.bound_interface))
            .map(|route| route.interface_id)
            .ok_or(NetError::NetworkUnreachable)
    }

    pub fn filter_egress(
        &mut self,
        socket: u64,
        destination: (IpAddr, u16),
        protocol: u8,
        source_port: u16,
    ) -> Result<((IpAddr, u16), crate::Verdict), NetError> {
        let value = self.sockets.get(&socket).ok_or(NetError::NotFound)?;
        let namespace_id = value.namespace;
        let interface_id = self
            .namespaces
            .get(&namespace_id)
            .and_then(|namespace| namespace.route(destination.0, value.bound_interface))
            .map(|route| route.interface_id)
            .ok_or(NetError::NetworkUnreachable)?;
        let namespace = self
            .namespaces
            .get_mut(&namespace_id)
            .ok_or(NetError::NotFound)?;
        let direct = matches!(
            namespace
                .interfaces
                .get(&interface_id)
                .map(|interface| &interface.backend),
            Some(InterfaceBackend::Direct { .. })
        );
        let source = namespace
            .interfaces
            .get(&interface_id)
            .and_then(|interface| {
                interface
                    .addresses
                    .iter()
                    .find(|address| address.address.is_ipv4() == destination.0.is_ipv4())
            })
            .map_or_else(
                || {
                    if destination.0.is_ipv4() {
                        "0.0.0.0".parse().unwrap()
                    } else {
                        "::".parse().unwrap()
                    }
                },
                |address| address.address,
            );
        let mut packet = crate::Packet {
            input_interface: None,
            output_interface: Some(interface_id),
            source,
            destination: destination.0,
            protocol,
            source_port,
            destination_port: destination.1,
            state: crate::ConnectionState::New,
        };
        let verdict = namespace.netfilter.evaluate(&mut packet, 0);
        if direct
            && (packet.source != source
                || packet.source_port != source_port
                || packet.destination != destination.0
                || packet.destination_port != destination.1)
        {
            // Direct-provider traffic never traverses a namespace packet path.
            // Filtering is meaningful, but pretending that DNAT happened would
            // bypass the provider's domain-scoped policy.
            return Err(NetError::Unsupported);
        }
        Ok(((packet.destination, packet.destination_port), verdict))
    }

    pub fn filter_ingress(
        &mut self,
        socket: u64,
        source: (IpAddr, u16),
        protocol: u8,
        destination_port: u16,
        bytes: usize,
    ) -> Result<((IpAddr, u16), crate::Verdict), NetError> {
        let value = self.sockets.get(&socket).ok_or(NetError::NotFound)?;
        let namespace_id = value.namespace;
        let namespace = self
            .namespaces
            .get_mut(&namespace_id)
            .ok_or(NetError::NotFound)?;
        let interface_id = value
            .bound_interface
            .or_else(|| {
                value.local.and_then(|local| {
                    namespace.interfaces.values().find_map(|interface| {
                        interface
                            .addresses
                            .iter()
                            .any(|address| address.address == local.0)
                            .then_some(interface.id)
                    })
                })
            })
            .or_else(|| {
                namespace
                    .route(source.0, None)
                    .map(|route| route.interface_id)
            })
            .ok_or(NetError::NetworkUnreachable)?;
        let direct = matches!(
            namespace
                .interfaces
                .get(&interface_id)
                .map(|interface| &interface.backend),
            Some(InterfaceBackend::Direct { .. })
        );
        let destination = value.local.map_or_else(
            || {
                if source.0.is_ipv4() {
                    IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED)
                } else {
                    IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED)
                }
            },
            |local| local.0,
        );
        let mut packet = crate::Packet {
            input_interface: Some(interface_id),
            output_interface: None,
            source: source.0,
            destination,
            protocol,
            source_port: source.1,
            destination_port,
            state: crate::ConnectionState::New,
        };
        let verdict = namespace.netfilter.evaluate(&mut packet, bytes);
        if direct
            && (packet.source != source.0
                || packet.source_port != source.1
                || packet.destination != destination
                || packet.destination_port != destination_port)
        {
            return Err(NetError::Unsupported);
        }
        Ok(((packet.source, packet.source_port), verdict))
    }

    pub fn require(&self, tid: u32, capability: u64) -> Result<(), NetError> {
        self.task_capabilities
            .get(&tid)
            .copied()
            .filter(|capabilities| capabilities.has(capability))
            .map(|_| ())
            .ok_or(NetError::PermissionDenied)
    }

    fn release_owner(&mut self, namespace: u64) {
        if let Some(value) = self.namespaces.get_mut(&namespace) {
            value.owner_references = value.owner_references.saturating_sub(1);
        }
    }

    fn collect_namespaces(&mut self) {
        let socket_namespaces = self
            .sockets
            .values()
            .map(|socket| socket.namespace)
            .collect::<BTreeSet<_>>();
        self.namespaces.retain(|id, namespace| {
            *id == 1
                || namespace.owner_references != 0
                || namespace.fd_references != 0
                || socket_namespaces.contains(id)
        });
    }
}
