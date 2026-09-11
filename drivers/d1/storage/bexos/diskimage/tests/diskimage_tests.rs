use bexos_d1_diskimage::block::{BackingFile, DISKIMAGE_BLOCK_SIZE, DiskImageServer};
use block_fidl::{BlockFlags, BlockOpcode, BlockRequest, Status};

#[derive(Debug)]
struct MemFile {
    bytes: Vec<u8>,
    syncs: u64,
}

impl MemFile {
    fn new(blocks: u64) -> Self {
        Self {
            bytes: vec![0; blocks as usize * DISKIMAGE_BLOCK_SIZE as usize],
            syncs: 0,
        }
    }
}

impl BackingFile for MemFile {
    fn len(&self) -> Result<u64, Status> {
        Ok(self.bytes.len() as u64)
    }

    fn read_at(&mut self, offset: u64, out: &mut [u8]) -> Result<(), Status> {
        let offset = usize::try_from(offset).map_err(|_| Status::ErrInvalidArgs)?;
        out.copy_from_slice(&self.bytes[offset..offset + out.len()]);
        Ok(())
    }

    fn write_at(&mut self, offset: u64, data: &[u8]) -> Result<(), Status> {
        let offset = usize::try_from(offset).map_err(|_| Status::ErrInvalidArgs)?;
        if offset + data.len() > self.bytes.len() {
            self.bytes.resize(offset + data.len(), 0);
        }
        self.bytes[offset..offset + data.len()].copy_from_slice(data);
        Ok(())
    }

    fn sync(&mut self) -> Result<(), Status> {
        self.syncs += 1;
        Ok(())
    }
}

#[test]
fn attach_rejects_empty_and_unaligned_backing_files() {
    assert!(matches!(
        DiskImageServer::attach(
            MemFile {
                bytes: Vec::new(),
                syncs: 0,
            },
            false
        ),
        Err(Status::ErrInvalidArgs)
    ));
    assert!(matches!(
        DiskImageServer::attach(
            MemFile {
                bytes: vec![0; DISKIMAGE_BLOCK_SIZE as usize + 1],
                syncs: 0,
            },
            false
        ),
        Err(Status::ErrInvalidArgs)
    ));
}

#[test]
fn info_reports_4k_geometry_and_read_only_flag() {
    let server = DiskImageServer::attach(MemFile::new(3), true).unwrap();
    let info = server.info();
    assert_eq!(info.block_size, DISKIMAGE_BLOCK_SIZE);
    assert_eq!(info.block_count, 3);
    assert_eq!(info.max_transfer_blocks, 1);
    assert_eq!(info.flags, BlockFlags::READ_ONLY);
}

#[test]
fn reads_and_writes_blocks_by_file_offset() {
    let mut server = DiskImageServer::attach(MemFile::new(2), false).unwrap();
    let (status, vmo_id) = server.register_buffer(42, 1);
    assert_eq!(status, Status::Ok);
    let mut buffer = [0x5a; DISKIMAGE_BLOCK_SIZE as usize];
    let write = server.dispatch(
        BlockRequest {
            req_id: 1,
            opcode: BlockOpcode::Write,
            vmo_id,
            vmo_offset_blocks: 0,
            device_block_offset: 1,
            block_count: 1,
        },
        &mut buffer,
    );
    assert_eq!(write.status, Status::Ok);

    buffer.fill(0);
    let read = server.dispatch(
        BlockRequest {
            req_id: 2,
            opcode: BlockOpcode::Read,
            vmo_id,
            vmo_offset_blocks: 0,
            device_block_offset: 1,
            block_count: 1,
        },
        &mut buffer,
    );
    assert_eq!(read.status, Status::Ok);
    assert_eq!(buffer, [0x5a; DISKIMAGE_BLOCK_SIZE as usize]);
}

#[test]
fn read_only_trim_and_out_of_range_requests_are_rejected() {
    let mut server = DiskImageServer::attach(MemFile::new(1), true).unwrap();
    let (_, vmo_id) = server.register_buffer(42, 1);
    let mut buffer = [0; DISKIMAGE_BLOCK_SIZE as usize];

    assert_eq!(
        server
            .dispatch(
                BlockRequest {
                    req_id: 1,
                    opcode: BlockOpcode::Write,
                    vmo_id,
                    vmo_offset_blocks: 0,
                    device_block_offset: 0,
                    block_count: 1,
                },
                &mut buffer,
            )
            .status,
        Status::ErrAccessDenied
    );
    assert_eq!(
        server
            .dispatch(
                BlockRequest {
                    req_id: 2,
                    opcode: BlockOpcode::Trim,
                    vmo_id,
                    vmo_offset_blocks: 0,
                    device_block_offset: 0,
                    block_count: 1,
                },
                &mut buffer,
            )
            .status,
        Status::ErrInvalidArgs
    );
    assert_eq!(
        server
            .dispatch(
                BlockRequest {
                    req_id: 3,
                    opcode: BlockOpcode::Read,
                    vmo_id,
                    vmo_offset_blocks: 0,
                    device_block_offset: 1,
                    block_count: 1,
                },
                &mut buffer,
            )
            .status,
        Status::ErrInvalidArgs
    );
}

