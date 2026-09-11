use bexos_d1_virtio_net::guest::migration::Runtime;
use bexos_d1_virtio_net::hal::{DmaAllocation, MmioMapping, SharedRange};
use bexos_d1_virtio_net::server::{Buffer, EthernetServer, rx_complete, rx_error, tx_complete};
use bexos_userspace::live_migration::{Resource, State};
use bexos_userspace::service_binding::BoundServiceEndpoint;
use ethernet_fidl::{DeviceFeatures, FrameEntry, FrameFlags, FrameOpcode, Status};

#[test]
fn server_reports_mac_mtu_and_dma_feature() {
    let server = EthernetServer::new([0x52, 0x54, 0x00, 0x12, 0x34, 0x56]);

    let info = server.info();

    assert_eq!(info.mac.octets, [0x52, 0x54, 0x00, 0x12, 0x34, 0x56]);
    assert_eq!(info.mtu, 1500);
    assert_ne!(info.features.0 & DeviceFeatures::DMA_64BIT.0, 0);
}

#[test]
fn rx_error_preserves_receive_completion_opcode() {
    let entry = FrameEntry {
        req_id: 11,
        opcode: FrameOpcode::RxSupply,
        vmo_id: 2,
        offset: 4,
        length: 64,
        flags: FrameFlags(0),
    };

    let complete = rx_error(entry);

    assert_eq!(complete.req_id, 11);
    assert_eq!(complete.opcode, FrameOpcode::RxComplete);
    assert_ne!(complete.flags.0 & FrameFlags::TRUNCATED.0, 0);
}

#[test]
fn frame_slice_requires_running_and_bounds_checks() {
    let mut server = EthernetServer::new([0; 6]);
    let mut bytes = [0u8; 128];
    let vmo_id = server
        .register_buffer(Buffer {
            handle: 1,
            vaddr: bytes.as_mut_ptr() as u64,
            paddr: 0x1000,
            token: 2,
            size: bytes.len() as u32,
        })
        .unwrap();
    let entry = FrameEntry {
        req_id: 7,
        opcode: FrameOpcode::TxSend,
        vmo_id,
        offset: 8,
        length: 16,
        flags: FrameFlags(0),
    };

    assert_eq!(
        server.frame_slice(entry).unwrap_err(),
        Status::ErrShouldWait
    );
    server.start();
    assert_eq!(server.frame_slice(entry).unwrap().len(), 16);

    let out_of_bounds = FrameEntry {
        offset: 120,
        length: 16,
        ..entry
    };
    assert_eq!(
        server.frame_slice(out_of_bounds).unwrap_err(),
        Status::ErrInvalidArgs
    );
}

#[test]
fn tx_complete_preserves_request_and_reports_error_with_truncated_flag() {
    let entry = FrameEntry {
        req_id: 44,
        opcode: FrameOpcode::TxSend,
        vmo_id: 2,
        offset: 16,
        length: 32,
        flags: FrameFlags(0),
    };

    let ok = tx_complete(entry, Status::Ok);
    assert_eq!(ok.req_id, 44);
    assert_eq!(ok.opcode, FrameOpcode::TxComplete);
    assert_eq!(ok.flags.0 & FrameFlags::TRUNCATED.0, 0);

    let failed = tx_complete(entry, Status::ErrTimedOut);
    assert_ne!(failed.flags.0 & FrameFlags::TRUNCATED.0, 0);
}

#[test]
fn repeated_rx_completions_preserve_offsets_and_lengths() {
    let first = FrameEntry {
        req_id: 1,
        opcode: FrameOpcode::RxSupply,
        vmo_id: 3,
        offset: 0,
        length: 2048,
        flags: FrameFlags(0),
    };
    let second = FrameEntry {
        req_id: 2,
        offset: 2048,
        ..first
    };

    let a = rx_complete(first, 60);
    let b = rx_complete(second, 1514);

    assert_eq!(a.opcode, FrameOpcode::RxComplete);
    assert_eq!(a.offset, 0);
    assert_eq!(a.length, 60);
    assert_eq!(b.offset, 2048);
    assert_eq!(b.length, 1514);
}

#[test]
fn guest_entry_is_tokio_future() {
    let _future = bexos_d1_virtio_net::guest::main(0);
}

#[test]
fn server_checkpoint_round_trip_preserves_buffers_and_running_state() {
    let mut server = EthernetServer::new([0x52, 0x54, 0, 0x12, 0x34, 0x56]);
    server.start();
    let first = server
        .register_buffer(Buffer {
            handle: 11,
            vaddr: 0x1000,
            paddr: 0x2000,
            token: 12,
            size: 4096,
        })
        .unwrap();
    let second = server
        .register_buffer(Buffer {
            handle: 13,
            vaddr: 0x3000,
            paddr: 0x4000,
            token: 14,
            size: 2048,
        })
        .unwrap();

    let adopted = EthernetServer::adopt(&server.checkpoint()).unwrap();

    assert!(adopted.running());
    assert_eq!(adopted.info().mac.octets, [0x52, 0x54, 0, 0x12, 0x34, 0x56]);
    assert_eq!(adopted.next_vmo_id(), second + 1);
    assert_eq!(adopted.buffers()[&first].handle, 11);
    assert_eq!(adopted.buffers()[&second].size, 2048);
}

