use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use net_fidl::{IpAddress, SocketAddress, Status};

use crate::config::ActiveConfig;
use crate::link::PacketLink;
use crate::stack::Netstack;

pub type TableId = u32;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FibRoute {
    pub destination: IpAddress,
    pub prefix_len: u8,
    pub gateway: Option<IpAddress>,
    pub interface_id: u64,
    pub metric: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InterfaceState {
    pub id: u64,
    pub name: alloc::string::String,
    pub up: bool,
    pub neighbor_generation: u64,
    pub packet_generation: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TableQuota {
    pub tcp: usize,
    pub listeners: usize,
    pub udp: usize,
    pub interfaces: usize,
    pub routes: usize,
}

impl Default for TableQuota {
    fn default() -> Self {
        Self {
            tcp: 64,
            listeners: 32,
            udp: 64,
            interfaces: 16,
            routes: 256,
        }
    }
}

pub struct Table {
    pub id: TableId,
    pub stack: Netstack,
    pub fib: Vec<FibRoute>,
    pub interfaces: BTreeMap<u64, InterfaceState>,
    pub links: BTreeMap<u64, PacketLink>,
    pub quota: TableQuota,
}

pub struct Router {
    pub(crate) tables: BTreeMap<TableId, Table>,
    pub(crate) interface_table: BTreeMap<u64, TableId>,
    pub(crate) default_config: ActiveConfig,
}

impl Router {
    pub fn new(default_config: ActiveConfig) -> Self {
        let mut tables = BTreeMap::new();
        tables.insert(
            0,
            Table {
                id: 0,
                stack: Netstack::from_active(default_config.clone()),
                fib: Vec::new(),
                interfaces: BTreeMap::new(),
                links: BTreeMap::new(),
                quota: TableQuota::default(),
            },
        );
        Self {
            tables,
            interface_table: BTreeMap::new(),
            default_config,
        }
    }

    pub fn create_table(&mut self, id: TableId, quota: TableQuota) -> Status {
        if id == 0 {
            return Status::ErrInvalidArgs;
        }
        if self.tables.contains_key(&id) {
            return Status::ErrAlreadyExists;
        }
        self.tables.insert(
            id,
            Table {
                id,
                stack: Netstack::from_active(self.default_config.clone()),
                fib: Vec::new(),
                interfaces: BTreeMap::new(),
                links: BTreeMap::new(),
                quota,
            },
        );
        Status::Ok
    }

    pub fn remove_table(&mut self, id: TableId) -> Status {
        let Some(table) = self.tables.get(&id) else {
            return Status::ErrNotFound;
        };
        if !table.stack.tcp.is_empty()
            || !table.stack.listeners.is_empty()
            || !table.stack.udp.is_empty()
        {
            return Status::ErrShouldWait;
        }
        let interfaces = table.interfaces.keys().copied().collect::<Vec<_>>();
        self.tables.remove(&id);
        for interface in interfaces {
            self.interface_table.remove(&interface);
        }
        Status::Ok
    }

    pub fn table(&self, id: TableId) -> Option<&Table> {
        self.tables.get(&id)
    }
    pub fn table_mut(&mut self, id: TableId) -> Option<&mut Table> {
        self.tables.get_mut(&id)
    }
    pub fn tables(&self) -> impl Iterator<Item = &Table> {
        self.tables.values()
    }

    pub fn attach_interface(&mut self, table_id: TableId, interface: InterfaceState) -> Status {
        if self.interface_table.contains_key(&interface.id) {
            return Status::ErrAlreadyExists;
        }
        let Some(table) = self.tables.get_mut(&table_id) else {
            return Status::ErrNotFound;
        };
        if table.interfaces.len() >= table.quota.interfaces {
            return Status::ErrResourceExhausted;
        }
        self.interface_table.insert(interface.id, table_id);
        table.interfaces.insert(interface.id, interface);
        Status::Ok
    }

    pub fn detach_interface(&mut self, table_id: TableId, interface_id: u64) -> Status {
        let Some(table) = self.tables.get_mut(&table_id) else {
            return Status::ErrNotFound;
        };
        if table
            .fib
            .iter()
            .any(|route| route.interface_id == interface_id)
            || table
                .stack
                .tcp
                .iter()
                .any(|socket| socket.smoltcp_handle.is_some())
        {
            return Status::ErrShouldWait;
        }
        if table.interfaces.remove(&interface_id).is_none() {
            return Status::ErrNotFound;
        }
        table.links.remove(&interface_id);
        self.interface_table.remove(&interface_id);
        Status::Ok
    }

    pub fn attach_link(
        &mut self,
        table_id: TableId,
        interface: InterfaceState,
        link: PacketLink,
    ) -> Status {
        let interface_id = interface.id;
        let status = self.attach_interface(table_id, interface);
        if status != Status::Ok {
            return status;
        }
        self.tables
            .get_mut(&table_id)
            .expect("table validated")
            .links
            .insert(interface_id, link);
        Status::Ok
    }

    pub fn add_route(&mut self, table_id: TableId, mut route: FibRoute) -> Status {
        let Some(table) = self.tables.get_mut(&table_id) else {
            return Status::ErrNotFound;
        };
        if !table.interfaces.contains_key(&route.interface_id)
            || !valid_prefix(route.destination, route.prefix_len)
        {
            return Status::ErrInvalidArgs;
        }
        if table.fib.len() >= table.quota.routes {
            return Status::ErrResourceExhausted;
        }
        route.destination = mask(route.destination, route.prefix_len);
        if table.fib.contains(&route) {
            return Status::ErrAlreadyExists;
        }
        table.fib.push(route);
        Status::Ok
    }

    pub fn remove_route(&mut self, table_id: TableId, mut route: FibRoute) -> Status {
        let Some(table) = self.tables.get_mut(&table_id) else {
            return Status::ErrNotFound;
        };
        if !valid_prefix(route.destination, route.prefix_len) {
            return Status::ErrInvalidArgs;
        }
        route.destination = mask(route.destination, route.prefix_len);
        let Some(index) = table.fib.iter().position(|candidate| *candidate == route) else {
            return Status::ErrNotFound;
        };
        table.fib.remove(index);
        Status::Ok
    }

    pub fn lookup(&self, table_id: TableId, address: IpAddress) -> Option<&FibRoute> {
        self.tables
            .get(&table_id)?
            .fib
            .iter()
            .filter(|route| prefix_contains(route.destination, route.prefix_len, address))
            .min_by_key(|route| (u8::MAX - route.prefix_len, route.metric, route.interface_id))
    }

    pub fn connect_tcp(
        &mut self,
        table_id: TableId,
        control: u64,
        remote: SocketAddress,
    ) -> Status {
        if self.tables.values().any(|table| {
            table
                .stack
                .tcp
                .iter()
                .any(|socket| socket.control == control)
                || table
                    .stack
                    .listeners
                    .iter()
                    .any(|listener| listener.control == control)
                || table
                    .stack
                    .udp
                    .iter()
                    .any(|socket| socket.control == control)
        }) {
            return Status::ErrAlreadyExists;
        }
        if self.lookup(table_id, remote.addr).is_none() {
            return Status::ErrNetworkUnreachable;
        }
        let Some(table) = self.tables.get_mut(&table_id) else {
            return Status::ErrNotFound;
        };
        if table.stack.tcp.len() >= table.quota.tcp {
            return Status::ErrResourceExhausted;
        }
        table.stack.connect_tcp(control, remote)
    }

    pub fn connect_tcp_with_stream(
        &mut self,
        table_id: TableId,
        connection_id: u64,
        remote: SocketAddress,
        stream: u64,
    ) -> Status {
        let status = self.connect_tcp(table_id, connection_id, remote);
        if status != Status::Ok {
            let _ = bexos_userspace::Memory::close(stream);
            return status;
        }
        let Some(endpoint) = self
            .tables
            .get_mut(&table_id)
            .and_then(|table| table.stack.tcp_mut(connection_id))
        else {
            let _ = bexos_userspace::Memory::close(stream);
            return Status::ErrNotFound;
        };
        endpoint.stream = Some(bexos_userspace::Socket(stream));
        self.attach_tcp_to_link(table_id, connection_id, remote.addr)
    }

    pub fn recover_connection(
        &mut self,
        table_id: TableId,
        connection_id: u64,
        stream: u64,
    ) -> Status {
        let Some(table) = self.tables.get_mut(&table_id) else {
            return Status::ErrNotFound;
        };
        let Some(endpoint) = table.stack.tcp_mut(connection_id) else {
            return Status::ErrNotFound;
        };
        endpoint.stream = Some(bexos_userspace::Socket(stream));
        Status::Ok
    }

    pub fn recover_control(
        &mut self,
        table_id: TableId,
        old_control: u64,
        kind: u8,
        control: u64,
    ) -> Status {
        let Some(table) = self.tables.get_mut(&table_id) else {
            return Status::ErrNotFound;
        };
        match kind {
            1 => match table
                .stack
                .listeners
                .iter_mut()
                .find(|listener| listener.control == old_control)
            {
                Some(listener) => listener.control = control,
                None => return Status::ErrNotFound,
            },
            2 => match table
                .stack
                .udp
                .iter_mut()
                .find(|socket| socket.control == old_control)
            {
                Some(socket) => socket.control = control,
                None => return Status::ErrNotFound,
            },
            _ => return Status::ErrInvalidArgs,
        }
        Status::Ok
    }

    fn attach_tcp_to_link(
        &mut self,
        table_id: TableId,
        connection_id: u64,
        destination: IpAddress,
    ) -> Status {
        let Some(interface_id) = self
            .lookup(table_id, destination)
            .map(|route| route.interface_id)
        else {
            return Status::ErrNetworkUnreachable;
        };
        let Some(table) = self.tables.get_mut(&table_id) else {
            return Status::ErrNotFound;
        };
        table
            .stack
            .attach_tcp_to_link(connection_id, table.links.get_mut(&interface_id))
    }

    pub fn poll_packet_planes(&mut self) {
        for table in self.tables.values_mut() {
            let mut ids = table.links.keys().copied().collect::<Vec<_>>();
            ids.sort_unstable();
            if let Some((first, rest)) = ids.split_first() {
                table.stack.poll_packet_plane(table.links.get_mut(first));
                for id in rest {
                    if let Some(link) = table.links.get_mut(id) {
                        table.stack.poll_secondary_packet_plane(link);
                    }
                }
            }
        }
    }

    pub fn close_object(&mut self, object_id: u64) -> Status {
        for table in self.tables.values_mut() {
            if table
                .stack
                .tcp
                .iter()
                .any(|socket| socket.control == object_id)
            {
                remove_backend_tcp(&mut table.stack, object_id);
                return Status::Ok;
            }
            if table
                .stack
                .listeners
                .iter()
                .any(|listener| listener.control == object_id)
            {
                table.stack.remove_listener(object_id);
                return Status::Ok;
            }
            if table
                .stack
                .udp
                .iter()
                .any(|socket| socket.control == object_id)
            {
                table.stack.remove_udp(object_id);
                return Status::Ok;
            }
        }
        Status::ErrNotFound
    }

    pub fn listen_tcp(&mut self, table_id: TableId, control: u64, local: SocketAddress) -> Status {
        if self.tables.values().any(|table| {
            table
                .stack
                .tcp
                .iter()
                .any(|socket| socket.control == control)
                || table
                    .stack
                    .listeners
                    .iter()
                    .any(|listener| listener.control == control)
                || table
                    .stack
                    .udp
                    .iter()
                    .any(|socket| socket.control == control)
        }) {
            return Status::ErrAlreadyExists;
        }
        let Some(table) = self.tables.get_mut(&table_id) else {
            return Status::ErrNotFound;
        };
        if table.interfaces.values().all(|interface| !interface.up) {
            return Status::ErrNetworkUnreachable;
        }
        if table.stack.listeners.len() >= table.quota.listeners {
            return Status::ErrResourceExhausted;
        }
        let status = table.stack.listen_tcp(control, local);
        if status != Status::Ok {
            return status;
        }
        let Some(interface_id) = table
            .interfaces
            .values()
            .filter(|interface| interface.up)
            .map(|interface| interface.id)
            .min()
        else {
            table.stack.remove_listener(control);
            return Status::ErrNetworkUnreachable;
        };
        let status = table
            .stack
            .attach_listener_to_link(control, table.links.get_mut(&interface_id));
        if status != Status::Ok {
            table.stack.remove_listener(control);
        }
        status
    }

    pub fn create_udp(&mut self, table_id: TableId, control: u64) -> Status {
        if self.tables.values().any(|table| {
            table
                .stack
                .tcp
                .iter()
                .any(|socket| socket.control == control)
                || table
                    .stack
                    .listeners
                    .iter()
                    .any(|listener| listener.control == control)
                || table
                    .stack
                    .udp
                    .iter()
                    .any(|socket| socket.control == control)
        }) {
            return Status::ErrAlreadyExists;
        }
        let Some(table) = self.tables.get_mut(&table_id) else {
            return Status::ErrNotFound;
        };
        if table.stack.udp.len() >= table.quota.udp {
            return Status::ErrResourceExhausted;
        }
        table.stack.create_udp(control)
    }

    pub fn route_udp(&self, table_id: TableId, destination: SocketAddress) -> Result<u64, Status> {
        self.lookup(table_id, destination.addr)
            .map(|route| route.interface_id)
            .ok_or(Status::ErrNetworkUnreachable)
    }
}

fn remove_backend_tcp(stack: &mut Netstack, connection_id: u64) {
    let _ = stack.remove_backend_tcp(connection_id);
}

fn valid_prefix(address: IpAddress, prefix_len: u8) -> bool {
    prefix_len
        <= match address {
            IpAddress::Ipv4(_) => 32,
            IpAddress::Ipv6(_) => 128,
        }
}

fn mask(address: IpAddress, prefix_len: u8) -> IpAddress {
    match address {
        IpAddress::Ipv4(mut value) => {
            mask_bytes(&mut value.octets, prefix_len);
            IpAddress::Ipv4(value)
        }
        IpAddress::Ipv6(mut value) => {
            mask_bytes(&mut value.octets, prefix_len);
            IpAddress::Ipv6(value)
        }
    }
}

fn prefix_contains(network: IpAddress, prefix_len: u8, address: IpAddress) -> bool {
    match (mask(network, prefix_len), mask(address, prefix_len)) {
        (IpAddress::Ipv4(a), IpAddress::Ipv4(b)) => a.octets == b.octets,
        (IpAddress::Ipv6(a), IpAddress::Ipv6(b)) => a.octets == b.octets,
        _ => false,
    }
}

fn mask_bytes(bytes: &mut [u8], prefix_len: u8) {
    let full = usize::from(prefix_len / 8);
    let remainder = prefix_len % 8;
    if remainder != 0 && full < bytes.len() {
        bytes[full] &= 0xff << (8 - remainder);
    }
    for byte in bytes.iter_mut().skip(full + usize::from(remainder != 0)) {
        *byte = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ConfigSource, DnsMode};
    use alloc::string::ToString;
    use net_fidl::{Ipv4Address, Ipv6Address};

    fn config() -> ActiveConfig {
        ActiveConfig {
            source: ConfigSource::Static,
            ipv4: Some([10, 0, 0, 2]),
            prefix_len: 24,
            gateway: None,
            dns: None,
            ipv6: Some([0; 16]),
            ipv6_prefix_len: 64,
            ipv6_gateway: None,
            dns_ipv6: None,
            doh_bootstrap_ipv6: None,
            slaac_enabled: false,
            mtu: 1500,
            dns_mode: DnsMode::Udp53,
            doh_host: "dns.test".to_string(),
            doh_path: "/dns-query".to_string(),
            doh_bootstrap_ipv4: None,
            doh_port: 443,
            doh_strict: true,
            dns_cache_capacity: 32,
        }
    }

    #[test]
    fn overlapping_routes_and_sockets_are_table_isolated() {
        let mut router = Router::new(config());
        assert_eq!(router.create_table(1, TableQuota::default()), Status::Ok);
        assert_eq!(router.create_table(2, TableQuota::default()), Status::Ok);
        for table in [1, 2] {
            assert_eq!(
                router.attach_interface(
                    table,
                    InterfaceState {
                        id: u64::from(table),
                        name: "eth".to_string(),
                        up: true,
                        neighbor_generation: 0,
                        packet_generation: 0,
                    }
                ),
                Status::Ok
            );
            assert_eq!(
                router.add_route(
                    table,
                    FibRoute {
                        destination: IpAddress::Ipv4(Ipv4Address {
                            octets: [10, 0, 0, 0]
                        }),
                        prefix_len: 8,
                        gateway: None,
                        interface_id: u64::from(table),
                        metric: 10,
                    }
                ),
                Status::Ok
            );
        }
        let destination = SocketAddress {
            addr: IpAddress::Ipv4(Ipv4Address {
                octets: [10, 1, 1, 1],
            }),
            port: 80,
        };
        assert_eq!(router.route_udp(1, destination), Ok(1));
        assert_eq!(router.route_udp(2, destination), Ok(2));
        assert_eq!(router.create_udp(1, 100), Status::Ok);
        assert_eq!(router.table(1).unwrap().stack.udp.len(), 1);
        assert_eq!(router.table(2).unwrap().stack.udp.len(), 0);
        let _ = Ipv6Address { octets: [0; 16] };
    }

    #[test]
    fn longest_prefix_then_metric_selects_interface() {
        let mut router = Router::new(config());
        router.create_table(1, TableQuota::default());
        for id in [1, 2] {
            router.attach_interface(
                1,
                InterfaceState {
                    id,
                    name: "eth".to_string(),
                    up: true,
                    neighbor_generation: 0,
                    packet_generation: 0,
                },
            );
        }
        router.add_route(
            1,
            FibRoute {
                destination: IpAddress::Ipv4(Ipv4Address {
                    octets: [10, 0, 0, 0],
                }),
                prefix_len: 8,
                gateway: None,
                interface_id: 1,
                metric: 1,
            },
        );
        router.add_route(
            1,
            FibRoute {
                destination: IpAddress::Ipv4(Ipv4Address {
                    octets: [10, 4, 0, 0],
                }),
                prefix_len: 16,
                gateway: None,
                interface_id: 2,
                metric: 100,
            },
        );
        assert_eq!(
            router
                .lookup(
                    1,
                    IpAddress::Ipv4(Ipv4Address {
                        octets: [10, 4, 2, 1]
                    })
                )
                .unwrap()
                .interface_id,
            2
        );
    }
}
