use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::packet::IpAddress;
use crate::switch::SwitchError;

pub const MAX_INTERFACES: usize = 256;
pub const MAX_ROUTES_PER_TABLE: usize = 4096;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InterfaceEndpoint {
    Physical(u64),
    Virtual(u64),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RoutedInterface {
    pub id: u64,
    pub table_id: u32,
    pub bridge_domain: u32,
    pub vlan_id: u16,
    pub endpoint: InterfaceEndpoint,
    pub mac: [u8; 6],
    pub mtu: u32,
    pub zone: u16,
    pub addresses: Vec<(IpAddress, u8)>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Route {
    pub table_id: u32,
    pub network: IpAddress,
    pub prefix_len: u8,
    pub gateway: Option<IpAddress>,
    pub interface_id: u64,
    pub metric: u32,
}

#[derive(Clone, Debug, Default)]
pub struct RoutingPlane {
    pub interfaces: BTreeMap<u64, RoutedInterface>,
    pub tables: BTreeMap<u32, Vec<Route>>,
}

impl RoutingPlane {
    pub fn interface_for_endpoint(&self, endpoint: InterfaceEndpoint) -> Option<&RoutedInterface> {
        self.interfaces
            .values()
            .find(|interface| interface.endpoint == endpoint)
    }

    pub fn configure_interface(&mut self, interface: RoutedInterface) -> Result<(), SwitchError> {
        if interface.id == 0
            || interface.mac == [0; 6]
            || interface.mac[0] & 1 != 0
            || interface.vlan_id > 4094
            || !(576..=9216).contains(&interface.mtu)
            || interface.addresses.iter().any(|(address, prefix)| {
                *prefix
                    > match address {
                        IpAddress::V4(_) => 32,
                        IpAddress::V6(_) => 128,
                    }
            })
        {
            return Err(SwitchError::InvalidRoute);
        }
        if !self.interfaces.contains_key(&interface.id) && self.interfaces.len() >= MAX_INTERFACES {
            return Err(SwitchError::QueueFull);
        }
        self.interfaces.insert(interface.id, interface);
        Ok(())
    }

    pub fn remove_interface(&mut self, id: u64) -> Result<(), SwitchError> {
        if self
            .tables
            .values()
            .flatten()
            .any(|route| route.interface_id == id)
        {
            return Err(SwitchError::InUse);
        }
        self.interfaces
            .remove(&id)
            .map(|_| ())
            .ok_or(SwitchError::NotFound)
    }

    pub fn add_route(&mut self, mut route: Route) -> Result<(), SwitchError> {
        let interface = self
            .interfaces
            .get(&route.interface_id)
            .ok_or(SwitchError::NotFound)?;
        if interface.table_id != route.table_id || !valid_prefix(route.network, route.prefix_len) {
            return Err(SwitchError::InvalidRoute);
        }
        route.network = mask(route.network, route.prefix_len);
        let table = self.tables.entry(route.table_id).or_default();
        if table.contains(&route) {
            return Err(SwitchError::AlreadyExists);
        }
        if table.len() >= MAX_ROUTES_PER_TABLE {
            return Err(SwitchError::QueueFull);
        }
        table.push(route);
        Ok(())
    }

    pub fn remove_route(&mut self, mut route: Route) -> Result<(), SwitchError> {
        route.network = mask(route.network, route.prefix_len);
        let table = self
            .tables
            .get_mut(&route.table_id)
            .ok_or(SwitchError::NotFound)?;
        let index = table
            .iter()
            .position(|candidate| *candidate == route)
            .ok_or(SwitchError::NotFound)?;
        table.remove(index);
        Ok(())
    }

    pub fn lookup(&self, table_id: u32, address: IpAddress) -> Option<Route> {
        self.tables
            .get(&table_id)?
            .iter()
            .copied()
            .filter(|route| contains(route.network, route.prefix_len, address))
            .min_by_key(|route| (u8::MAX - route.prefix_len, route.metric, route.interface_id))
    }
}

fn valid_prefix(address: IpAddress, prefix: u8) -> bool {
    prefix
        <= match address {
            IpAddress::V4(_) => 32,
            IpAddress::V6(_) => 128,
        }
}

pub fn mask(address: IpAddress, prefix: u8) -> IpAddress {
    match address {
        IpAddress::V4(mut bytes) => {
            mask_bytes(&mut bytes, prefix);
            IpAddress::V4(bytes)
        }
        IpAddress::V6(mut bytes) => {
            mask_bytes(&mut bytes, prefix);
            IpAddress::V6(bytes)
        }
    }
}

fn contains(network: IpAddress, prefix: u8, address: IpAddress) -> bool {
    match (mask(network, prefix), mask(address, prefix)) {
        (IpAddress::V4(a), IpAddress::V4(b)) => a == b,
        (IpAddress::V6(a), IpAddress::V6(b)) => a == b,
        _ => false,
    }
}

fn mask_bytes(bytes: &mut [u8], prefix: u8) {
    let whole = usize::from(prefix / 8);
    let remaining = prefix % 8;
    if remaining != 0 && whole < bytes.len() {
        bytes[whole] &= 0xff << (8 - remaining);
    }
    let start = whole + usize::from(remaining != 0);
    bytes.iter_mut().skip(start).for_each(|byte| *byte = 0);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn interface(id: u64) -> RoutedInterface {
        RoutedInterface {
            id,
            table_id: 7,
            bridge_domain: 1,
            vlan_id: 0,
            endpoint: InterfaceEndpoint::Virtual(id),
            mac: [2, 0, 0, 0, 0, id as u8],
            mtu: 1500,
            zone: 1,
            addresses: Vec::new(),
        }
    }

    #[test]
    fn longest_prefix_metric_and_table_are_deterministic() {
        let mut plane = RoutingPlane::default();
        plane.configure_interface(interface(1)).unwrap();
        plane.configure_interface(interface(2)).unwrap();
        for route in [
            Route {
                table_id: 7,
                network: IpAddress::V4([10, 0, 0, 0]),
                prefix_len: 8,
                gateway: None,
                interface_id: 1,
                metric: 1,
            },
            Route {
                table_id: 7,
                network: IpAddress::V4([10, 2, 0, 0]),
                prefix_len: 16,
                gateway: None,
                interface_id: 2,
                metric: 20,
            },
        ] {
            plane.add_route(route).unwrap();
        }
        assert_eq!(
            plane
                .lookup(7, IpAddress::V4([10, 2, 3, 4]))
                .unwrap()
                .interface_id,
            2
        );
        assert!(plane.lookup(8, IpAddress::V4([10, 2, 3, 4])).is_none());
        plane
            .add_route(Route {
                table_id: 7,
                network: IpAddress::V6([0x20, 1, 0x0d, 0xb8, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
                prefix_len: 64,
                gateway: None,
                interface_id: 2,
                metric: 1,
            })
            .unwrap();
        assert_eq!(
            plane
                .lookup(
                    7,
                    IpAddress::V6([0x20, 1, 0x0d, 0xb8, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 9,])
                )
                .unwrap()
                .interface_id,
            2
        );
    }
}
