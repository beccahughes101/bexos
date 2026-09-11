use bexos_netstackd::{
    config::{ConfigSource, DhcpLease, NetConfig},
    dhcp,
    dns::{DnsRecord, encode_query, parse_a_records},
    link::{LinkModel, LinkResources, PacketLink, SLOT_COUNT, SLOT_SIZE},
    migration::Runtime,
    stack::Netstack,
    tcp::{TcpEndpoint, TcpState},
    udp::{decode_udp_ipv4_ethernet, encode_udp_ipv4_ethernet},
};
use bexos_userspace::live_migration::{Resource, State};
use bexos_userspace::service_binding::BoundServiceEndpoint;
use ethernet_fidl::{FrameEntry, FrameFlags, FrameOpcode};
use net_fidl::{IpAddress, Ipv4Address, SocketAddress, Status};

#[test]
fn static_config_wins_over_dhcp() {
    let config = NetConfig {
        static_ipv4: Some([192, 0, 2, 10]),
        static_prefix_len: 24,
        static_gateway: Some([192, 0, 2, 1]),
        static_dns: Some([192, 0, 2, 53]),
        dhcp_enabled: true,
        dhcp_timeout_ms: 1,
        mtu: 1500,
        ..NetConfig::default()
    };
    let lease = DhcpLease {
        ipv4: [10, 0, 2, 15],
        prefix_len: 24,
        gateway: Some([10, 0, 2, 2]),
        dns: Some([10, 0, 2, 3]),
    };
    let stack = Netstack::new(config, Some(lease));
    assert_eq!(stack.config.source, ConfigSource::Static);
    assert_eq!(stack.config.ipv4, Some([192, 0, 2, 10]));
}

#[test]
fn dhcp_config_used_when_static_missing() {
    let lease = DhcpLease {
        ipv4: [10, 0, 2, 15],
        prefix_len: 24,
        gateway: Some([10, 0, 2, 2]),
        dns: Some([10, 0, 2, 3]),
    };
    let stack = Netstack::new(NetConfig::default(), Some(lease));
    assert_eq!(stack.config.source, ConfigSource::Dhcp);
    assert_eq!(stack.config.dns, Some([10, 0, 2, 3]));
}

#[test]
fn tcp_requires_configured_ip() {
    let mut stack = Netstack::default();
    let remote = SocketAddress {
        addr: IpAddress::Ipv4(Ipv4Address {
            octets: [1, 1, 1, 1],
        }),
        port: 443,
    };
    assert_eq!(stack.connect_tcp(7, remote), Status::ErrNetworkUnreachable);
}

#[test]
fn dns_query_and_a_record_response_round_trip() {
    let mut query = [0; 512];
    let len = encode_query("example.com", 0x1234, &mut query).unwrap();
    assert!(len > 20);
    let response = [
        0x12, 0x34, 0x81, 0x80, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x07, b'e', b'x',
        b'a', b'm', b'p', b'l', b'e', 0x03, b'c', b'o', b'm', 0x00, 0x00, 0x01, 0x00, 0x01, 0xc0,
        0x0c, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x00, 0x3c, 0x00, 0x04, 93, 184, 216, 34,
    ];
    let addresses = parse_a_records(&response, 0x1234).unwrap();
    assert_eq!(addresses, vec![[93, 184, 216, 34]]);
}

#[test]
fn fifo_link_model_sequences_tx_and_rx_entries() {
    let mut model = LinkModel::default();
    let tx = model.tx_entry(9, 128).unwrap();
    assert_eq!(tx.opcode, FrameOpcode::TxSend);
    assert_eq!(tx.vmo_id, 9);
    assert_eq!(tx.offset, 0);
    assert_eq!(tx.length, 128);

    let rx = model.rx_supply_entries(3, SLOT_COUNT);
    assert_eq!(rx.len(), SLOT_COUNT);
    assert_eq!(rx[0].opcode, FrameOpcode::RxSupply);
    assert_eq!(rx[1].offset, SLOT_SIZE as u32);
    assert_eq!(model.rx_supply_entries(3, SLOT_COUNT).len(), 0);

    model.complete(FrameEntry {
        opcode: FrameOpcode::RxComplete,
        length: 64,
        flags: FrameFlags(0),
        ..rx[0]
    });
    let complete = model.pop_rx().unwrap();
    assert_eq!(complete.req_id, rx[0].req_id);
    assert_eq!(complete.len, 64);
    assert_eq!(complete.offset, 0);
}

