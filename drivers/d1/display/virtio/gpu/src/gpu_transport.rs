//! Bounded asynchronous Venus transport on granted endpoints.
use crate::{
    gpu_state::SharedMemory,
    hardware::{put32, put64},
    state::Display,
};
use bexos_graphics_runtime as rt;
use bexos_userspace::{Channel, Memory};
use bexos_virtio_gpu_protocol::transport;
use bexos_virtio_hal::buffer::DmaBuffer;
use graphics_fidl::*;

#[derive(Clone, Copy)]
enum Operation {
    CreateContext,
    DestroyContext,
    Submit,
    CreateMemory {
        id: u32,
        duplicate: u64,
        attached: bool,
    },
    Release {
        id: u32,
        detached: bool,
    },
    CreateHost {
        id: u32,
        stage: u8,
    },
    ReleaseHost {
        id: u32,
        stage: u8,
    },
}
pub struct Pending {
    owner: u64,
    reply: u64,
    context: u32,
    operation: Operation,
}
fn error(error: kernel_fidl::Status) -> Status {
    match error {
        kernel_fidl::Status::ErrNoMemory | kernel_fidl::Status::ErrResourceExhausted => {
            Status::ErrNoMemory
        }
        kernel_fidl::Status::ErrTimedOut => Status::ErrTimedOut,
        _ => Status::ErrIo,
    }
}
fn admission(error: bexos_virtio_gpu_protocol::Error) -> Status {
    match error {
        bexos_virtio_gpu_protocol::Error::Capacity => Status::ErrNoMemory,
        _ => Status::ErrInvalidArgs,
    }
}
fn respond(p: Pending, status: Status, fence: u64) {
    let channel = Channel(p.reply);
    if p.reply == 0 {
        if let Operation::CreateMemory { duplicate, .. } = p.operation {
            let _ = Memory::close(duplicate);
        }
        return;
    }
    match p.operation {
        Operation::CreateContext => rt::reply(
            channel,
            &DisplayCoordinatorCreateGpuContextResponse {
                status,
                context: if status == Status::Ok { p.context } else { 0 },
            },
        ),
        Operation::DestroyContext => rt::reply(
            channel,
            &DisplayCoordinatorDestroyGpuContextResponse { status },
        ),
        Operation::Submit => rt::reply(
            channel,
            &DisplayCoordinatorSubmitGpuCommandResponse { status, fence },
        ),
        Operation::CreateMemory { id, duplicate, .. } => {
            if status != Status::Ok && duplicate != 0 {
                let _ = Memory::close(duplicate);
            }
            rt::reply(
                channel,
                &DisplayCoordinatorCreateGpuSharedMemoryResponse {
                    status,
                    resource: if status == Status::Ok { id } else { 0 },
                    memory: if status == Status::Ok {
                        Some(GpuSharedMemory {
                            buffer: HandleRef { raw: duplicate },
                            host_visible: false,
                        })
                    } else {
                        None
                    },
                },
            );
        }
        Operation::Release { .. } => rt::reply(
            channel,
            &DisplayCoordinatorReleaseGpuResourceResponse { status },
        ),
        Operation::ReleaseHost { .. } => rt::reply(
            channel,
            &DisplayCoordinatorReleaseGpuResourceResponse { status },
        ),
        Operation::CreateHost { .. } => rt::reply(
            channel,
            &DisplayCoordinatorCreateGpuSharedMemoryResponse {
                status,
                resource: 0,
                memory: None,
            },
        ),
    }
}
fn submit(
    d: &mut Display,
    context: u32,
    command: u32,
    payload: &[u8],
    ring: Option<u8>,
) -> Result<(), Status> {
    d.hardware
        .as_mut()
        .filter(|h| h.healthy)
        .ok_or(Status::ErrIo)?
        .submit_context_request(command, payload, 0x1100, context, ring)
        .map(|_| ())
        .map_err(error)
}
pub fn handle(d: &mut Display, owner: u64, ordinal: u64, bytes: &[u8], handles: &[u64]) -> bool {
    if !(10..=15).contains(&ordinal) {
        return false;
    }
    let mut pending = Pending {
        owner,
        reply: owner,
        context: 0,
        operation: match ordinal {
            10 => Operation::CreateContext,
            11 => Operation::DestroyContext,
            12 => Operation::Submit,
            13 | 15 => Operation::CreateMemory {
                id: 0,
                duplicate: 0,
                attached: false,
            },
            _ => Operation::Release {
                id: 0,
                detached: false,
            },
        },
    };
    let result = (|| {
        if !handles.is_empty() {
            return Err(Status::ErrInvalidArgs);
        }
        if d.pending_gpu.is_some() || d.pending_reply.is_some() || d.pending_scanout.is_some() {
            return Err(Status::ErrBusy);
        }
        if !d
            .hardware
            .as_ref()
            .is_some_and(|h| h.healthy && transport::supported(&h.capabilities))
        {
            return Err(Status::ErrIo);
        }
        match ordinal {
            10 => {
                DisplayCoordinatorCreateGpuContextRequest::decode(bytes, &[])
                    .map_err(|_| Status::ErrInvalidArgs)?;
                pending.context = d.gpu.registry.create_context(owner).map_err(admission)?;
                let mut payload = [0; 72];
                put32(&mut payload, 0, 5);
                put32(&mut payload, 4, 4); // Venus capset, not the VirGL default.
                payload[8..13].copy_from_slice(b"bexos");
                submit(d, pending.context, 0x200, &payload, None)?;
            }
            11 => {
                let q = DisplayCoordinatorDestroyGpuContextRequest::decode(bytes, &[])
                    .map_err(|_| Status::ErrInvalidArgs)?;
                authorize(d, owner, q.context)?;
                if d.gpu
                    .registry
                    .resources
                    .iter()
                    .any(|r| r.context == q.context)
                {
                    return Err(Status::ErrBusy);
                }
                pending.context = q.context;
                submit(d, q.context, 0x201, &[], None)?;
            }
            12 => {
                let q = DisplayCoordinatorSubmitGpuCommandRequest::decode(bytes, &[])
                    .map_err(|_| Status::ErrInvalidArgs)?;
                authorize(d, owner, q.context)?;
                if q.ring >= 64 || q.data.len() % 4 != 0 {
                    return Err(Status::ErrInvalidArgs);
                }
                pending.context = q.context;
                let mut payload = [0; 4000];
                put32(&mut payload, 0, q.data.len() as u32);
                payload[8..8 + q.data.len()].copy_from_slice(q.data);
                submit(
                    d,
                    q.context,
                    0x207,
                    &payload[..8 + q.data.len()],
                    Some(q.ring as u8),
                )?;
            }
            13 => {
                let q = DisplayCoordinatorCreateGpuSharedMemoryRequest::decode(bytes, &[])
                    .map_err(|_| Status::ErrInvalidArgs)?;
                authorize(d, owner, q.context)?;
                pending.context = q.context;
                if d.gpu.aperture.size != 0 {
                    let id = crate::host_memory::create(d, owner, q.context, q.size, 0, true)?;
                    pending.operation = Operation::CreateHost { id, stage: 0 };
                    return Ok(());
                }
                let id = d
                    .gpu
                    .registry
                    .create_resource(owner, q.context, q.size)
                    .map_err(admission)?;
                // Reserve identity before allocation; failures consume IDs, never reuse them.
                let allocate = (|| {
                    let buffer =
                        DmaBuffer::new((q.size / 4096) as usize).ok_or(Status::ErrNoMemory)?;
                    let snapshot = buffer.snapshot();
                    let allocation = bexos_virtio_hal::dma_snapshot()
                        .into_iter()
                        .find(|v| v.paddr == snapshot[0])
                        .ok_or(Status::ErrIo)?;
                    let duplicate =
                        Memory::duplicate(allocation.handle, 1 | 2 | 4 | 16 | 32).map_err(error)?;
                    d.gpu.memory.push(SharedMemory {
                        id,
                        buffer: Some(buffer),
                        saved: snapshot,
                        retiring: false,
                    });
                    Ok((snapshot, duplicate))
                })();
                let (snapshot, duplicate) = match allocate {
                    Ok(value) => value,
                    Err(error) => {
                        let _ = d.gpu.registry.remove_resource(owner, q.context, id);
                        return Err(error);
                    }
                };
                pending.operation = Operation::CreateMemory {
                    id,
                    duplicate,
                    attached: false,
                };
                let mut payload = [0; 48];
                put32(&mut payload, 0, id);
                put32(&mut payload, 4, 1); // GUEST memory, with one contiguous DMA entry.
                put32(&mut payload, 8, 1); // MAPPABLE
                put32(&mut payload, 12, 1);
                put64(&mut payload, 24, q.size);
                put64(&mut payload, 32, snapshot[0]);
                put32(&mut payload, 40, q.size as u32);
                submit(d, q.context, 0x10c, &payload, None)?;
            }
            14 => {
                let q = DisplayCoordinatorReleaseGpuResourceRequest::decode(bytes, &[])
                    .map_err(|_| Status::ErrInvalidArgs)?;
                d.gpu
                    .registry
                    .resource(owner, q.context, q.resource)
                    .map_err(|_| Status::ErrAccessDenied)?;
                pending.context = q.context;
                if d.gpu.host.iter().any(|m| m.id == q.resource) {
                    crate::host_memory::release(d, q.context, q.resource)?;
                    pending.operation = Operation::ReleaseHost {
                        id: q.resource,
                        stage: 0,
                    };
                    return Ok(());
                }
                pending.operation = Operation::Release {
                    id: q.resource,
                    detached: false,
                };
                let mut payload = [0; 8];
                put32(&mut payload, 0, q.resource);
                submit(d, q.context, 0x203, &payload, None)?;
            }
            15 => {
                let q = DisplayCoordinatorCreateGpuBlobRequest::decode(bytes, &[])
                    .map_err(|_| Status::ErrInvalidArgs)?;
                authorize(d, owner, q.context)?;
                if q.blob_id == 0 && !q.host_visible {
                    return Err(Status::ErrInvalidArgs);
                }
                pending.context = q.context;
                let id = crate::host_memory::create(
                    d,
                    owner,
                    q.context,
                    q.size,
                    q.blob_id,
                    q.host_visible,
                )?;
                pending.operation = Operation::CreateHost { id, stage: 0 };
            }
            _ => unreachable!(),
        }
        Ok(())
    })();
    match result {
        Ok(()) => d.pending_gpu = Some(pending),
        Err(status) => {
            // Submission errors occur before publishing a descriptor. These
            // allocations were never made visible to the host device.
            match pending.operation {
                Operation::CreateContext if pending.context != 0 => {
                    let _ = d.gpu.registry.remove_context(owner, pending.context);
                }
                Operation::CreateMemory { id, .. } if id != 0 => {
                    let _ = d.gpu.registry.remove_resource(owner, pending.context, id);
                    d.gpu.memory.retain(|m| m.id != id);
                }
                _ => {}
            }
            respond(pending, status, 0);
        }
    }
    true
}
fn authorize(d: &Display, owner: u64, context: u32) -> Result<(), Status> {
    d.gpu
        .registry
        .context(owner, context)
        .map_err(|_| Status::ErrAccessDenied)
}
pub fn poll(d: &mut Display) {
    let Some(mut pending) = d.pending_gpu.take() else {
        return;
    };
    let result = d
        .hardware
        .as_mut()
        .ok_or(kernel_fidl::Status::ErrInvalidArgs)
        .and_then(|h| h.poll_response());
    let length = match result {
        Ok(None) => {
            d.pending_gpu = Some(pending);
            return;
        }
        Err(e) => {
            let healthy = d.hardware.as_ref().is_some_and(|h| h.healthy);
            match pending.operation {
                Operation::CreateContext if healthy => {
                    let _ = d
                        .gpu
                        .registry
                        .remove_context(pending.owner, pending.context);
                }
                Operation::CreateMemory {
                    id,
                    attached: false,
                    ..
                } if healthy => {
                    let _ = d
                        .gpu
                        .registry
                        .remove_resource(pending.owner, pending.context, id);
                    d.gpu.memory.retain(|m| m.id != id);
                }
                Operation::CreateMemory { id, .. } => {
                    if let Some(m) = d.gpu.memory.iter_mut().find(|m| m.id == id) {
                        m.retiring = true;
                    }
                }
                Operation::CreateHost { id, stage } => {
                    crate::host_memory::failed_create(d, pending.owner, pending.context, id, stage)
                }
                _ => {}
            }
            // Failed orphan cleanup must not spin or unpin uncertain DMA.
            if pending.reply == 0 {
                if let Some(h) = &mut d.hardware {
                    h.healthy = false;
                }
            }
            respond(pending, error(e), 0);
            return;
        }
        Ok(Some(length)) => length,
    };
    let fence = d.hardware.as_ref().unwrap().fence;
    if let Operation::CreateHost { id, mut stage } = pending.operation {
        match crate::host_memory::created(d, pending.owner, pending.context, id, &mut stage, length)
        {
            Ok(Some(duplicate)) => {
                rt::reply(
                    Channel(pending.reply),
                    &DisplayCoordinatorCreateGpuSharedMemoryResponse {
                        status: Status::Ok,
                        resource: id,
                        memory: (duplicate != 0).then_some(GpuSharedMemory {
                            buffer: HandleRef { raw: duplicate },
                            host_visible: true,
                        }),
                    },
                );
            }
            Ok(None) => {
                pending.operation = Operation::CreateHost { id, stage };
                d.pending_gpu = Some(pending);
            }
            Err(status) => {
                crate::host_memory::retire_create(d, id);
                respond(pending, status, 0);
            }
        }
        return;
    }
    if let Operation::ReleaseHost { id, mut stage } = pending.operation {
        match crate::host_memory::released(d, pending.owner, pending.context, id, &mut stage) {
            Ok(true) => respond(pending, Status::Ok, fence),
            Ok(false) => {
                pending.operation = Operation::ReleaseHost { id, stage };
                d.pending_gpu = Some(pending);
            }
            Err(status) => respond(pending, status, 0),
        }
        return;
    }
    let next = match pending.operation {
        Operation::CreateMemory {
            id,
            duplicate,
            attached: false,
        } => {
            pending.operation = Operation::CreateMemory {
                id,
                duplicate,
                attached: true,
            };
            Some((0x202, id, pending.context))
        }
        Operation::Release {
            id,
            detached: false,
        } => {
            pending.operation = Operation::Release { id, detached: true };
            Some((0x102, id, 0))
        }
        Operation::Release { id, detached: true } => {
            let _ = d
                .gpu
                .registry
                .remove_resource(pending.owner, pending.context, id);
            d.gpu.memory.retain(|m| m.id != id); // Unpin only after device unref completion.
            None
        }
        Operation::DestroyContext => {
            let _ = d
                .gpu
                .registry
                .remove_context(pending.owner, pending.context);
            None
        }
        _ => None,
    };
    if let Some((command, id, context)) = next {
        let mut payload = [0; 8];
        put32(&mut payload, 0, id);
        match submit(d, context, command, &payload, None) {
            Ok(()) => d.pending_gpu = Some(pending),
            Err(status) => respond(pending, status, 0),
        }
    } else {
        respond(pending, Status::Ok, fence);
    }
}

