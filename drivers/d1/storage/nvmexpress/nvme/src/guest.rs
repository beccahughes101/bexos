mod migration;
use crate::block::BlockDeviceServer;
use crate::hardware::Hardware;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use bexos_userspace::service_binding::{BoundServiceEndpoint, ServiceBinding};
use bexos_userspace::{Channel, HardwareResourceKind, Memory, Startup, log};
use block_fidl::*;
use hardware_manager_fidl::{
    DriverLifecyclePrepareStopRequest, DriverLifecyclePrepareStopResponse,
    FidlDecode as LifecycleDecode, FidlEncode as LifecycleEncode, HandleRef as LifecycleHandleRef,
    Status as LifecycleStatus,
};
use power_fidl::{
    DevicePowerControlSetPowerStateRequest, DevicePowerControlSetPowerStateResponse,
    DevicePowerState, FidlDecode as PowerDecode, FidlEncode as PowerEncode,
    HandleRef as PowerHandleRef, Status as PowerStatus,
};

const REGISTERED_BUFFER_BYTES: u64 = 512 * 1024;
const REGISTERED_BUFFER_BLOCKS: u64 = REGISTERED_BUFFER_BYTES / 512;

pub async fn main(channel: u64) -> ! {
    let control = Channel(channel);
    let start = match Startup::receive(control) {
        Ok(start) => start,
        Err(error) => {
            log(&alloc::format!("nvme: startup receive failed {error:?}\n"));
            bexos_userspace::exit()
        }
    };
    let state = if start.migration_target {
        match bexos_userspace::live_migration::receive::<migration::Runtime>(
            control,
            start.migration_generation,
        ) {
            Ok(s) => s,
            Err(e) => {
                log(&alloc::format!(
                    "nvme: candidate migration rejected {e:?}\n"
                ));
                bexos_userspace::exit()
            }
        }
    } else {
        let Some(resource) = start
            .driver_resources
            .iter()
            .find(|resource| resource.kind == HardwareResourceKind::Mmio)
            .map(|resource| (resource.handle, resource.length))
            .or_else(|| {
                start
                    .resources
                    .first()
                    .copied()
                    .map(|handle| (handle, start.arg0))
            })
        else {
            log("nvme: startup missing MMIO resource\n");
            bexos_userspace::exit()
        };
        let Some(iommu_domain) = start
            .driver_resources
            .iter()
            .find(|resource| resource.kind == HardwareResourceKind::IommuDomain)
            .map(|resource| resource.handle)
        else {
            log("nvme: startup missing IOMMU domain\n");
            bexos_userspace::exit()
        };
        let mmio = match Memory::map(resource.0, resource.1, 6) {
            Ok(mmio) => mmio,
            Err(error) => {
                log(&alloc::format!("nvme: MMIO map failed {error:?}\n"));
                bexos_userspace::exit()
            }
        };
        log(&alloc::format!(
            "nvme: startup resources ready mmio_bytes={}\n",
            resource.1
        ));
        let hardware = match Hardware::connect(mmio, iommu_domain).await {
            Ok(hardware) => hardware,
            Err(error) => {
                log(&alloc::format!(
                    "nvme: controller connect failed {error:?}\n"
                ));
                bexos_userspace::exit()
            }
        };
        let (sq, cq, payload) = hardware.queue_addresses();
        log(&alloc::format!(
            "nvme: initial queues sq_pa={sq:#x} cq_pa={cq:#x} payload_pa={payload:#x}\n"
        ));
        let server = BlockDeviceServer::new(hardware.info);
        log("nvme: EL0 controller identified; block service ready\n");
        Startup::ready(control).unwrap();
        migration::Runtime {
            control,
            migration: start.migration,
            mmio,
            mmio_handle: resource.0,
            mmio_size: resource.1,
            hardware: Some(hardware),
            server: Some(server),
            buffers: BTreeMap::new(),
            fifos: Vec::new(),
            block_endpoints: Vec::new(),
            lifecycle: start.driver_lifecycle,
        }
    };
    serve(state).await
}
async fn serve(mut state: migration::Runtime) -> ! {
    let control = state.control;
    let mut source = bexos_userspace::live_migration::Source::new(state.migration);
    let mut power_endpoints = Vec::new();
    let mut source_error_logged = false;
    loop {
        if let Err(e) = source.poll(&state) {
            if !source_error_logged {
                log(&alloc::format!("nvme: source migration rejected {e:?}\n"));
                source_error_logged = true;
            }
            let _ = bexos_userspace::migration::abort();
        }
        if source.quiescing() {
            bexos_userspace_async::yield_once().await;
            continue;
        }
        let hardware = state.hardware.as_mut().unwrap();
        let server = state.server.as_mut().unwrap();
        let buffers = &mut state.buffers;
        let fifos = &mut state.fifos;
        match control.try_recv() {
            Ok(m) => {
                if let (Some(endpoint), Ok(metadata)) =
                    (m.handles.first().copied(), core::str::from_utf8(&m.bytes))
                {
                    if let Some(binding) = ServiceBinding::parse(metadata) {
                        if binding.protocol_is("DevicePowerControl") {
                            power_endpoints.push(BoundServiceEndpoint::new(
                                Channel(endpoint),
                                binding.method_ordinals,
                            ));
                        } else if binding.protocol_is("BlockDevice") {
                            state.block_endpoints.push(BoundServiceEndpoint::new(
                                Channel(endpoint),
                                binding.method_ordinals,
                            ));
                            source.changed(3);
                        }
                        continue;
                    } else if metadata_protocol(metadata).is_some() {
                        let _ = Memory::close(endpoint);
                        continue;
                    }
                }
                source.changed_keys([1, 2]);
                handle_block_message(
                    control, server, hardware, buffers, fifos, m.bytes, m.handles,
                );
            }
            Err(kernel_fidl::Status::ErrTimedOut) => {}
            Err(e) => panic!("block control {e:?}"),
        }
        poll_block_endpoints(&mut state.block_endpoints, server, hardware, buffers, fifos);
        poll_lifecycle(state.lifecycle);
        for fifo in fifos.iter() {
            if let Ok(m) = fifo.try_recv() {
                source.changed(0);
                let req = BlockRequest::decode(&m.bytes, &[]).unwrap();
                let mut response = server.dispatch(req);
                if response.status == Status::Ok {
                    let r = if req.opcode == BlockOpcode::Flush {
                        hardware.flush().await
                    } else {
                        let (_, va) = buffers[&req.vmo_id];
                        let bytes = unsafe {
                            core::slice::from_raw_parts_mut(
                                (va + req.vmo_offset_blocks * 512) as *mut u8,
                                req.block_count as usize * 512,
                            )
                        };
                        hardware
                            .transfer(
                                if req.opcode == BlockOpcode::Read {
                                    2
                                } else {
                                    1
                                },
                                req.device_block_offset,
                                bytes,
                            )
                            .await
                    };
                    if let Err(e) = r {
                        response.status = if e == kernel_fidl::Status::ErrTimedOut {
                            Status::ErrTimedOut
                        } else {
                            Status::ErrInvalidArgs
                        };
                    }
                }
                let mut bytes = [0; 32];
                let e = response.encode(&mut bytes, &mut []).unwrap();
                fifo.send(&bytes[..e.bytes], &[]).unwrap();
                server.complete_dispatched();
            }
        }
        poll_power_endpoints(&mut power_endpoints, hardware).await;
        bexos_userspace_async::yield_once().await;
    }
}