#[test]
fn fifo_link_model_rejects_oversized_tx_frame() {
    let mut model = LinkModel::default();
    assert_eq!(
        model.tx_entry(1, SLOT_SIZE + 1).unwrap_err(),
        Status::ErrInvalidArgs
    );
}

#[test]
fn fifo_buffers_are_not_reused_until_completion_and_consumption() {
    let mut model = LinkModel::default();
    let tx: Vec<_> = (0..SLOT_COUNT)
        .map(|_| model.tx_entry(1, 32).unwrap())
        .collect();
    assert_eq!(model.tx_entry(1, 32).unwrap_err(), Status::ErrShouldWait);
    model.complete(FrameEntry {
        opcode: FrameOpcode::TxComplete,
        ..tx[7]
    });
    assert_eq!(model.tx_entry(1, 32).unwrap().offset, tx[7].offset);
    let rx = model.rx_supply_entries(2, SLOT_COUNT);
    model.complete(FrameEntry {
        opcode: FrameOpcode::RxComplete,
        length: 32,
        ..rx[3]
    });
    assert!(model.rx_supply_entries(2, SLOT_COUNT).is_empty());
    assert_eq!(model.pop_rx().unwrap().req_id, rx[3].req_id);
    assert_eq!(
        model.rx_supply_entries(2, SLOT_COUNT)[0].offset,
        rx[3].offset
    );
}

#[test]
fn deferred_packet_is_only_consumed_by_smoltcp_not_reclassified_forever() {
    let mut link = PacketLink::from_resources(LinkResources {
        control: 1,
        fifo: 2,
        rx_vmo: 3,
        tx_vmo: 4,
        rx_vaddr: 0,
        tx_vaddr: 0,
        rx_vmo_id: 1,
        tx_vmo_id: 2,
        mtu: 1500,
        mac: [0; 6],
    });
    assert_eq!(link.push_rx_backlog(b"packet"), Status::Ok);
    let mut bytes = [0; 16];
    assert_eq!(link.receive_device(&mut bytes), Err(Status::ErrShouldWait));
    assert_eq!(link.receive(&mut bytes).unwrap(), 6);
    assert_eq!(&bytes[..6], b"packet");
    assert_eq!(link.receive(&mut bytes), Err(Status::ErrShouldWait));
}

#[test]
fn dns_retry_response_with_no_addresses_is_encodable() {
    use net_fidl::{FidlDecode, FidlEncode, NetstackResolveHostResponse};
    let response = NetstackResolveHostResponse {
        status: Status::ErrShouldWait,
        addresses: net_fidl::WireVector::from_slice(&[]),
    };
    let mut bytes = [0; 256];
    let size = response.encode(&mut bytes, &mut []).unwrap().bytes;
    let decoded = NetstackResolveHostResponse::decode(&bytes[..size], &[]).unwrap();
    assert_eq!(decoded.status, Status::ErrShouldWait);
    assert!(decoded.addresses.is_empty());
}

#[test]
fn dhcp_ack_adopts_address_router_dns_and_prefix() {
    let mut packet = [0u8; 256];
    packet[16..20].copy_from_slice(&[10, 0, 2, 15]);
    packet[236..240].copy_from_slice(&[99, 130, 83, 99]);
    packet[240..243].copy_from_slice(&[53, 1, 5]);
    packet[243..249].copy_from_slice(&[1, 4, 255, 255, 255, 0]);
    packet[249..255].copy_from_slice(&[3, 4, 10, 0, 2, 2]);
    packet[255] = 255;
    let lease = dhcp::parse_ack(&packet).unwrap();
    assert_eq!(lease.ipv4, [10, 0, 2, 15]);
    assert_eq!(lease.prefix_len, 24);
    assert_eq!(lease.gateway, Some([10, 0, 2, 2]));
}

#[test]
fn udp_ipv4_ethernet_round_trip_decodes_payload_and_ports() {
    let source = SocketAddress {
        addr: IpAddress::Ipv4(Ipv4Address {
            octets: [10, 0, 2, 15],
        }),
        port: 49152,
    };
    let destination = SocketAddress {
        addr: IpAddress::Ipv4(Ipv4Address {
            octets: [10, 0, 2, 3],
        }),
        port: 53,
    };
    let mut frame = [0u8; SLOT_SIZE];
    let len = encode_udp_ipv4_ethernet(
        b"hello",
        source,
        destination,
        [0x52, 0x54, 0, 0x12, 0x34, 0x56],
        &mut frame,
    )
    .unwrap();

    let decoded = decode_udp_ipv4_ethernet(&frame[..len]).unwrap();
    assert_eq!(decoded.source.port, 49152);
    assert_eq!(decoded.destination.port, 53);
    assert_eq!(&decoded.data[..decoded.len], b"hello");
}

