mod migration;
mod service;
mod wire;

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use bexos_userspace::{Channel, Memory, Startup, fs, log};
use block_fidl::*;
use diskimage_fidl as diskimage;
use diskimage_fidl::{FidlDecode as DiskImageDecode, FidlEncode as DiskImageEncode};

use crate::block::{BackingFile, DISKIMAGE_BLOCK_SIZE, DiskImageServer, RegisteredBuffer};

pub(crate) struct Attachment {
    control: Channel,
    server: DiskImageServer<FsBackingFile>,
    buffers: BTreeMap<u32, (u64, u64, u64)>,
    fifos: Vec<Channel>,
    key_handle: u64,
}

pub(crate) struct FsBackingFile {
    file: Channel,
    owned: bool,
}

impl Drop for Attachment {
    fn drop(&mut self) {
        if self.key_handle != 0 {
            let _ = Memory::close(self.key_handle);
            self.key_handle = 0;
        }
    }
}

impl Drop for FsBackingFile {
    fn drop(&mut self) {
        if self.owned {
            let _ = fs::close(self.file);
        }
    }
}

impl BackingFile for FsBackingFile {
    fn len(&self) -> Result<u64, Status> {
        fs::attributes(self.file)
            .map(|attrs| attrs.size_bytes)
            .map_err(block_status)
    }

    fn read_at(&mut self, offset: u64, out: &mut [u8]) -> Result<(), Status> {
        let offset = i64::try_from(offset).map_err(|_| Status::ErrInvalidArgs)?;
        fs::seek(self.file, offset).map_err(block_status)?;
        let bytes = fs::read(self.file, out.len() as u64).map_err(block_status)?;
        if bytes.len() != out.len() {
            return Err(Status::ErrBufferTooSmall);
        }
        out.copy_from_slice(&bytes);
        Ok(())
    }

    fn write_at(&mut self, offset: u64, data: &[u8]) -> Result<(), Status> {
        let offset = i64::try_from(offset).map_err(|_| Status::ErrInvalidArgs)?;
        fs::seek(self.file, offset).map_err(block_status)?;
        fs::write(self.file, data).map_err(block_status)
    }

    fn sync(&mut self) -> Result<(), Status> {
        fs::sync_file(self.file).map_err(block_status)
    }
}

impl DiskImageServer<FsBackingFile> {
    pub(crate) fn file_handle(&self) -> Channel {
        self.file().file
    }
}

pub async fn main(channel: u64) -> ! {
    let control = Channel(channel);
    let start = Startup::receive(control).unwrap();
    let state = if start.migration_target {
        match bexos_userspace::live_migration::receive::<migration::Runtime>(
            control,
            start.migration_generation,
        ) {
            Ok(state) => state,
            Err(error) => {
                log(&alloc::format!(
                    "diskimage: candidate migration rejected {error:?}\n"
                ));
                bexos_userspace::exit()
            }
        }
    } else {
        log("diskimage: EL0 virtual block service ready\n");
        Startup::ready(control).unwrap();
        migration::Runtime {
            control,
            migration: start.migration,
            attachments: Vec::new(),
        }
    };
    service::serve(state).await
}

