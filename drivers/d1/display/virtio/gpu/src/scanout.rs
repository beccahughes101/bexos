//! Asynchronous 2D resource import and retirement. No driver staging copy is
//! performed: TRANSFER_TO_HOST_2D reads the retained client DMA backing directly.
use crate::{
    hardware::{put32, put64},
    scanout_state::{FORMATS, Imported, MAX_BUFFERS, MAX_BYTES, Retirement},
    state::Display,
};
use bexos_graphics_runtime as rt;
use bexos_userspace::{Channel, Memory};
use graphics_fidl::*;

#[derive(Clone, Copy)]
enum Operation {
    Import { attached: bool },
    Release,
}
pub struct Pending {
    owner: u64,
    resource: u32,
    operation: Operation,
}
fn respond(p: Pending, status: Status) {
    if p.owner == 0 {
        return;
    }
    match p.operation {
        Operation::Import { .. } => rt::reply(
            Channel(p.owner),
            &DisplayCoordinatorImportScanoutResponse {
                status,
                resource: if status == Status::Ok { p.resource } else { 0 },
            },
        ),
        Operation::Release => rt::reply(
            Channel(p.owner),
            &DisplayCoordinatorReleaseScanoutResponse { status },
        ),
    }
}
fn submit(d: &mut Display, opcode: u32, payload: &[u8]) -> Result<(), Status> {
    d.hardware
        .as_mut()
        .filter(|h| h.healthy)
        .ok_or(Status::ErrIo)?
        .submit_request(opcode, payload, 0x1100)
        .map(|_| ())
        .map_err(|_| Status::ErrIo)
}
fn destroy(d: &mut Display, id: u32) {
    if let Some(index) = d.scanout.buffers.iter().position(|b| b.id == id) {
        let b = d.scanout.buffers.remove(index);
        // The device has acknowledged RESOURCE_UNREF, or was never given this backing.
        let _ = Memory::unmap_dma(b.token);
        drop(b);
    }
}
pub fn handle(d: &mut Display, owner: u64, ordinal: u64, bytes: &[u8], handles: &[u64]) -> bool {
    if !(16..=19).contains(&ordinal) {
        return false;
    }
    let refs = rt::refs(handles);
    if ordinal == 16 {
        let valid = handles.is_empty()
            && DisplayCoordinatorGetScanoutCapabilitiesRequest::decode(bytes, &[]).is_ok();
        let status = if !valid {
            Status::ErrInvalidArgs
        } else if d.hardware.as_ref().is_some_and(|h| h.healthy) {
            Status::Ok
        } else {
            Status::ErrIo
        };
        rt::reply(
            Channel(owner),
            &DisplayCoordinatorGetScanoutCapabilitiesResponse {
                status,
                formats: if status == Status::Ok { FORMATS } else { 0 },
                max_buffers: MAX_BUFFERS as u32,
            },
        );
        return true;
    }
    let mut resource = 0;
    let result = (|| {
        if d.pending_gpu.is_some() || d.pending_reply.is_some() || d.pending_scanout.is_some() {
            return Err(Status::ErrBusy);
        }
        match ordinal {
            17 => {
                let q = DisplayCoordinatorImportScanoutRequest::decode(bytes, &refs)
                    .map_err(|_| Status::ErrInvalidArgs)?;
                if handles.len() != 1 {
                    return Err(Status::ErrInvalidArgs);
                }
                d.ownership
                    .authorize(owner, q.generation)
                    .map_err(|_| Status::ErrAccessDenied)?;
                let display = d
                    .hardware
                    .as_ref()
                    .filter(|h| h.healthy)
                    .ok_or(Status::ErrIo)?
                    .surface;
                let surface = rt::surface(q.surface).map_err(|_| Status::ErrInvalidArgs)?;
                let len = crate::scanout_state::import_size(surface, display)
                    .ok_or(Status::ErrInvalidArgs)?;
                if d.scanout.buffers.len() == MAX_BUFFERS || d.scanout.bytes() + len > MAX_BYTES {
                    return Err(Status::ErrNoMemory);
                }
                resource = d
                    .gpu
                    .registry
                    .reserve_resource_id()
                    .map_err(|_| Status::ErrNoMemory)?;
                let handle = Memory::duplicate(q.buffer.raw, 1 | 2 | 16 | 32)
                    .map_err(|_| Status::ErrAccessDenied)?;
                let (address, token) = match Memory::map_dma(d.domain, handle, 0, len, 2) {
                    Ok(v) => v,
                    Err(_) => {
                        let _ = Memory::close(handle);
                        return Err(Status::ErrInvalidArgs);
                    }
                };
                let mapping = match rt::Mapping::map(handle, len, 2) {
                    Ok(v) => v,
                    Err(_) => {
                        let _ = Memory::unmap_dma(token);
                        return Err(Status::ErrNoMemory);
                    }
                };
                d.scanout.buffers.push(Imported {
                    id: resource,
                    owner,
                    surface,
                    mapping,
                    address,
                    token,
                    ready: false,
                    retiring: true,
                });
                let mut payload = [0; 16];
                put32(&mut payload, 0, resource);
                // VirtIO formats B8G8R8X8_UNORM=2, R8G8B8X8_UNORM=134.
                put32(
                    &mut payload,
                    4,
                    if surface.format.is_bgra() { 2 } else { 134 },
                );
                put32(&mut payload, 8, surface.width);
                put32(&mut payload, 12, surface.height);
                if let Err(e) = submit(d, 0x101, &payload) {
                    destroy(d, resource);
                    return Err(e);
                }
                d.pending_scanout = Some(Pending {
                    owner,
                    resource,
                    operation: Operation::Import { attached: false },
                });
            }
            18 => {
                let q = DisplayCoordinatorPresentScanoutRequest::decode(bytes, &refs)
                    .map_err(|_| Status::ErrInvalidArgs)?;
                if handles.len() != 1
                    || q.sequence == 0
                    || !Memory::object_info(q.release.raw)
                        .is_ok_and(|(kind, _)| kind == kernel_fidl::ObjectType::Channel)
                {
                    return Err(Status::ErrInvalidArgs);
                }
                d.ownership
                    .authorize(owner, q.generation)
                    .map_err(|_| Status::ErrAccessDenied)?;
                // Splash takeover still requires an exact copy through the legacy path.
                if d.ownership.pending.is_some() || d.scanout.busy(q.resource) {
                    return Err(Status::ErrBusy);
                }
                let b = d
                    .scanout
                    .get(owner, q.resource)
                    .ok_or(Status::ErrAccessDenied)?;
                let damage = bexos_graphics::Damage {
                    x: q.damage.x,
                    y: q.damage.y,
                    width: q.damage.width,
                    height: q.damage.height,
                };
                damage
                    .validate(b.surface)
                    .map_err(|_| Status::ErrInvalidArgs)?;
                let release = Memory::duplicate(q.release.raw, 1 | 2 | 4 | 32)
                    .map_err(|_| Status::ErrInvalidArgs)?;
                if d.hardware
                    .as_mut()
                    .ok_or(Status::ErrIo)?
                    .begin_imported(q.resource, b.surface, damage)
                    .is_err()
                {
                    let _ = Memory::close(release);
                    return Err(Status::ErrIo);
                }
                d.scanout.pending = Some(Retirement {
                    resource: q.resource,
                    sequence: q.sequence,
                    channel: release,
                });
                d.pending_reply = Some((owner, q.generation));
            }
            19 => {
                let q = DisplayCoordinatorReleaseScanoutRequest::decode(bytes, &refs)
                    .map_err(|_| Status::ErrInvalidArgs)?;
                if !handles.is_empty() {
                    return Err(Status::ErrInvalidArgs);
                }
                d.ownership
                    .authorize(owner, q.generation)
                    .map_err(|_| Status::ErrAccessDenied)?;
                if d.scanout.busy(q.resource) {
                    return Err(Status::ErrBusy);
                }
                if !d
                    .scanout
                    .buffers
                    .iter()
                    .any(|b| b.id == q.resource && b.owner == owner)
                {
                    return Err(Status::ErrAccessDenied);
                }
                let mut payload = [0; 8];
                put32(&mut payload, 0, q.resource);
                submit(d, 0x102, &payload)?;
                d.pending_scanout = Some(Pending {
                    owner,
                    resource: q.resource,
                    operation: Operation::Release,
                });
            }
            _ => unreachable!(),
        }
        Ok(())
    })();
    if let Err(status) = result {
        if ordinal == 18 {
            // No descriptor was submitted, so the received buffer lease can be
            // retired immediately even when admission or authorization failed.
            if let Ok(q) = DisplayCoordinatorPresentScanoutRequest::decode(bytes, &refs) {
                if handles.len() == 1 && q.sequence != 0 {
                    rt::reply(
                        Channel(q.release.raw),
                        &PresentationRelease {
                            sequence: q.sequence,
                            status,
                        },
                    );
                }
            }
            rt::reply(
                Channel(owner),
                &DisplayCoordinatorPresentScanoutResponse { status, fence: 0 },
            );
        } else {
            respond(
                Pending {
                    owner,
                    resource,
                    operation: if ordinal == 17 {
                        Operation::Import { attached: false }
                    } else {
                        Operation::Release
                    },
                },
                status,
            );
        }
    }
    true
}
pub fn poll(d: &mut Display) {
    let Some(mut p) = d.pending_scanout.take() else {
        return;
    };
    match d
        .hardware
        .as_mut()
        .ok_or(kernel_fidl::Status::ErrInvalidArgs)
        .and_then(|h| h.poll_response())
    {
        Ok(None) => {
            d.pending_scanout = Some(p);
            return;
        }
        Err(_) => {
            if matches!(p.operation, Operation::Import { attached: false })
                && d.hardware.as_ref().is_some_and(|h| h.healthy)
            {
                destroy(d, p.resource);
            } else if let Some(b) = d.scanout.buffers.iter_mut().find(|b| b.id == p.resource) {
                b.retiring = true;
            }
            if p.owner == 0 {
                if let Some(h) = &mut d.hardware {
                    h.healthy = false;
                }
            }
            respond(p, Status::ErrIo);
            return;
        }
        Ok(Some(_)) => {}
    }
    match p.operation {
        Operation::Import { attached: false } => {
            let b = d
                .scanout
                .buffers
                .iter()
                .find(|b| b.id == p.resource)
                .unwrap();
            let mut payload = [0; 24];
            put32(&mut payload, 0, p.resource);
            put32(&mut payload, 4, 1);
            put64(&mut payload, 8, b.address);
            put32(&mut payload, 16, b.surface.stride * b.surface.height);
            p.operation = Operation::Import { attached: true };
            match submit(d, 0x106, &payload) {
                Ok(()) => d.pending_scanout = Some(p),
                Err(status) => {
                    d.scanout
                        .buffers
                        .iter_mut()
                        .find(|b| b.id == p.resource)
                        .unwrap()
                        .retiring = true;
                    respond(p, status);
                }
            }
        }
        Operation::Import { attached: true } => {
            let b = d
                .scanout
                .buffers
                .iter_mut()
                .find(|b| b.id == p.resource)
                .unwrap();
            b.ready = true;
            b.retiring = false;
            respond(p, Status::Ok);
        }
        Operation::Release => {
            destroy(d, p.resource);
            respond(p, Status::Ok);
        }
    }
}
fn retire(p: Retirement, status: Status) {
    rt::reply(
        Channel(p.channel),
        &PresentationRelease {
            sequence: p.sequence,
            status,
        },
    );
    let _ = Memory::close(p.channel);
}
pub fn progress(d: &mut Display, failed: bool) {
    if let Some(h) = &d.hardware {
        if let Some(old) = d.scanout.switched(h.scanout_resource) {
            retire(old, Status::Ok);
        }
    }
    if failed {
        // A failed transfer never acquired scanout ownership. If SET_SCANOUT
        // completed, switched() has already moved its lease into current state.
        if let Some(p) = d.scanout.pending.take() {
            if d.hardware.as_ref().is_some_and(|h| h.healthy) {
                retire(p, Status::ErrIo);
            } else {
                d.scanout.uncertain = Some(p);
            }
        }
    }
}
pub fn cleanup(d: &mut Display, clients: &[u64]) {
    if d.pending_reply.is_some()
        || d.pending_gpu.is_some()
        || d.pending_scanout.is_some()
        || !d.hardware.as_ref().is_some_and(|h| h.healthy)
    {
        return;
    }
    let Some(id) = d
        .scanout
        .buffers
        .iter()
        .find(|b| !d.scanout.busy(b.id) && (b.retiring || !clients.contains(&b.owner)))
        .map(|b| b.id)
    else {
        return;
    };
    let mut payload = [0; 8];
    put32(&mut payload, 0, id);
    if submit(d, 0x102, &payload).is_ok() {
        d.pending_scanout = Some(Pending {
            owner: 0,
            resource: id,
            operation: Operation::Release,
        });
    }
}
