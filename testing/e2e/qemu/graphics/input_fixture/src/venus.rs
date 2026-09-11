//! Exercise actual device contexts and retained shared memory on Venus hosts.
use bexos_graphics_runtime as rt;
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
use bexos_userspace::Channel;
use graphics_fidl::*;

pub struct Probe {
    context: u32,
    resource: u32,
    pub mapping: rt::Mapping,
    fence: u64,
}

impl Probe {
    pub fn encode(&self, w: &mut Encoder) {
        w.word(self.context as u64);
        w.word(self.resource as u64);
        w.word(self.fence);
        self.mapping.encode(w);
    }
    pub fn decode(r: &mut Decoder<'_>) -> Result<Self, Error> {
        let context = r.word()?.try_into().map_err(|_| Error::InvalidData)?;
        let resource = r.word()?.try_into().map_err(|_| Error::InvalidData)?;
        let fence = r.word()?;
        let mapping = rt::Mapping::decode(r)?;
        if context == 0 || resource < 3 || fence == 0 || mapping.size != 4096 || mapping.rights != 6
        {
            return Err(Error::InvalidData);
        }
        Ok(Self {
            context,
            resource,
            mapping,
            fence,
        })
    }
    pub fn verify(&mut self, channel: &mut Channel) {
        assert_eq!(
            &self.mapping.bytes()[..8],
            &0x1234_5678_9abc_def0u64.to_le_bytes()
        );
        let offset = crate::venus_query::REPLY_OFFSET;
        self.mapping.bytes_mut()[offset..offset + 20].fill(0xff);
        let command = crate::venus_query::command(self.resource);
        let fence: DisplayCoordinatorSubmitGpuCommandResponse = rt::call(
            channel,
            12,
            &DisplayCoordinatorSubmitGpuCommandRequest {
                context: self.context,
                ring: 0,
                data: &command,
            },
        )
        .unwrap();
        assert_eq!(fence.status, Status::Ok);
        assert!(fence.fence > self.fence);
        self.fence = fence.fence;
        // Host writes this mapping. Read only after its CPU timeline fence;
        // volatile loads also prevent reusing the previous probe's reply.
        core::sync::atomic::fence(core::sync::atomic::Ordering::Acquire);
        let mut reply = [0; 20];
        for (i, byte) in reply.iter_mut().enumerate() {
            *byte = unsafe {
                core::ptr::read_volatile((self.mapping.address as *const u8).add(offset + i))
            };
        }
        let version = crate::venus_query::decode(&reply).expect("host Vulkan version reply");
        bexos_userspace::log(&format!(
            "input-fixture: guest Venus Vulkan version={}.{}.{} verified\n",
            version >> 22,
            (version >> 12) & 1023,
            version & 4095
        ));
        let busy: DisplayCoordinatorReleaseGpuResourceResponse = rt::call(
            channel,
            14,
            &DisplayCoordinatorReleaseGpuResourceRequest {
                context: self.context,
                resource: self.resource,
            },
        )
        .unwrap();
        assert_eq!(busy.status, Status::ErrBusy);
    }
    pub fn release(self, channel: &mut Channel) {
        let Self {
            context,
            resource,
            mapping,
            ..
        } = self;
        drop(mapping);
        let released: DisplayCoordinatorReleaseGpuResourceResponse = rt::call(
            channel,
            14,
            &DisplayCoordinatorReleaseGpuResourceRequest { context, resource },
        )
        .unwrap();
        assert_eq!(released.status, Status::Ok);
        let destroyed: DisplayCoordinatorDestroyGpuContextResponse = rt::call(
            channel,
            11,
            &DisplayCoordinatorDestroyGpuContextRequest { context },
        )
        .unwrap();
        assert_eq!(destroyed.status, Status::Ok);
    }
}

