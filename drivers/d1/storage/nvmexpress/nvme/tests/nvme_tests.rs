use bexos_d1_nvme::block::{BlockBackendOp, BlockDeviceServer};
use bexos_d1_nvme::namespace::{NamespaceInfo, parse_identify_namespace};
use bexos_d1_nvme::prp::{PrpError, build_prp};
use bexos_d1_nvme::queue::{Completion, NvmeQueue, QueueError, Submission};
use bexos_d1_nvme::spec::NvmeStatus;
use block_fidl::{BlockOpcode, BlockRequest, Status};

fn namespace() -> NamespaceInfo {
    NamespaceInfo {
        namespace_id: 1,
        block_size: 512,
        block_count: 128,
        max_transfer_blocks: 16,
    }
}

#[test]
fn queue_wraps_and_matches_command_ids() {
    let mut queue = NvmeQueue::new(2);
    assert_eq!(
        queue.submit(Submission {
            command_id: 7,
            ..Submission::default()
        }),
        Ok(0)
    );
    queue.inject_completion(
        0,
        Completion {
            command_id: 7,
            status: 0,
            phase: true,
        },
    );
    assert_eq!(
        queue.complete_next(7).expect("completion").decoded_status(),
        NvmeStatus::Success
    );
    assert_eq!(queue.completion_head(), 1);

    assert_eq!(
        queue.submit(Submission {
            command_id: 8,
            ..Submission::default()
        }),
        Ok(1)
    );
    queue.inject_completion(
        1,
        Completion {
            command_id: 9,
            status: 0,
            phase: true,
        },
    );
    assert_eq!(
        queue.complete_next(8),
        Err(QueueError::CommandIdMismatch {
            expected: 8,
            actual: 9
        })
    );
}

#[test]
fn completion_status_decodes_known_status_codes() {
    assert_eq!(NvmeStatus::from_completion(0), NvmeStatus::Success);
    assert_eq!(NvmeStatus::from_completion(0x2), NvmeStatus::InvalidOpcode);
    assert_eq!(NvmeStatus::from_completion(0x4), NvmeStatus::InvalidField);
}

#[test]
fn prp_builder_supports_one_or_two_pages_and_rejects_long_spans() {
    assert_eq!(build_prp(0x8000, 512).expect("single page").pages, 1);
    let two = build_prp(0x8f00, 1024).expect("two pages");
    assert_eq!(two.prp1, 0x8f00);
    assert_eq!(two.prp2, 0x9000);
    assert_eq!(build_prp(0x8f00, 8192), Err(PrpError::CrossesTooManyPages));
}

#[test]
fn namespace_identify_parses_lba_geometry() {
    let mut data = vec![0u8; 4096];
    data[0..8].copy_from_slice(&128u64.to_le_bytes());
    data[26] = 0;
    data[128 + 2] = 9;

    assert_eq!(
        parse_identify_namespace(1, &data, 32).expect("identify namespace"),
        NamespaceInfo {
            namespace_id: 1,
            block_size: 512,
            block_count: 128,
            max_transfer_blocks: 32,
        }
    );
}

#[test]
fn block_server_registers_buffers_and_dispatches_requests() {
    let mut server = BlockDeviceServer::new(namespace());
    let (status, vmo_id) = server.register_buffer(42, 32);
    assert_eq!(status, Status::Ok);
    assert_ne!(vmo_id, 0);

    let response = server.dispatch(BlockRequest {
        req_id: 99,
        opcode: BlockOpcode::Read,
        vmo_id,
        vmo_offset_blocks: 0,
        device_block_offset: 8,
        block_count: 4,
    });
    assert_eq!(response.status, Status::Ok);
    assert_eq!(
        server.dispatched(),
        &[BlockBackendOp::Read {
            req_id: 99,
            vmo_id,
            vmo_offset_blocks: 0,
            device_block_offset: 8,
            block_count: 4,
        }]
    );

    let response = server.dispatch(BlockRequest {
        req_id: 100,
        opcode: BlockOpcode::Flush,
        vmo_id: 0,
        vmo_offset_blocks: 0,
        device_block_offset: 0,
        block_count: 0,
    });
    assert_eq!(response.status, Status::Ok);
    assert!(matches!(
        server.dispatched().last(),
        Some(BlockBackendOp::Flush { req_id: 100 })
    ));
}

#[test]
fn block_server_migration_preserves_large_registered_buffers() {
    let mut source = BlockDeviceServer::new(namespace());
    let (status, vmo_id) = source.register_buffer(42, 128);
    assert_eq!(status, Status::Ok);

    let mut adopted =
        BlockDeviceServer::adopt(&source.checkpoint(), namespace()).expect("adopt block server");
    let response = adopted.dispatch(BlockRequest {
        req_id: 101,
        opcode: BlockOpcode::Read,
        vmo_id,
        vmo_offset_blocks: 120,
        device_block_offset: 0,
        block_count: 8,
    });
    assert_eq!(response.status, Status::Ok);
}

#[test]
fn block_server_rejects_invalid_ranges_and_unsupported_trim() {
    let mut server = BlockDeviceServer::new(namespace());
    let (_, vmo_id) = server.register_buffer(42, 4);

    let invalid_vmo = server.dispatch(BlockRequest {
        req_id: 1,
        opcode: BlockOpcode::Write,
        vmo_id: 999,
        vmo_offset_blocks: 0,
        device_block_offset: 0,
        block_count: 1,
    });
    assert_eq!(invalid_vmo.status, Status::ErrInvalidHandle);

    let too_large = server.dispatch(BlockRequest {
        req_id: 2,
        opcode: BlockOpcode::Write,
        vmo_id,
        vmo_offset_blocks: 2,
        device_block_offset: 0,
        block_count: 4,
    });
    assert_eq!(too_large.status, Status::ErrBufferTooSmall);

    let trim = server.dispatch(BlockRequest {
        req_id: 3,
        opcode: BlockOpcode::Trim,
        vmo_id,
        vmo_offset_blocks: 0,
        device_block_offset: 0,
        block_count: 1,
    });
    assert_eq!(trim.status, Status::ErrInvalidArgs);
}