pub(crate) async fn serve_inner(mut state: migration::Runtime) -> ! {
    use bexos_userspace::live_migration::State;

    let control = state.control;
    let mut source = bexos_userspace::live_migration::Source::new(state.migration);
    let mut source_error_logged = false;
    loop {
        if let Err(error) = source.poll(&state) {
            if !source_error_logged {
                log(&alloc::format!(
                    "diskimage: source migration rejected {error:?}\n"
                ));
                source_error_logged = true;
            }
            let _ = bexos_userspace::migration::abort();
        }
        if source.quiescing() {
            bexos_userspace::yield_now();
            continue;
        }
        let old_attachments = state.attachments.len();
        if let Ok(message) = control.try_recv() {
            let (ordinal, req) = envelope(&message.bytes);
            let handles = diskimage_refs(&message.handles);
            let mut out = [0; 192];
            let mut out_handles = [diskimage::HandleRef { raw: 0 }; 4];
            let encoded = match ordinal {
                1 => {
                    let q = diskimage::DiskImageManagerAttachRequest::decode(req, &handles)
                        .expect("diskimage attach decode");
                    let file = FsBackingFile {
                        file: Channel(q.file_handle.raw),
                        owned: true,
                    };
                    let result = DiskImageServer::attach(file, q.read_only);
                    let (status, block_device) = match result {
                        Ok(server) => {
                            let (client, server_channel) = Channel::pair().unwrap();
                            state.attachments.push(Attachment {
                                control: server_channel,
                                server,
                                buffers: BTreeMap::new(),
                                fifos: Vec::new(),
                                key_handle: 0,
                            });
                            (diskimage::Status::Ok, client.0)
                        }
                        Err(status) => (diskimage_status(status), 0),
                    };
                    diskimage::DiskImageManagerAttachResponse {
                        status,
                        block_device: block_device_binding(block_device),
                    }
                    .encode(&mut out, &mut out_handles)
                }
                2 => {
                    let q =
                        diskimage::DiskImageManagerCreateEncryptedRequest::decode(req, &handles)
                            .expect("diskimage create encrypted decode");
                    let file = FsBackingFile {
                        file: Channel(q.file_handle.raw),
                        owned: true,
                    };
                    let (key_status, key) = read_key_vmo(q.key_vmo.raw);
                    let result = key
                        .as_ref()
                        .map(|(key, _)| {
                            DiskImageServer::create_encrypted(
                                file,
                                key,
                                q.image_uuid,
                                q.virtual_size_bytes,
                            )
                        })
                        .unwrap_or(Err(key_status));
                    let (status, block_device, image_uuid, virtual_size_bytes) = match result {
                        Ok(server) => {
                            let info = server.encrypted_info().expect("encrypted image info");
                            let (client, server_channel) = Channel::pair().unwrap();
                            state.attachments.push(Attachment {
                                control: server_channel,
                                server,
                                buffers: BTreeMap::new(),
                                fifos: Vec::new(),
                                key_handle: key.map(|(_, handle)| handle).unwrap_or(0),
                            });
                            (
                                diskimage::Status::Ok,
                                client.0,
                                info.image_uuid,
                                info.virtual_size_bytes,
                            )
                        }
                        Err(status) => {
                            if let Some((_, handle)) = key {
                                let _ = Memory::close(handle);
                            }
                            (diskimage_status(status), 0, [0; 16], 0)
                        }
                    };
                    diskimage::DiskImageManagerCreateEncryptedResponse {
                        status,
                        block_device: block_device_binding(block_device),
                        image_uuid,
                        virtual_size_bytes,
                    }
                    .encode(&mut out, &mut out_handles)
                }
                3 => {
                    let q = diskimage::DiskImageManagerOpenEncryptedRequest::decode(req, &handles)
                        .expect("diskimage open encrypted decode");
                    let file = FsBackingFile {
                        file: Channel(q.file_handle.raw),
                        owned: true,
                    };
                    let (key_status, key) = read_key_vmo(q.key_vmo.raw);
                    let result = key
                        .as_ref()
                        .map(|(key, _)| DiskImageServer::open_encrypted(file, key, q.read_only))
                        .unwrap_or(Err(key_status));
                    let (status, block_device, image_uuid, virtual_size_bytes) = match result {
                        Ok(server) => {
                            let info = server.encrypted_info().expect("encrypted image info");
                            let (client, server_channel) = Channel::pair().unwrap();
                            state.attachments.push(Attachment {
                                control: server_channel,
                                server,
                                buffers: BTreeMap::new(),
                                fifos: Vec::new(),
                                key_handle: key.map(|(_, handle)| handle).unwrap_or(0),
                            });
                            (
                                diskimage::Status::Ok,
                                client.0,
                                info.image_uuid,
                                info.virtual_size_bytes,
                            )
                        }
                        Err(status) => {
                            if let Some((_, handle)) = key {
                                let _ = Memory::close(handle);
                            }
                            (diskimage_status(status), 0, [0; 16], 0)
                        }
                    };
                    diskimage::DiskImageManagerOpenEncryptedResponse {
                        status,
                        block_device: block_device_binding(block_device),
                        image_uuid,
                        virtual_size_bytes,
                    }
                    .encode(&mut out, &mut out_handles)
                }
                _ => panic!("unknown diskimage ordinal"),
            }
            .unwrap();
            control
                .send(
                    &out[..encoded.bytes],
                    &out_handles[..encoded.handles]
                        .iter()
                        .map(|handle| handle.raw)
                        .collect::<Vec<_>>(),
                )
                .unwrap();
        }
        for attachment in state.attachments.iter_mut() {
            poll_attachment(attachment, &mut source);
        }
        if state.attachments.len() != old_attachments {
            source.changed(0);
            source.changed_keys(
                state
                    .keys()
                    .into_iter()
                    .filter(|key| *key >> 40 > old_attachments as u64),
            );
        }
        bexos_userspace::yield_now();
    }
}