/// Release one orphan's next resource, then its context. This also runs after
/// transplant; disconnected endpoints never regain authority through ID reuse.
pub fn cleanup(d: &mut Display, clients: &[u64]) {
    if d.pending_scanout.is_some()
        || d.pending_gpu.is_some()
        || d.pending_reply.is_some()
        || !d.hardware.as_ref().is_some_and(|h| h.healthy)
    {
        return;
    }
    let retiring = d
        .gpu
        .memory
        .iter()
        .find(|m| m.retiring)
        .map(|m| m.id)
        .or_else(|| d.gpu.host.iter().find(|m| m.retiring).map(|m| m.id));
    let retired = retiring
        .and_then(|id| d.gpu.registry.resources.iter().find(|r| r.id == id))
        .copied();
    let Some(context) = d
        .gpu
        .registry
        .contexts
        .iter()
        .find(|c| retired.is_some_and(|r| r.context == c.id) || !clients.contains(&c.owner))
        .copied()
    else {
        return;
    };
    let mut pending = Pending {
        owner: context.owner,
        reply: 0,
        context: context.id,
        operation: Operation::DestroyContext,
    };
    let result = if let Some(resource) = retired.or_else(|| {
        d.gpu
            .registry
            .resources
            .iter()
            .find(|r| r.context == context.id)
            .copied()
    }) {
        if d.gpu.host.iter().any(|m| m.id == resource.id) {
            pending.operation = Operation::ReleaseHost {
                id: resource.id,
                stage: 0,
            };
            crate::host_memory::release(d, context.id, resource.id)
        } else {
            pending.operation = Operation::Release {
                id: resource.id,
                detached: false,
            };
            let mut payload = [0; 8];
            put32(&mut payload, 0, resource.id);
            submit(d, context.id, 0x203, &payload, None)
        }
    } else {
        submit(d, context.id, 0x201, &[], None)
    };
    if result.is_ok() {
        d.pending_gpu = Some(pending);
    }
}
