mod aperture;
mod command;
mod discovery;
pub mod gpu_state;
pub mod gpu_transport;
pub mod hardware;
mod host_memory;
mod present;
mod scanout;
pub mod scanout_state;
pub mod state;
use bexos_graphics_runtime::{self as rt, migration::Runtime};
use bexos_userspace::{Channel, HardwareResourceKind, Memory, Startup, live_migration::Source};
use graphics_fidl::*;
use state::Display;
pub async fn main(channel: u64) -> ! {
    let control = Channel(channel);
    let start = Startup::receive(control).expect("GPU startup");
    let mut runtime = if start.migration_target {
        bexos_userspace::live_migration::receive::<Runtime<Display>>(
            control,
            start.migration_generation,
        )
        .unwrap_or_else(|_| bexos_userspace::exit())
    } else {
        let domain = start
            .driver_resources
            .iter()
            .find(|r| r.kind == HardwareResourceKind::IommuDomain)
            .map(|r| r.handle)
            .unwrap_or(0);
        bexos_virtio_hal::set_iommu_domain(domain);
        let hardware = match hardware::Hardware::new(start.arg1) {
            Ok(h) => Some(h),
            Err(e) => {
                bexos_userspace::log(&format!(
                    "virtio-gpu: initialization failed {e:?} node={} domain={domain}\n",
                    start.arg1
                ));
                None
            }
        };
        // Optional display failure must still complete bootstrap readiness.
        Startup::ready(control).unwrap();
        if let Some(h) = hardware.as_ref().filter(|h| h.healthy) {
            bexos_userspace::log(&format!(
                "virtio-gpu: ready {}x{}\n",
                h.surface.width, h.surface.height
            ));
        } else {
            bexos_userspace::log("virtio-gpu: unavailable; continuing boot\n");
        }
        Runtime::new(
            control,
            start.migration,
            Display {
                gpu: crate::gpu_state::GpuState {
                    aperture: hardware.as_ref().map_or(Default::default(), |h| h.aperture),
                    ..Default::default()
                },
                hardware,
                domain,
                lifecycle: start.driver_lifecycle.map_or(0, |c| c.0),
                ..Default::default()
            },
        )
    };
    let control = runtime.control;
    let mut source = Source::new(runtime.migration);
    let mut waiters = Vec::with_capacity(67);
    loop {
        if source.poll(&runtime).is_err() {
            let _ = bexos_userspace::migration::abort();
        }
        if source.quiescing() {
            bexos_userspace::yield_now();
            continue;
        }
        if runtime.component.pending_reply.is_some() {
            present::poll(&mut runtime.component);
            source.changed(1);
        }
        if runtime.component.pending_gpu.is_some() {
            gpu_transport::poll(&mut runtime.component);
            source.changed(1);
        }
        if runtime.component.pending_scanout.is_some() {
            scanout::poll(&mut runtime.component);
            source.changed(1);
        }
        let accepting = runtime.component.pending_scanout.is_none()
            && runtime.component.pending_reply.is_none()
            && runtime.component.pending_gpu.is_none()
            && !source.draining();
        if accepting && runtime.component.lifecycle != 0 {
            if let Ok(message) = Channel(runtime.component.lifecycle).try_recv() {
                use hardware_manager_fidl::{FidlDecode, FidlEncode};
                let (_, body) = rt::envelope(&message.bytes).unwrap_or((0, &message.bytes));
                let refs: Vec<_> = message
                    .handles
                    .iter()
                    .map(|h| hardware_manager_fidl::HandleRef { raw: *h })
                    .collect();
                let status = if hardware_manager_fidl::DriverLifecyclePrepareStopRequest::decode(
                    body, &refs,
                )
                .is_ok()
                {
                    if let Some(h) = &mut runtime.component.hardware {
                        h.healthy = false;
                    }
                    source.changed(1);
                    hardware_manager_fidl::Status::Ok
                } else {
                    hardware_manager_fidl::Status::ErrInvalidArgs
                };
                let mut out = [0; 32];
                let mut handles = [];
                if let Ok(n) =
                    (hardware_manager_fidl::DriverLifecyclePrepareStopResponse { status })
                        .encode(&mut out, &mut handles)
                {
                    let _ = Channel(runtime.component.lifecycle).send(&out[..n.bytes], &[]);
                }
                rt::close(&message.handles);
            }
        }
        if runtime.bindings("DisplayCoordinator") {
            source.changed(0);
        }
        if runtime.component.pending_reply.is_none()
            && runtime.component.ownership.expire(rt::now_us())
        {
            source.changed(1);
        }
        let mut ids = [0; 64];
        let count = runtime.clients.len();
        for (id, client) in ids.iter_mut().zip(&runtime.clients) {
            *id = client.channel.0;
        }
        if accepting {
            runtime.clients.retain(|client| {
                if runtime.component.pending_reply.is_some()
                    || runtime.component.pending_gpu.is_some()
                    || runtime.component.pending_scanout.is_some()
                {
                    return true;
                }
                match client.channel.try_recv() {
                    Ok(m) => {
                        handle(&mut runtime.component, client, &ids[..count], m);
                        source.changed(1);
                        true
                    }
                    Err(kernel_fidl::Status::ErrPeerClosed) => {
                        runtime.component.ownership.disconnected(client.channel.0);
                        source.changed(0);
                        source.changed(1);
                        let _ = Memory::close(client.channel.0);
                        false
                    }
                    Err(_) => true,
                }
            });
            let count = runtime.clients.len();
            for (id, client) in ids.iter_mut().zip(&runtime.clients) {
                *id = client.channel.0;
            }
            scanout::cleanup(&mut runtime.component, &ids[..count]);
            gpu_transport::cleanup(&mut runtime.component, &ids[..count]);
            if runtime.component.pending_gpu.is_some()
                || runtime.component.pending_scanout.is_some()
            {
                source.changed(1);
            }
        }
        waiters.clear();
        waiters.push(control);
        waiters.extend(runtime.migration);
        if accepting {
            waiters.extend(runtime.clients.iter().map(|c| c.channel));
        }
        if accepting && runtime.component.lifecycle != 0 {
            waiters.push(Channel(runtime.component.lifecycle));
        }
        rt::wait(&waiters, rt::now_us().saturating_add(1_000));
    }
}
fn handle(
    d: &mut Display,
    c: &bexos_userspace::service_binding::BoundServiceEndpoint,
    ids: &[u64],
    m: bexos_userspace::ipc::Message,
) {
    let Some((ordinal, bytes)) = rt::envelope(&m.bytes) else {
        rt::close(&m.handles);
        return;
    };
    let hs = rt::refs(&m.handles);
    if !c.allows(ordinal) {
        rt::close(&m.handles);
        return;
    }
    if discovery::handle(d, c.channel, ordinal, bytes, &m.handles) {
        rt::close(&m.handles);
        return;
    }
    if gpu_transport::handle(d, c.channel.0, ordinal, bytes, &m.handles) {
        rt::close(&m.handles);
        return;
    }
    if scanout::handle(d, c.channel.0, ordinal, bytes, &m.handles) {
        rt::close(&m.handles);
        return;
    }
    let surface = d
        .hardware
        .as_ref()
        .map(|h| rt::wire(h.surface))
        .unwrap_or(Surface {
            width: 0,
            height: 0,
            stride: 0,
            format: 1,
        });
    let ready = if d.hardware.as_ref().is_some_and(|h| h.healthy) {
        Status::Ok
    } else {
        Status::ErrIo
    };
    match ordinal {
        1 => rt::reply(
            c.channel,
            &DisplayCoordinatorGetInfoResponse {
                status: ready,
                surface,
                client_id: c.channel.0,
                generation: d.ownership.generation,
                owner_id: d.ownership.owner,
                transfer_pending: d.ownership.pending.is_some(),
            },
        ),
        2 => {
            let status = if ready == Status::Ok && d.ownership.acquire(c.channel.0).is_ok() {
                Status::Ok
            } else {
                Status::ErrAccessDenied
            };
            rt::reply(
                c.channel,
                &DisplayCoordinatorAcquireResponse {
                    status,
                    generation: d.ownership.generation,
                },
            );
        }
        3 => {
            let status = (|| {
                let q = DisplayCoordinatorPresentRequest::decode(bytes, &hs)
                    .map_err(|_| Status::ErrInvalidArgs)?;
                d.ownership
                    .authorize(c.channel.0, q.generation)
                    .map_err(|_| Status::ErrAccessDenied)?;
                let surface = rt::surface(q.surface).map_err(|_| Status::ErrInvalidArgs)?;
                let len = surface
                    .validate(u64::MAX)
                    .map_err(|_| Status::ErrInvalidArgs)?;
                let map = Memory::duplicate(q.buffer.raw, 1 | 2 | 16 | 32)
                    .and_then(|h| rt::Mapping::map(h, len as u64, 2))
                    .map_err(|_| Status::ErrIo)?;
                let h = d.hardware.as_mut().ok_or(Status::ErrIo)?;
                if d.ownership.pending.is_some() && !h.matches_front(map.bytes(), surface) {
                    return Err(Status::ErrInvalidArgs);
                }
                h.begin_present(
                    map.bytes(),
                    surface,
                    bexos_graphics::Damage {
                        x: q.damage.x,
                        y: q.damage.y,
                        width: q.damage.width,
                        height: q.damage.height,
                    },
                )
                .map_err(|_| Status::ErrIo)?;
                d.pending_reply = Some((c.channel.0, q.generation));
                Ok::<_, Status>(())
            })();
            if let Err(status) = status {
                rt::reply(
                    c.channel,
                    &DisplayCoordinatorPresentResponse { status, fence: 0 },
                );
            }
            rt::close(&m.handles);
            return;
        }
        4 => {
            let status = if let Ok(q) = DisplayCoordinatorTransferRequest::decode(bytes, &hs) {
                if ids.contains(&q.next_client)
                    && d.ownership
                        .begin(c.channel.0, q.generation, q.next_client, rt::now_us())
                        .is_ok()
                {
                    Status::Ok
                } else {
                    Status::ErrAccessDenied
                }
            } else {
                Status::ErrInvalidArgs
            };
            rt::reply(
                c.channel,
                &DisplayCoordinatorTransferResponse {
                    status,
                    generation: d.ownership.generation,
                },
            );
        }
        5 => {
            let mut buffer = 0;
            let mut status = Status::ErrAccessDenied;
            if d.ownership.owner == c.channel.0 {
                if let Some(h) = &mut d.hardware {
                    let snapshot = if let Some(b) =
                        d.scanout.buffers.iter().find(|b| b.id == d.scanout.current)
                    {
                        let mut out =
                            rt::Mapping::new((h.surface.stride * h.surface.height) as u64);
                        if let Ok(ref mut m) = out {
                            for (dst, src) in m
                                .bytes_mut()
                                .chunks_exact_mut(4)
                                .zip(b.mapping.bytes().chunks_exact(4))
                            {
                                dst.copy_from_slice(
                                    &b.surface
                                        .format
                                        .convert(src.try_into().unwrap(), h.surface.format),
                                );
                            }
                        }
                        out
                    } else {
                        h.snapshot()
                    };
                    if let Ok(map) = snapshot {
                        buffer = Memory::duplicate(map.handle, 1 | 2 | 16 | 32).unwrap_or(0);
                        status = if buffer != 0 {
                            Status::Ok
                        } else {
                            Status::ErrIo
                        };
                    }
                }
            }
            rt::reply(
                c.channel,
                &DisplayCoordinatorSnapshotResponse {
                    status,
                    frame: (buffer != 0).then_some(DisplayFrame {
                        buffer: HandleRef { raw: buffer },
                        surface,
                        generation: d.ownership.generation,
                    }),
                },
            );
        }
        6 => {
            let mut status = Status::ErrAccessDenied;
            if let Ok(q) = DisplayCoordinatorRollbackRequest::decode(bytes, &hs) {
                if d.ownership.generation == q.generation
                    && d.ownership
                        .pending
                        .is_some_and(|p| p.previous == c.channel.0)
                {
                    if d.ownership.rollback().is_ok() {
                        status = Status::Ok;
                    }
                }
            }
            rt::reply(
                c.channel,
                &DisplayCoordinatorRollbackResponse {
                    status,
                    generation: d.ownership.generation,
                },
            );
        }
        _ => {}
    }
    rt::close(&m.handles);
}