pub fn create(channel: &mut Channel) -> Probe {
    bexos_userspace::log("input-fixture: creating Venus context\n");
    let created: DisplayCoordinatorCreateGpuContextResponse =
        rt::call(channel, 10, &DisplayCoordinatorCreateGpuContextRequest {}).unwrap();
    assert_eq!(created.status, Status::Ok);
    assert_ne!(created.context, 0);
    let context = created.context;
    bexos_userspace::log("input-fixture: Venus context created\n");
    let invalid: DisplayCoordinatorSubmitGpuCommandResponse = rt::call(
        channel,
        12,
        &DisplayCoordinatorSubmitGpuCommandRequest {
            context: u32::MAX,
            ring: 0,
            data: &[],
        },
    )
    .unwrap();
    assert_eq!(invalid.status, Status::ErrAccessDenied);
    let shared: DisplayCoordinatorCreateGpuSharedMemoryResponse = rt::call(
        channel,
        13,
        &DisplayCoordinatorCreateGpuSharedMemoryRequest {
            context,
            size: 4096,
        },
    )
    .unwrap();
    assert_eq!(shared.status, Status::Ok);
    bexos_userspace::log("input-fixture: Venus shared memory created\n");
    let memory = shared.memory.unwrap();
    assert!(
        memory.host_visible,
        "Venus fixture requires a host-visible aperture"
    );
    bexos_userspace::log("input-fixture: Venus host-visible mapping verified\n");
    let mut mapping = rt::Mapping::map(memory.buffer.raw, 4096, 6).unwrap();
    assert!(mapping.bytes().iter().all(|v| *v == 0));
    mapping.bytes_mut()[..8].copy_from_slice(&0x1234_5678_9abc_def0u64.to_le_bytes());
    let busy: DisplayCoordinatorDestroyGpuContextResponse = rt::call(
        channel,
        11,
        &DisplayCoordinatorDestroyGpuContextRequest { context },
    )
    .unwrap();
    assert_eq!(busy.status, Status::ErrBusy);
    let fence: DisplayCoordinatorSubmitGpuCommandResponse = rt::call(
        channel,
        12,
        &DisplayCoordinatorSubmitGpuCommandRequest {
            context,
            ring: 0,
            data: &[],
        },
    )
    .unwrap();
    assert_eq!(fence.status, Status::Ok);
    assert_ne!(fence.fence, 0);
    if memory.host_visible {
        let busy: DisplayCoordinatorReleaseGpuResourceResponse = rt::call(
            channel,
            14,
            &DisplayCoordinatorReleaseGpuResourceRequest {
                context,
                resource: shared.resource,
            },
        )
        .unwrap();
        assert_eq!(busy.status, Status::ErrBusy);
    }
    Probe {
        context,
        resource: shared.resource,
        mapping,
        fence: fence.fence,
    }
}

pub fn verify(channel: &mut Channel) -> Probe {
    let mut disposable = create(channel);
    disposable.verify(channel);
    disposable.release(channel);
    bexos_userspace::log("input-fixture: Venus context/shared-memory/fence verified\n");
    let retained = create(channel);
    bexos_userspace::log("input-fixture: Venus resources retained for transplant\n");
    retained
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retained_gpu_probe_rebuilds_logical_state_without_owning_source_mapping() {
        let probe = Probe {
            context: 7,
            resource: 9,
            fence: 13,
            mapping: rt::Mapping {
                handle: 42,
                address: 0x1000,
                size: 4096,
                rights: 6,
                owned: false,
            },
        };
        let mut w = Encoder::new();
        probe.encode(&mut w);
        let bytes = w.finish();
        let restored = Probe::decode(&mut Decoder::new(&bytes)).unwrap();
        assert_eq!(
            (restored.context, restored.resource, restored.fence),
            (7, 9, 13)
        );
        assert_eq!(restored.mapping.handle, 42);
        assert!(!restored.mapping.owned);
        for len in 0..bytes.len() {
            assert!(Probe::decode(&mut Decoder::new(&bytes[..len])).is_err());
        }
        for (offset, value) in [
            (0, 0),
            (0, u64::MAX),
            (8, 2),
            (16, 0),
            (32, 1),
            (40, 8192),
            (48, 2),
        ] {
            let mut invalid = bytes.clone();
            invalid[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
            assert!(Probe::decode(&mut Decoder::new(&invalid)).is_err());
        }
    }
}