#[test]
fn udp_send_to_queues_packet_for_packet_plane() {
    let mut stack = Netstack::new(
        NetConfig {
            static_ipv4: Some([10, 0, 2, 15]),
            static_dns: Some([10, 0, 2, 3]),
            ..NetConfig::default()
        },
        None,
    );
    let socket = SocketAddress {
        addr: IpAddress::Ipv4(Ipv4Address {
            octets: [10, 0, 2, 15],
        }),
        port: 49152,
    };
    let destination = SocketAddress {
        addr: IpAddress::Ipv4(Ipv4Address {
            octets: [10, 0, 2, 3],
        }),
        port: 53,
    };
    assert_eq!(stack.create_udp(7), Status::Ok);
    assert_eq!(stack.udp_mut(7).unwrap().bind(socket), Status::Ok);
    assert_eq!(
        stack.udp_mut(7).unwrap().send_to(b"query", destination),
        (Status::Ok, 5)
    );
    assert_eq!(stack.udp_mut(7).unwrap().drain_outbound().len(), 1);
}

#[test]
fn migration_record_preserves_config_clients_dns_sockets_and_link_resources() {
    let mut stack = Netstack::new(
        NetConfig {
            static_ipv4: Some([192, 0, 2, 10]),
            static_gateway: Some([192, 0, 2, 1]),
            static_dns: Some([192, 0, 2, 53]),
            ..NetConfig::default()
        },
        None,
    );
    stack.next_ephemeral_port = 50000;
    stack.cache_dns("example.com", &[[93, 184, 216, 34]]);
    stack.tcp.push(TcpEndpoint {
        control: 30,
        stream: None,
        peer: SocketAddress {
            addr: IpAddress::Ipv4(Ipv4Address {
                octets: [93, 184, 216, 34],
            }),
            port: 443,
        },
        local: SocketAddress {
            addr: IpAddress::Ipv4(Ipv4Address {
                octets: [192, 0, 2, 10],
            }),
            port: 50000,
        },
        state: TcpState::Closed,
        smoltcp_handle: None,
        smoltcp_migration: None,
        client_control: None,
    });
    let runtime = Runtime {
        control: bexos_userspace::Channel(1),
        migration: Some(bexos_userspace::Channel(2)),
        tls_trust: Some(bexos_userspace::Channel(19)),
        clients: vec![BoundServiceEndpoint::new(
            bexos_userspace::Channel(20),
            vec![1, 2, 3, 4],
        )],
        link_watchers: vec![21],
        stack,
        link: Some(PacketLink::from_resources(LinkResources {
            control: 3,
            fifo: 4,
            rx_vmo: 5,
            tx_vmo: 6,
            rx_vaddr: 0x1000,
            tx_vaddr: 0x2000,
            rx_vmo_id: 7,
            tx_vmo_id: 8,
            mtu: 1500,
            mac: [0x52, 0x54, 0, 0x12, 0x34, 0x56],
        })),
        generation: 9,
    };

    let record = runtime.encode_record(0).unwrap().unwrap();
    let mut adopted = Runtime::empty();
    adopted.adopt_record(0, Some(&record)).unwrap();

    assert_eq!(adopted.generation, 9);
    assert_eq!(adopted.stack.config.ipv4, Some([192, 0, 2, 10]));
    assert_eq!(adopted.stack.next_ephemeral_port, 50000);
    assert_eq!(
        adopted.stack.resolve_cached("example.com").unwrap(),
        &[DnsRecord::A([93, 184, 216, 34])]
    );
    assert_eq!(adopted.clients[0].channel.0, 20);
    assert_eq!(adopted.link_watchers, vec![21]);
    assert_eq!(adopted.stack.tcp[0].control, 30);
    assert!(matches!(
        adopted.resources().as_slice(),
        [Resource::Handle(1), Resource::Handle(2), ..]
    ));
    assert!(adopted.resources().iter().any(|resource| {
        matches!(
            resource,
            Resource::Mapping {
                handle: 5,
                va: 0x1000,
                ..
            }
        )
    }));
}