async fn poll_power_endpoints(endpoints: &mut Vec<BoundServiceEndpoint>, hardware: &mut Hardware) {
    let mut index = 0;
    while index < endpoints.len() {
        let keep = match endpoints[index].channel.try_recv() {
            Ok(message) => {
                let (ordinal, req) = envelope(&message.bytes);
                let handles: Vec<_> = message
                    .handles
                    .iter()
                    .map(|raw| PowerHandleRef { raw: *raw })
                    .collect();
                let status = if endpoints[index].allows(ordinal) && ordinal == 1 {
                    match DevicePowerControlSetPowerStateRequest::decode(req, &handles) {
                        Ok(request) => set_power_state(hardware, request.state).await,
                        Err(_) => PowerStatus::ErrInvalidArgs,
                    }
                } else {
                    PowerStatus::ErrInvalidArgs
                };
                let mut out = [0; 16];
                let mut out_handles = [PowerHandleRef { raw: 0 }; 1];
                if let Ok(encoded) = (DevicePowerControlSetPowerStateResponse { status })
                    .encode(&mut out, &mut out_handles)
                {
                    let handles: Vec<_> = out_handles[..encoded.handles]
                        .iter()
                        .map(|handle| handle.raw)
                        .collect();
                    let _ = endpoints[index]
                        .channel
                        .send(&out[..encoded.bytes], &handles);
                }
                true
            }
            Err(kernel_fidl::Status::ErrPeerClosed) => false,
            Err(_) => true,
        };
        if keep {
            index += 1;
        } else {
            endpoints.remove(index);
        }
    }
}

fn poll_block_endpoints(
    endpoints: &mut Vec<BoundServiceEndpoint>,
    server: &mut BlockDeviceServer,
    hardware: &mut Hardware,
    buffers: &mut BTreeMap<u32, (u64, u64)>,
    fifos: &mut Vec<Channel>,
) {
    let mut index = 0;
    while index < endpoints.len() {
        let keep = match endpoints[index].channel.try_recv() {
            Ok(message) => {
                handle_block_message(
                    endpoints[index].channel,
                    server,
                    hardware,
                    buffers,
                    fifos,
                    message.bytes,
                    message.handles,
                );
                true
            }
            Err(kernel_fidl::Status::ErrPeerClosed) => false,
            Err(_) => true,
        };
        if keep {
            index += 1;
        } else {
            endpoints.remove(index);
        }
    }
}

