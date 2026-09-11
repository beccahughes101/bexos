pub mod migration;
mod packets;

use alloc::format;
use alloc::vec::Vec;

use bexos_userspace::service_binding::{BoundServiceEndpoint, ServiceBinding};
use bexos_userspace::{Channel, HardwareResourceKind, Memory, Startup, log};
use ethernet_fidl::*;
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

use crate::hal;
use crate::hardware::Hardware;
use crate::server::{Buffer, EthernetServer};

pub async fn main(channel: u64) -> ! {
    let control = Channel(channel);
    let start = Startup::receive(control).expect("virtio-net startup");
    let state = if start.migration_target {
        match bexos_userspace::live_migration::receive::<migration::Runtime>(
            control,
            start.migration_generation,
        ) {
            Ok(state) => state,
            Err(error) => {
                log(&format!(
                    "virtio-net: candidate migration rejected {error:?}\n"
                ));
                bexos_userspace::exit()
            }
        }
    } else {
        let iommu_domain = start
            .driver_resources
            .iter()
            .find(|resource| resource.kind == HardwareResourceKind::IommuDomain)
            .map(|resource| resource.handle)
            .expect("virtio-net IOMMU domain");
        hal::set_iommu_domain(iommu_domain);
        let mut hardware = Hardware::connect(start.arg1).expect("virtio-net device");
        hardware.start();
        let mac = hardware.mac_address();
        let server = EthernetServer::new(mac);
        log(&format!(
            "virtio-net: EL0 device ready mac={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}\n",
            mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
        ));
        Startup::ready(control).unwrap();
        let mut runtime = migration::Runtime::new(control, start.migration, hardware, server);
        runtime.lifecycle = start.driver_lifecycle;
        runtime
    };
    serve(state).await
}

