use alloc::vec::Vec;

use bexos_userspace::Channel;
use net_fidl::*;

use crate::migration::Runtime;
use crate::packet::IpAddress as PacketIpAddress;
use crate::routing::{InterfaceEndpoint, Route, RoutedInterface};
use crate::switch::SwitchError;

pub fn handle(
    runtime: &mut Runtime,
    channel: Channel,
    ordinal: u64,
    request: &[u8],
    handles: &[HandleRef],
) {
    match ordinal {
        1 => {
            let status = SwitchRoutingControllerConfigureInterfaceRequest::decode(request, handles)
                .map_err(|_| SwitchError::InvalidRoute)
                .and_then(|request| interface(request.interface))
                .and_then(|interface| runtime.data_plane.routing.configure_interface(interface))
                .map(|_| Status::Ok)
                .unwrap_or_else(status);
            reply(
                channel,
                &SwitchRoutingControllerConfigureInterfaceResponse { status },
            );
        }
        2 => {
            let status = SwitchRoutingControllerRemoveInterfaceRequest::decode(request, handles)
                .map_err(|_| SwitchError::InvalidRoute)
                .and_then(|request| {
                    runtime
                        .data_plane
                        .routing
                        .remove_interface(request.interface_id)
                })
                .map(|_| Status::Ok)
                .unwrap_or_else(status);
            reply(
                channel,
                &SwitchRoutingControllerRemoveInterfaceResponse { status },
            );
        }
        3 => {
            let status = SwitchRoutingControllerAddRouteRequest::decode(request, handles)
                .map_err(|_| SwitchError::InvalidRoute)
                .and_then(|request| route(request.route))
                .and_then(|route| runtime.data_plane.routing.add_route(route))
                .map(|_| Status::Ok)
                .unwrap_or_else(status);
            reply(channel, &SwitchRoutingControllerAddRouteResponse { status });
        }
        4 => {
            let status = SwitchRoutingControllerRemoveRouteRequest::decode(request, handles)
                .map_err(|_| SwitchError::InvalidRoute)
                .and_then(|request| route(request.route))
                .and_then(|route| runtime.data_plane.routing.remove_route(route))
                .map(|_| Status::Ok)
                .unwrap_or_else(status);
            reply(
                channel,
                &SwitchRoutingControllerRemoveRouteResponse { status },
            );
        }
        5 => {
            let status = SwitchRoutingControllerUpdateNeighborRequest::decode(request, handles)
                .map_err(|_| SwitchError::InvalidNeighbor)
                .and_then(|request| {
                    runtime.data_plane.neighbors.learn(
                        request.neighbor.interface_id,
                        address(request.neighbor.address),
                        request.neighbor.mac,
                        monotonic_ns(),
                    )
                })
                .map(|_| Status::Ok)
                .unwrap_or_else(status);
            reply(
                channel,
                &SwitchRoutingControllerUpdateNeighborResponse { status },
            );
        }
        _ => {}
    }
}

fn interface(value: SwitchRoutedInterface<'_>) -> Result<RoutedInterface, SwitchError> {
    let endpoint = match value.endpoint {
        SwitchEndpoint::PhysicalInterface(value) => InterfaceEndpoint::Physical(value.interface_id),
        SwitchEndpoint::VirtualPort(value) => InterfaceEndpoint::Virtual(value.port_id),
    };
    let mut addresses = Vec::new();
    for index in 0..value.addresses.len() {
        let subnet = value
            .addresses
            .get(index)
            .map_err(|_| SwitchError::InvalidRoute)?;
        addresses.push((address(subnet.network), subnet.prefix_len));
    }
    Ok(RoutedInterface {
        id: value.interface_id,
        table_id: value.table.value,
        bridge_domain: value.bridge_domain,
        vlan_id: value.vlan_id,
        endpoint,
        mac: value.mac,
        mtu: value.mtu,
        zone: value.zone,
        addresses,
    })
}

fn route(value: SwitchRoute) -> Result<Route, SwitchError> {
    Ok(Route {
        table_id: value.table.value,
        network: address(value.destination.network),
        prefix_len: value.destination.prefix_len,
        gateway: value.has_gateway.then(|| address(value.gateway)),
        interface_id: value.interface_id,
        metric: value.metric,
    })
}

fn address(value: IpAddress) -> PacketIpAddress {
    match value {
        IpAddress::Ipv4(value) => PacketIpAddress::V4(value.octets),
        IpAddress::Ipv6(value) => PacketIpAddress::V6(value.octets),
    }
}

fn monotonic_ns() -> u64 {
    let frequency = bexos_userspace::syscall::frequency().max(1);
    ((u128::from(bexos_userspace::syscall::ticks()) * 1_000_000_000) / u128::from(frequency)) as u64
}

fn status(error: SwitchError) -> Status {
    match error {
        SwitchError::AlreadyExists => Status::ErrAlreadyExists,
        SwitchError::NotFound => Status::ErrNotFound,
        SwitchError::QueueFull => Status::ErrResourceExhausted,
        SwitchError::InUse => Status::ErrShouldWait,
        _ => Status::ErrInvalidArgs,
    }
}

fn reply<T: FidlEncode>(channel: Channel, response: &T) {
    let mut bytes = alloc::vec![0; 4096];
    if let Ok(encoded) = response.encode(&mut bytes, &mut []) {
        let _ = channel.send(&bytes[..encoded.bytes], &[]);
    }
}