fn handle_block_message(
    reply: Channel,
    server: &mut BlockDeviceServer,
    hardware: &mut Hardware,
    buffers: &mut BTreeMap<u32, (u64, u64)>,
    fifos: &mut Vec<Channel>,
    bytes: Vec<u8>,
    handles: Vec<u64>,
) {
    let hs: Vec<_> = handles.iter().map(|h| HandleRef { raw: *h }).collect();
    let (ordinal, req) = envelope(&bytes);
    let mut out = [0; 256];
    let mut oh = [HandleRef { raw: 0 }; 4];
    let encoded = match ordinal {
        1 => BlockDeviceGetInfoResponse {
            info: server.info(),
        }
        .encode(&mut out, &mut oh),
        2 => {
            let q = BlockDeviceRegisterBufferRequest::decode(req, &hs).unwrap();
            let va = Memory::map(q.vmo.raw, REGISTERED_BUFFER_BYTES, 6).expect("block buffer map");
            let (status, id) = server.register_buffer(q.vmo.raw, REGISTERED_BUFFER_BLOCKS);
            buffers.insert(id, (q.vmo.raw, va));
            BlockDeviceRegisterBufferResponse { status, vmo_id: id }.encode(&mut out, &mut oh)
        }
        3 => {
            let q = BlockDeviceUnregisterBufferRequest::decode(req, &hs).unwrap();
            let status = server.unregister_buffer(q.vmo_id);
            if let Some((h, va)) = buffers.remove(&q.vmo_id) {
                Memory::unmap(va, REGISTERED_BUFFER_BYTES).unwrap();
                Memory::close(h).unwrap();
            }
            BlockDeviceUnregisterBufferResponse { status }.encode(&mut out, &mut oh)
        }
        4 => {
            let (fifo, client) = Channel::pair().unwrap();
            fifos.push(fifo);
            BlockDeviceGetFifoResponse {
                status: Status::Ok,
                fifo_handle: HandleRef { raw: client.0 },
            }
            .encode(&mut out, &mut oh)
        }
        5 => {
            let _ = BlockDeviceGetMigrationMarkersRequest::decode(req, &hs).unwrap();
            let (sq, cq, payload) = hardware.queue_addresses();
            BlockDeviceGetMigrationMarkersResponse {
                sq_physical: sq,
                cq_physical: cq,
                payload_physical: payload,
            }
            .encode(&mut out, &mut oh)
        }
        _ => return,
    }
    .unwrap();
    reply
        .send(
            &out[..encoded.bytes],
            &oh[..encoded.handles]
                .iter()
                .map(|h| h.raw)
                .collect::<Vec<_>>(),
        )
        .unwrap();
}

fn poll_lifecycle(lifecycle: Option<Channel>) {
    let Some(lifecycle) = lifecycle else {
        return;
    };
    if let Ok(message) = lifecycle.try_recv() {
        let (_, req) = envelope(&message.bytes);
        let handles: Vec<_> = message
            .handles
            .iter()
            .map(|raw| LifecycleHandleRef { raw: *raw })
            .collect();
        let status = match DriverLifecyclePrepareStopRequest::decode(req, &handles) {
            Ok(_) => LifecycleStatus::Ok,
            Err(_) => LifecycleStatus::ErrInvalidArgs,
        };
        let mut out = [0; 16];
        let mut out_handles = [LifecycleHandleRef { raw: 0 }; 1];
        if let Ok(encoded) =
            (DriverLifecyclePrepareStopResponse { status }).encode(&mut out, &mut out_handles)
        {
            let handles: Vec<_> = out_handles[..encoded.handles]
                .iter()
                .map(|handle| handle.raw)
                .collect();
            let _ = lifecycle.send(&out[..encoded.bytes], &handles);
        }
    }
}

async fn set_power_state(hardware: &mut Hardware, state: DevicePowerState) -> PowerStatus {
    let result = match state {
        DevicePowerState::D0FullPower => hardware.power_up().await,
        DevicePowerState::D3Off => hardware.power_down().await,
        DevicePowerState::D1Standby | DevicePowerState::D2LowPower => {
            return PowerStatus::ErrInvalidArgs;
        }
    };
    match result {
        Ok(()) => PowerStatus::Ok,
        Err(kernel_fidl::Status::ErrTimedOut) => PowerStatus::ErrTimedOut,
        Err(kernel_fidl::Status::ErrNoMemory) => PowerStatus::ErrNoMemory,
        Err(kernel_fidl::Status::ErrInvalidHandle) => PowerStatus::ErrInvalidHandle,
        Err(_) => PowerStatus::ErrInvalidArgs,
    }
}

fn metadata_protocol(metadata: &str) -> Option<&str> {
    metadata.split('|').nth(1)
}

fn envelope(bytes: &[u8]) -> (u64, &[u8]) {
    if bytes.len() < 8 {
        return (0, &[]);
    }
    let mut raw = [0; 8];
    raw.copy_from_slice(&bytes[..8]);
    (u64::from_le_bytes(raw), &bytes[8..])
}