async fn serve(mut state: migration::Runtime) -> ! {
    let control = state.control;
    let mut source = bexos_userspace::live_migration::Source::new(state.migration);
    let mut source_error_logged = false;
    loop {
        if let Err(error) = source.poll(&state) {
            if !source_error_logged {
                log(&format!(
                    "virtio-net: source migration rejected {error:?}\n"
                ));
                source_error_logged = true;
            }
            let _ = bexos_userspace::migration::abort();
        }
        if source.quiescing() {
            bexos_userspace::yield_now();
            continue;
        }
        match control.try_recv() {
            Ok(message) => {
                if let (Some(endpoint), Ok(metadata)) = (
                    message.handles.first().copied(),
                    core::str::from_utf8(&message.bytes),
                ) {
                    if let Some(binding) = ServiceBinding::parse(metadata) {
                        if binding.protocol_is("DevicePowerControl") {
                            state.power_endpoints.push(BoundServiceEndpoint::new(
                                Channel(endpoint),
                                binding.method_ordinals,
                            ));
                            source.changed(0);
                            continue;
                        } else if binding.protocol_is("Device") {
                            state.device_endpoints.push(BoundServiceEndpoint::new(
                                Channel(endpoint),
                                binding.method_ordinals,
                            ));
                            source.changed(0);
                            continue;
                        }
                    } else if metadata_protocol(metadata).is_some() {
                        let _ = Memory::close(endpoint);
                        continue;
                    }
                }
                let manager_endpoint =
                    BoundServiceEndpoint::new(control, alloc::vec![1, 2, 3, 4, 5, 6]);
                handle_control(
                    &manager_endpoint,
                    state.server.as_mut().unwrap(),
                    &mut state.fifos,
                    message.bytes,
                    message.handles,
                );
                source.changed_keys([0, 1, 3]);
            }
            Err(kernel_fidl::Status::ErrTimedOut) => {}
            Err(e) => panic!("virtio-net control {e:?}"),
        }
        poll_device_endpoints(
            &mut state.device_endpoints,
            state.server.as_mut().unwrap(),
            &mut state.fifos,
            &mut source,
        );
        poll_lifecycle(state.lifecycle);

        packets::poll(&mut state, &mut source);
        poll_power_endpoints(
            &mut state.power_endpoints,
            state.hardware.as_mut().unwrap(),
            &mut source,
        )
        .await;
        if source.active() && !state.drain_for_quiesce() {
            let _ = bexos_userspace::migration::abort();
        }
        bexos_userspace::yield_now();
    }
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

fn poll_device_endpoints(
    endpoints: &mut Vec<BoundServiceEndpoint>,
    server: &mut EthernetServer,
    fifos: &mut Vec<Channel>,
    source: &mut bexos_userspace::live_migration::Source,
) {
    endpoints.retain(|endpoint| match endpoint.channel.try_recv() {
        Ok(message) => {
            handle_control(endpoint, server, fifos, message.bytes, message.handles);
            source.changed_keys([0, 1, 3]);
            true
        }
        Err(kernel_fidl::Status::ErrPeerClosed) => {
            source.changed(0);
            false
        }
        Err(_) => true,
    });
}

fn handle_control(
    endpoint: &BoundServiceEndpoint,
    server: &mut EthernetServer,
    fifos: &mut Vec<Channel>,
    bytes: Vec<u8>,
    handles: Vec<u64>,
) {
    let (ordinal, req) = envelope(&bytes);
    if !endpoint.allows(ordinal) {
        return;
    }
    let control = endpoint.channel;
    let hs: Vec<_> = handles.iter().map(|h| HandleRef { raw: *h }).collect();
    let mut out = [0u8; 256];
    let mut oh = [HandleRef { raw: 0 }; 4];
    let encoded = match ordinal {
        1 => DeviceGetInfoResponse {
            info: server.info(),
        }
        .encode(&mut out, &mut oh),
        2 => {
            let q = DeviceRegisterBufferRequest::decode(req, &hs).unwrap();
            let rounded = page_round(q.size_bytes as u64).unwrap();
            let result = (|| {
                let vaddr = Memory::map(q.vmo.raw, rounded, 6).map_err(map_kernel_error)?;
                let domain = hal::iommu_domain_handle().ok_or(Status::ErrInvalidArgs)?;
                let (paddr, token) = Memory::map_dma(domain, q.vmo.raw, 0, rounded, 2 | 4)
                    .map_err(map_kernel_error)?;
                if !hal::register_shared_range(q.vmo.raw, vaddr, paddr, token, rounded) {
                    let _ = Memory::unmap_dma(token);
                    let _ = Memory::unmap(vaddr, rounded);
                    return Err(Status::ErrNoMemory);
                }
                match server.register_buffer(Buffer {
                    handle: q.vmo.raw,
                    vaddr,
                    paddr,
                    token,
                    size: q.size_bytes,
                }) {
                    Ok(id) => Ok(id),
                    Err(status) => {
                        let _ = hal::unregister_shared_range(q.vmo.raw);
                        let _ = Memory::unmap_dma(token);
                        let _ = Memory::unmap(vaddr, rounded);
                        let _ = Memory::close(q.vmo.raw);
                        Err(status)
                    }
                }
            })();
            let (status, vmo_id) = match result {
                Ok(id) => (Status::Ok, id),
                Err(status) => (status, 0),
            };
            DeviceRegisterBufferResponse { status, vmo_id }.encode(&mut out, &mut oh)
        }
        3 => {
            let q = DeviceUnregisterBufferRequest::decode(req, &hs).unwrap();
            let status = match server.unregister_buffer(q.vmo_id) {
                Ok(buffer) => {
                    let (vaddr, token, size) = hal::unregister_shared_range(buffer.handle)
                        .unwrap_or((buffer.vaddr, buffer.token, buffer.size as u64));
                    let _ = Memory::unmap_dma(token);
                    let _ = Memory::unmap(vaddr, size);
                    let _ = Memory::close(buffer.handle);
                    Status::Ok
                }
                Err(status) => status,
            };
            DeviceUnregisterBufferResponse { status }.encode(&mut out, &mut oh)
        }
        4 => {
            let (fifo, client) = Channel::pair().unwrap();
            fifos.push(fifo);
            DeviceGetFifoResponse {
                status: Status::Ok,
                fifo_handle: HandleRef { raw: client.0 },
            }
            .encode(&mut out, &mut oh)
        }
        5 => {
            server.start();
            DeviceStartResponse { status: Status::Ok }.encode(&mut out, &mut oh)
        }
        6 => {
            server.stop();
            DeviceStopResponse { status: Status::Ok }.encode(&mut out, &mut oh)
        }
        _ => panic!("unknown ethernet ordinal {ordinal}"),
    }
    .unwrap();
    control
        .send(
            &out[..encoded.bytes],
            &oh[..encoded.handles]
                .iter()
                .map(|handle| handle.raw)
                .collect::<Vec<_>>(),
        )
        .unwrap();
}

pub(super) fn reply_frame(fifo: Channel, entry: FrameEntry) {
    let mut out = [0u8; 64];
    let encoded = entry.encode(&mut out, &mut []).unwrap();
    fifo.send(&out[..encoded.bytes], &[]).unwrap();
}

pub(crate) fn page_round(value: u64) -> Option<u64> {
    value.checked_add(4095).map(|n| n & !4095)
}

async fn poll_power_endpoints(
    endpoints: &mut Vec<BoundServiceEndpoint>,
    hardware: &mut Hardware,
    source: &mut bexos_userspace::live_migration::Source,
) {
    let mut index = 0;
    while index < endpoints.len() {
        let keep = match endpoints[index].channel.try_recv() {
            Ok(message) => {
                source.changed(0);
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
            source.changed(0);
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
    if result.is_ok() {
        PowerStatus::Ok
    } else {
        PowerStatus::ErrInvalidArgs
    }
}

fn map_kernel_error(status: kernel_fidl::Status) -> Status {
    match status {
        kernel_fidl::Status::ErrTimedOut => Status::ErrTimedOut,
        kernel_fidl::Status::ErrNoMemory => Status::ErrNoMemory,
        kernel_fidl::Status::ErrInvalidHandle => Status::ErrInvalidHandle,
        kernel_fidl::Status::ErrAccessDenied => Status::ErrAccessDenied,
        _ => Status::ErrInvalidArgs,
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