#[test]
fn flush_calls_backing_sync() {
    let mut server = DiskImageServer::attach(MemFile::new(1), false).unwrap();
    let mut empty = [];
    let response = server.dispatch(
        BlockRequest {
            req_id: 7,
            opcode: BlockOpcode::Flush,
            vmo_id: 0,
            vmo_offset_blocks: 0,
            device_block_offset: 0,
            block_count: 0,
        },
        &mut empty,
    );
    assert_eq!(response.status, Status::Ok);
}

#[test]
fn encrypted_images_round_trip_and_hide_unwritten_sectors() {
    let key = [0x11; 32];
    let uuid = [0x22; 16];
    let mut server =
        DiskImageServer::create_encrypted(MemFile::new(0), &key, uuid, 3 * 4096).unwrap();
    let (_, vmo_id) = server.register_buffer(42, 1);

    let mut buffer = [0x7c; DISKIMAGE_BLOCK_SIZE as usize];
    assert_eq!(
        server
            .dispatch(
                BlockRequest {
                    req_id: 1,
                    opcode: BlockOpcode::Write,
                    vmo_id,
                    vmo_offset_blocks: 0,
                    device_block_offset: 1,
                    block_count: 1,
                },
                &mut buffer,
            )
            .status,
        Status::Ok
    );
    assert!(
        !server
            .file()
            .bytes
            .windows(32)
            .any(|window| window == [0x7c; 32])
    );

    buffer.fill(0);
    assert_eq!(
        server
            .dispatch(
                BlockRequest {
                    req_id: 2,
                    opcode: BlockOpcode::Read,
                    vmo_id,
                    vmo_offset_blocks: 0,
                    device_block_offset: 2,
                    block_count: 1,
                },
                &mut buffer,
            )
            .status,
        Status::Ok
    );
    assert_eq!(buffer, [0; DISKIMAGE_BLOCK_SIZE as usize]);

    assert_eq!(
        server
            .dispatch(
                BlockRequest {
                    req_id: 3,
                    opcode: BlockOpcode::Flush,
                    vmo_id: 0,
                    vmo_offset_blocks: 0,
                    device_block_offset: 0,
                    block_count: 0,
                },
                &mut [],
            )
            .status,
        Status::Ok
    );
    let file = server.file().bytes.clone();
    let mut reopened = DiskImageServer::open_encrypted(
        MemFile {
            bytes: file,
            syncs: 0,
        },
        &key,
        false,
    )
    .unwrap();
    let (_, reopened_vmo) = reopened.register_buffer(43, 1);
    buffer.fill(0);
    assert_eq!(
        reopened
            .dispatch(
                BlockRequest {
                    req_id: 4,
                    opcode: BlockOpcode::Read,
                    vmo_id: reopened_vmo,
                    vmo_offset_blocks: 0,
                    device_block_offset: 1,
                    block_count: 1,
                },
                &mut buffer,
            )
            .status,
        Status::Ok
    );
    assert_eq!(buffer, [0x7c; DISKIMAGE_BLOCK_SIZE as usize]);
}

#[test]
fn encrypted_images_fail_closed_for_wrong_key_and_tamper() {
    let key = [0x33; 32];
    let uuid = [0x44; 16];
    let mut server =
        DiskImageServer::create_encrypted(MemFile::new(0), &key, uuid, 2 * 4096).unwrap();
    let (_, vmo_id) = server.register_buffer(42, 1);
    let mut buffer = [0x91; DISKIMAGE_BLOCK_SIZE as usize];
    assert_eq!(
        server
            .dispatch(
                BlockRequest {
                    req_id: 1,
                    opcode: BlockOpcode::Write,
                    vmo_id,
                    vmo_offset_blocks: 0,
                    device_block_offset: 0,
                    block_count: 1,
                },
                &mut buffer,
            )
            .status,
        Status::Ok
    );
    assert_eq!(
        server
            .dispatch(
                BlockRequest {
                    req_id: 2,
                    opcode: BlockOpcode::Flush,
                    vmo_id: 0,
                    vmo_offset_blocks: 0,
                    device_block_offset: 0,
                    block_count: 0,
                },
                &mut [],
            )
            .status,
        Status::Ok
    );
    let file = server.file().bytes.clone();
    assert!(matches!(
        DiskImageServer::open_encrypted(
            MemFile {
                bytes: file.clone(),
                syncs: 0
            },
            &[0x34; 32],
            false
        ),
        Err(Status::ErrAccessDenied)
    ));

    let mut header_tampered = file.clone();
    header_tampered[64] ^= 1;
    assert!(
        DiskImageServer::open_encrypted(
            MemFile {
                bytes: header_tampered,
                syncs: 0,
            },
            &key,
            false
        )
        .is_err()
    );

    let mut sector_tampered = file;
    let data_offset = 8192usize;
    sector_tampered[data_offset] ^= 1;
    let mut reopened = DiskImageServer::open_encrypted(
        MemFile {
            bytes: sector_tampered,
            syncs: 0,
        },
        &key,
        false,
    )
    .unwrap();
    let (_, vmo_id) = reopened.register_buffer(43, 1);
    assert_eq!(
        reopened
            .dispatch(
                BlockRequest {
                    req_id: 3,
                    opcode: BlockOpcode::Read,
                    vmo_id,
                    vmo_offset_blocks: 0,
                    device_block_offset: 0,
                    block_count: 1,
                },
                &mut buffer,
            )
            .status,
        Status::ErrAccessDenied
    );
}