fn block_device_binding(handle: u64) -> Option<diskimage::BlockDeviceBinding> {
    (handle != 0).then_some(diskimage::BlockDeviceBinding {
        endpoint: diskimage::HandleRef { raw: handle },
    })
}

fn read_key_vmo(handle: u64) -> (Status, Option<([u8; 32], u64)>) {
    let va = match Memory::map(handle, 4096, 2) {
        Ok(va) => va,
        Err(_) => {
            let _ = Memory::close(handle);
            return (Status::ErrAccessDenied, None);
        }
    };
    let mut key = [0u8; 32];
    key.copy_from_slice(unsafe { core::slice::from_raw_parts(va as *const u8, 32) });
    // The caller grants a read-only key. Handover retains that authority;
    // requesting execute permission here would reject the valid key VMO.
    let retained = match Memory::duplicate(handle, 1 | 2 | 16 | 32) {
        Ok(retained) => retained,
        Err(_) => {
            key.fill(0);
            let _ = Memory::unmap(va, 4096);
            let _ = Memory::close(handle);
            return (Status::ErrNoMemory, None);
        }
    };
    let _ = Memory::unmap(va, 4096);
    let _ = Memory::close(handle);
    (Status::Ok, Some((key, retained)))
}

fn poll_attachment(
    attachment: &mut Attachment,
    source: &mut bexos_userspace::live_migration::Source,
) {
    if let Ok(message) = attachment.control.try_recv() {
        source.changed(attachment_key(attachment.control));
        let (ordinal, req) = envelope(&message.bytes);
        let handles = block_refs(&message.handles);
        match ordinal {
            1 => block_reply(
                attachment.control,
                &BlockDeviceGetInfoResponse {
                    info: attachment.server.info(),
                },
            ),
            2 => {
                let q = BlockDeviceRegisterBufferRequest::decode(req, &handles).unwrap();
                let blocks = 1;
                let size = u64::from(DISKIMAGE_BLOCK_SIZE) * blocks;
                let va = Memory::map(q.vmo.raw, size, 6).expect("diskimage buffer map");
                let (status, vmo_id) = attachment.server.register_buffer(q.vmo.raw, blocks);
                if status == Status::Ok {
                    attachment.buffers.insert(vmo_id, (q.vmo.raw, va, blocks));
                } else {
                    let _ = Memory::unmap(va, size);
                    let _ = Memory::close(q.vmo.raw);
                }
                block_reply(
                    attachment.control,
                    &BlockDeviceRegisterBufferResponse { status, vmo_id },
                );
            }
            3 => {
                let q = BlockDeviceUnregisterBufferRequest::decode(req, &handles).unwrap();
                let status = attachment.server.unregister_buffer(q.vmo_id);
                if let Some((handle, va, blocks)) = attachment.buffers.remove(&q.vmo_id) {
                    let _ = Memory::unmap(va, u64::from(DISKIMAGE_BLOCK_SIZE) * blocks);
                    let _ = Memory::close(handle);
                }
                block_reply(
                    attachment.control,
                    &BlockDeviceUnregisterBufferResponse { status },
                );
            }
            4 => {
                let (fifo, client) = Channel::pair().unwrap();
                attachment.fifos.push(fifo);
                block_reply(
                    attachment.control,
                    &BlockDeviceGetFifoResponse {
                        status: Status::Ok,
                        fifo_handle: HandleRef { raw: client.0 },
                    },
                );
            }
            5 => {
                let _ = BlockDeviceGetMigrationMarkersRequest::decode(req, &handles).unwrap();
                block_reply(
                    attachment.control,
                    &BlockDeviceGetMigrationMarkersResponse {
                        sq_physical: 0,
                        cq_physical: 0,
                        payload_physical: 0,
                    },
                );
            }
            _ => panic!("unknown block ordinal"),
        }
    }
    for fifo in attachment.fifos.iter() {
        if let Ok(message) = fifo.try_recv() {
            source.changed(attachment_key(attachment.control));
            let request = BlockRequest::decode(&message.bytes, &[]).unwrap();
            let mut empty = [];
            let buffer = if request.opcode == BlockOpcode::Flush {
                &mut empty[..]
            } else {
                let (_, va, blocks) = attachment.buffers[&request.vmo_id];
                unsafe {
                    core::slice::from_raw_parts_mut(
                        va as *mut u8,
                        DISKIMAGE_BLOCK_SIZE as usize * blocks as usize,
                    )
                }
            };
            let response = attachment.server.dispatch(request, buffer);
            let mut bytes = [0; 32];
            let encoded = response.encode(&mut bytes, &mut []).unwrap();
            fifo.send(&bytes[..encoded.bytes], &[]).unwrap();
        }
    }
}