#[test]
fn migration_validation_rejects_established_tcp_without_smoltcp_checkpoint() {
    let mut runtime = Runtime::empty();
    runtime.control = bexos_userspace::Channel(1);
    runtime.migration = Some(bexos_userspace::Channel(2));
    runtime.stack.tcp.push(TcpEndpoint {
        control: 30,
        stream: None,
        peer: SocketAddress {
            addr: IpAddress::Ipv4(Ipv4Address {
                octets: [93, 184, 216, 34],
            }),
            port: 443,
        },
        local: SocketAddress {
            addr: IpAddress::Ipv4(Ipv4Address {
                octets: [192, 0, 2, 10],
            }),
            port: 50000,
        },
        state: TcpState::Established,
        smoltcp_handle: None,
        smoltcp_migration: None,
        client_control: None,
    });

    assert!(runtime.validate().is_err());
}

#[test]
fn dns_union_vector_round_trip_and_invalid_payload() {
    use net_fidl::{FidlDecode, FidlEncode, Ipv6Address, NetstackResolveHostResponse, WireVector};
    let addresses = [
        IpAddress::Ipv4(Ipv4Address {
            octets: [10, 0, 2, 3],
        }),
        IpAddress::Ipv6(Ipv6Address { octets: [0x20; 16] }),
    ];
    let response = NetstackResolveHostResponse {
        status: Status::Ok,
        addresses: WireVector::from_slice(&addresses),
    };
    let mut bytes = [0; 256];
    let size = response.encode(&mut bytes, &mut []).unwrap().bytes;
    let decoded = NetstackResolveHostResponse::decode(&bytes[..size], &[]).unwrap();
    assert_eq!(decoded.addresses.len(), 2);
    assert_eq!(decoded.addresses.get(0).unwrap(), addresses[0]);
    assert_eq!(decoded.addresses.get(1).unwrap(), addresses[1]);
    assert!(decoded.addresses.get(2).is_err());
    let truncated = NetstackResolveHostResponse::decode(&bytes[..size - 1], &[]).unwrap();
    assert!(truncated.addresses.get(1).is_err());
    // Vector field follows the four-byte status: reject an overflowing count.
    bytes[12..20].copy_from_slice(&u64::MAX.to_le_bytes());
    assert!(NetstackResolveHostResponse::decode(&bytes[..size], &[]).is_err());
}

#[test]
fn header_delta_preserves_previously_adopted_packet_records() {
    let mut source = Runtime::empty();
    source.control = bexos_userspace::Channel(1);
    source.migration = Some(bexos_userspace::Channel(2));
    let mut link = PacketLink::from_resources(LinkResources {
        control: 3,
        fifo: 4,
        rx_vmo: 5,
        tx_vmo: 6,
        rx_vaddr: 0x1000,
        tx_vaddr: 0x2000,
        rx_vmo_id: 5,
        tx_vmo_id: 6,
        mtu: 1500,
        mac: [0; 6],
    });
    link.push_rx_backlog(b"retained packet");
    source.link = Some(link);
    let mut target = Runtime::empty();
    let header = source.encode_record(0).unwrap().unwrap();
    target.adopt_record(0, Some(&header)).unwrap();
    assert!(
        target.validate().is_err(),
        "missing queue ownership must reject adoption"
    );
    for key in source.keys().into_iter().skip(1) {
        target
            .adopt_record(key, source.encode_record(key).unwrap().as_deref())
            .unwrap();
    }
    target.validate().unwrap();
    source.generation += 1;
    target
        .adopt_record(0, source.encode_record(0).unwrap().as_deref())
        .unwrap();
    target.validate().unwrap();
    let mut packet = [0; 32];
    let len = target.link.as_mut().unwrap().receive(&mut packet).unwrap();
    assert_eq!(&packet[..len], b"retained packet");
    let mut wrong_arch = header.clone();
    wrong_arch[8..16].copy_from_slice(&3u64.to_le_bytes());
    assert!(Runtime::empty().adopt_record(0, Some(&wrong_arch)).is_err());
}

#[test]
fn udp_bind_nested_ipv4_and_ipv6_wire_sizes_fit_client_buffer() {
    use net_fidl::{FidlDecode, FidlEncode, Ipv6Address, UdpSocketBindRequest};
    for addr in [
        IpAddress::Ipv4(Ipv4Address { octets: [0; 4] }),
        IpAddress::Ipv6(Ipv6Address { octets: [0; 16] }),
    ] {
        let request = UdpSocketBindRequest {
            local_addr: SocketAddress { addr, port: 0 },
        };
        assert!(request.encode(&mut [0; 64], &mut []).is_err());
        let mut bytes = [0; 128];
        let size = request.encode(&mut bytes, &mut []).unwrap().bytes;
        let decoded = UdpSocketBindRequest::decode(&bytes[..size], &[]).unwrap();
        assert_eq!(decoded.local_addr, request.local_addr);
    }
}