#[test]
fn migration_records_preserve_handles_server_and_resources() {
    let mut server = EthernetServer::new([1, 2, 3, 4, 5, 6]);
    server.start();
    server
        .register_buffer(Buffer {
            handle: 30,
            vaddr: 0x8000,
            paddr: 0x9000,
            token: 31,
            size: 4096,
        })
        .unwrap();
    let mut runtime = Runtime::empty_for_test();
    runtime.control = bexos_userspace::Channel(1);
    runtime.migration = Some(bexos_userspace::Channel(2));
    runtime.server = Some(server);
    runtime.fifos.push(bexos_userspace::Channel(3));
    let request = FrameEntry {
        req_id: 101,
        opcode: FrameOpcode::RxSupply,
        vmo_id: 1,
        offset: 0,
        length: 1024,
        flags: FrameFlags(0),
    };
    runtime
        .pending
        .rx
        .push_back((bexos_userspace::Channel(3), request));
    runtime.pending.tx.push_back((
        bexos_userspace::Channel(3),
        FrameEntry {
            req_id: 102,
            opcode: FrameOpcode::TxSend,
            ..request
        },
    ));
    runtime.pending.transmitting = Some((
        bexos_userspace::Channel(3),
        FrameEntry {
            req_id: 103,
            opcode: FrameOpcode::TxSend,
            ..request
        },
    ));
    runtime.device_endpoints.push(BoundServiceEndpoint::new(
        bexos_userspace::Channel(4),
        vec![1, 2, 3, 4, 5, 6],
    ));
    runtime.power_endpoints.push(BoundServiceEndpoint::new(
        bexos_userspace::Channel(5),
        vec![1],
    ));
    runtime.set_snapshots_for_test(
        vec![1, 2, 3, 4],
        vec![DmaAllocation {
            paddr: 0xa000,
            vaddr: 0xb000,
            size: 4096,
            handle: 40,
            token: 41,
            active: true,
            owned: true,
        }],
        vec![SharedRange {
            vaddr: 0xc000,
            paddr: 0xd000,
            size: 4096,
            vmo: 30,
            token: 31,
            active: true,
            owned: true,
        }],
        vec![MmioMapping {
            paddr: 0xe000,
            vaddr: 0xf000,
            size: 4096,
            handle: 50,
            active: true,
            owned: true,
        }],
    );

    let mut adopted = Runtime::empty();
    for key in runtime.keys() {
        let record = runtime.encode_record(key).unwrap();
        adopted.adopt_record(key, record.as_deref()).unwrap();
    }

    assert_eq!(adopted.control.0, 1);
    assert_eq!(adopted.migration.unwrap().0, 2);
    assert_eq!(adopted.fifos[0].0, 3);
    assert_eq!(adopted.pending.rx.front().unwrap().1.req_id, 101);
    assert_eq!(adopted.pending.tx.front().unwrap().1.req_id, 102);
    assert_eq!(adopted.pending.transmitting.unwrap().1.req_id, 103);
    assert!(adopted.validate().is_ok());
    let mut foreign = runtime.encode_record(0).unwrap().unwrap();
    let machine = u64::from_le_bytes(foreign[8..16].try_into().unwrap());
    foreign[8..16].copy_from_slice(&(if machine == 1 { 2u64 } else { 1u64 }).to_le_bytes());
    assert!(Runtime::empty().adopt_record(0, Some(&foreign)).is_err());
    assert_eq!(adopted.device_endpoints[0].channel.0, 4);
    assert_eq!(adopted.power_endpoints[0].channel.0, 5);
    assert_eq!(adopted.server.as_ref().unwrap().buffers()[&1].handle, 30);
    assert!(adopted.resources().iter().any(|resource| {
        matches!(
            resource,
            Resource::Mapping {
                handle: 30,
                va: 0xc000,
                ..
            }
        )
    }));
    assert!(
        adopted
            .resources()
            .iter()
            .any(|resource| matches!(resource, Resource::Pin(41)))
    );
}

#[test]
fn migration_validation_rejects_missing_migration_and_bad_buffers() {
    let mut runtime = Runtime::empty_for_test();
    runtime.control = bexos_userspace::Channel(1);
    runtime.server = Some(EthernetServer::new([0; 6]));
    runtime.set_snapshots_for_test(vec![1], Vec::new(), Vec::new(), Vec::new());

    assert!(runtime.validate().is_err());

    runtime.migration = Some(bexos_userspace::Channel(2));
    let mut server = EthernetServer::new([0; 6]);
    server
        .register_buffer(Buffer {
            handle: 0,
            vaddr: 0x1000,
            paddr: 0x2000,
            token: 3,
            size: 4096,
        })
        .unwrap();
    runtime.server = Some(server);

    assert!(runtime.validate().is_err());
}