fn attachment_key(channel: Channel) -> u64 {
    migration::ATTACHMENT | channel.0
}

fn envelope(bytes: &[u8]) -> (u64, &[u8]) {
    wire::envelope(bytes)
}

fn diskimage_refs(handles: &[u64]) -> Vec<diskimage::HandleRef> {
    handles
        .iter()
        .map(|handle| diskimage::HandleRef { raw: *handle })
        .collect()
}

fn block_refs(handles: &[u64]) -> Vec<HandleRef> {
    handles
        .iter()
        .map(|handle| HandleRef { raw: *handle })
        .collect()
}

fn block_reply<Q: FidlEncode>(channel: Channel, response: &Q) {
    let mut out = [0; 128];
    let mut handles = [HandleRef { raw: 0 }; 4];
    let encoded = response.encode(&mut out, &mut handles).unwrap();
    channel
        .send(
            &out[..encoded.bytes],
            &handles[..encoded.handles]
                .iter()
                .map(|handle| handle.raw)
                .collect::<Vec<_>>(),
        )
        .unwrap();
}

fn block_status(status: fs_fidl::FsStatus) -> Status {
    match status {
        fs_fidl::FsStatus::Ok => Status::Ok,
        fs_fidl::FsStatus::AccessDenied | fs_fidl::FsStatus::ReadOnly => Status::ErrAccessDenied,
        fs_fidl::FsStatus::InvalidArgs => Status::ErrInvalidArgs,
        fs_fidl::FsStatus::NoSpace => Status::ErrNoMemory,
        fs_fidl::FsStatus::BadState => Status::ErrInvalidHandle,
        _ => Status::ErrInvalidArgs,
    }
}

fn diskimage_status(status: Status) -> diskimage::Status {
    match status {
        Status::Ok => diskimage::Status::Ok,
        Status::ErrInvalidHandle => diskimage::Status::ErrInvalidHandle,
        Status::ErrAccessDenied => diskimage::Status::ErrAccessDenied,
        Status::ErrNoMemory => diskimage::Status::ErrNoMemory,
        Status::ErrBufferTooSmall => diskimage::Status::ErrBufferTooSmall,
        Status::ErrPeerClosed => diskimage::Status::ErrPeerClosed,
        Status::ErrTimedOut => diskimage::Status::ErrTimedOut,
        Status::ErrAlreadyExists => diskimage::Status::ErrAlreadyExists,
        Status::ErrInvalidArgs => diskimage::Status::ErrInvalidArgs,
    }
}
