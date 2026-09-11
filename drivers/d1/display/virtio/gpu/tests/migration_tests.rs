use bexos_graphics_runtime::migration::Runtime;
use bexos_userspace::live_migration::State;
use bexos_virtio_gpu::state::Display;
#[test]
fn outstanding_device_reply_prevents_cutover_until_drained() {
    let mut runtime = Runtime::<Display>::empty();
    assert!(runtime.quiescence_ready());
    runtime.component.pending_reply = Some((8, 3));
    assert!(!runtime.quiescence_ready());
    runtime.component.pending_reply = None;
    assert!(runtime.quiescence_ready());
}
#[test]
fn rejects_truncated_or_wrong_version_gpu_state() {
    let mut receiver = Runtime::<Display>::empty();
    for bytes in [&[][..], &[0; 8][..], &[1, 0, 0, 0, 0, 0, 0, 0][..]] {
        assert!(receiver.adopt_record(1, Some(bytes)).is_err());
    }
    assert!(receiver.validate().is_err());
}

#[test]
fn retained_scanout_and_pending_transfer_roundtrip_without_device_reset() {
    use bexos_migration::codec::Encoder;
    use bexos_userspace::{Channel, live_migration::Resource};
    let mut w = Encoder::new();
    w.word(1);
    // Ownership, pending transfer, IOMMU domain and lifecycle endpoint.
    for v in [
        11, 5, 1, 10, 11, 2_000_000, 20, 21, 1, 22, 0x4000000, 0x1000000, 6, 256,
    ] {
        w.word(v);
    }
    let buffers = [
        [0x100000, 0x8000000, 3],
        [0x200000, 0x9000000, 2],
        [0x300000, 0xa000000, 469],
        [0x600000, 0xb000000, 469],
    ];
    for b in buffers {
        for v in b {
            w.word(v);
        }
    }
    for v in [31, 1, 42, 4] {
        w.word(v);
    }
    for (i, b) in buffers.iter().enumerate() {
        for v in [b[0], b[1], b[2] * 4096, 30 + i as u64, 40 + i as u64, 1, 1] {
            w.word(v);
        }
    }
    w.word(0);
    w.word(1);
    let bytes = w.finish();
    let mut candidate = Runtime::<Display>::new(Channel(1), Some(Channel(2)), Display::default());
    candidate.adopt_record(1, Some(&bytes)).unwrap();
    candidate.validate().unwrap();
    assert_eq!(candidate.component.ownership.owner, 11);
    assert_eq!(candidate.component.ownership.pending.unwrap().previous, 10);
    assert_eq!(candidate.encode_record(1).unwrap().unwrap(), bytes);
    assert!(
        candidate
            .resources()
            .iter()
            .any(|r| matches!(r, Resource::Pin(43)))
    );
    let before = candidate.encode_record(1).unwrap();
    let mut rejected = Runtime::<Display>::empty();
    assert!(
        rejected
            .adopt_record(1, Some(&bytes[..bytes.len() - 1]))
            .is_err()
    );
    assert_eq!(candidate.encode_record(1).unwrap(), before);
}

#[test]
fn disabled_display_remains_transplantable() {
    use bexos_userspace::Channel;
    let source = Runtime::new(Channel(1), Some(Channel(2)), Display::default());
    let mut target = Runtime::<Display>::empty();
    for k in source.keys() {
        target
            .adopt_record(k, source.encode_record(k).unwrap().as_deref())
            .unwrap();
    }
    target.validate().unwrap();
    assert!(target.component.hardware.is_none());
}

#[test]
fn device_only_blobs_migrate_without_aperture_spans_or_vmo_handles() {
    use bexos_migration::codec::{Decoder, Encoder};
    use bexos_virtio_gpu::gpu_state::{GpuState, HostMemory};
    let mut state = GpuState::default();
    let context = state.registry.create_context(7).unwrap();
    for _ in 0..2 {
        let id = state.registry.create_resource(7, context, 4096).unwrap();
        state.host.push(HostMemory {
            id,
            visible: false,
            offset: 0,
            handle: 0,
            mapped: false,
            retiring: false,
            owned: false,
        });
    }
    let encode = |s: &GpuState| {
        let mut w = Encoder::new();
        s.encode(&mut w);
        w.finish()
    };
    let bytes = encode(&state);
    let mut r = Decoder::new(&bytes);
    let restored = GpuState::decode(&mut r, &[]).unwrap();
    r.finish().unwrap();
    assert_eq!(encode(&restored), bytes);
    state.host[0].handle = 3;
    assert!(GpuState::decode(&mut Decoder::new(&encode(&state)), &[]).is_err());
    state.host[0].handle = 0;
    state.host[0].mapped = true;
    assert!(GpuState::decode(&mut Decoder::new(&encode(&state)), &[]).is_err());
}

#[test]
fn version_four_host_mappings_import_and_reencode_before_activation() {
    use bexos_migration::codec::{Decoder, Encoder};
    use bexos_virtio_gpu::gpu_state::{GpuState, HostMemory};
    let mut state = GpuState::default();
    state.aperture.base = 0x2000_0000;
    state.aperture.size = 0x10000;
    let context = state.registry.create_context(7).unwrap();
    let id = state.registry.create_resource(7, context, 4096).unwrap();
    state.host.push(HostMemory {
        id,
        visible: true,
        offset: 0,
        handle: 77,
        mapped: true,
        retiring: false,
        owned: false,
    });
    let mut w = Encoder::new();
    state.encode_version(&mut w, true);
    let bytes = w.finish();
    let mut r = Decoder::new(&bytes);
    let restored = GpuState::decode_version(&mut r, &[], 4).unwrap();
    r.finish().unwrap();
    assert!(restored.host[0].visible);
    let mut w = Encoder::new();
    restored.encode_version(&mut w, true);
    assert_eq!(w.finish(), bytes);
}

#[test]
fn venus_shared_memory_migration_requires_matching_dma_and_owned_context() {
    use bexos_migration::codec::{Decoder, Encoder};
    use bexos_virtio_gpu::gpu_state::{GpuState, SharedMemory};
    use bexos_virtio_hal::DmaAllocation;
    let mut state = GpuState::default();
    let context = state.registry.create_context(7).unwrap();
    let id = state.registry.create_resource(7, context, 4096).unwrap();
    state.memory.push(SharedMemory {
        id,
        buffer: None,
        saved: [0x1000, 0x2000, 1],
        retiring: false,
    });
    let allocation = DmaAllocation {
        paddr: 0x1000,
        vaddr: 0x2000,
        size: 4096,
        handle: 3,
        token: 4,
        active: true,
        owned: false,
    };
    let mut w = Encoder::new();
    state.encode(&mut w);
    let bytes = w.finish();
    let mut r = Decoder::new(&bytes);
    let decoded = GpuState::decode(&mut r, &[allocation]).unwrap();
    r.finish().unwrap();
    assert_eq!(decoded.registry, state.registry);
    assert_eq!(decoded.memory[0].snapshot(), [0x1000, 0x2000, 1]);
    assert!(decoded.registry.context(8, context).is_err());
    assert!(GpuState::decode(&mut Decoder::new(&bytes), &[]).is_err());
    assert!(
        GpuState::decode(
            &mut Decoder::new(&bytes),
            &[DmaAllocation {
                vaddr: 0x3000,
                ..allocation
            }]
        )
        .is_err()
    );
    for len in 0..bytes.len() {
        assert!(GpuState::decode(&mut Decoder::new(&bytes[..len]), &[allocation]).is_err());
    }
}

#[test]
fn venus_host_aperture_migration_rejects_live_aliases_and_invalid_backings() {
    use bexos_migration::codec::{Decoder, Encoder};
    use bexos_virtio_gpu::gpu_state::{GpuState, HostMemory};
    let mut state = GpuState::default();
    state.aperture.base = 0x2000_0000;
    state.aperture.size = 0x10000;
    let context = state.registry.create_context(7).unwrap();
    let a = state.registry.create_resource(7, context, 4096).unwrap();
    let b = state.registry.create_resource(7, context, 4096).unwrap();
    state.host.push(HostMemory {
        visible: true,
        id: a,
        offset: 0,
        handle: 70,
        mapped: true,
        retiring: false,
        owned: false,
    });
    state.host.push(HostMemory {
        visible: true,
        id: b,
        offset: 4096,
        handle: 71,
        mapped: true,
        retiring: true,
        owned: false,
    });
    fn encoded(state: &GpuState) -> Vec<u8> {
        let mut w = Encoder::new();
        state.encode(&mut w);
        w.finish()
    }
    let bytes = encoded(&state);
    let mut decoder = Decoder::new(&bytes);
    let restored = GpuState::decode(&mut decoder, &[]).unwrap();
    decoder.finish().unwrap();
    assert_eq!(encoded(&restored), bytes);
    assert!(!restored.host[0].owned);
    state.host[1].offset = 0;
    assert!(GpuState::decode(&mut Decoder::new(&encoded(&state)), &[]).is_err());
    state.host[1].offset = 4096;
    state.host[1].handle = 70;
    assert!(GpuState::decode(&mut Decoder::new(&encoded(&state)), &[]).is_err());
    state.host[1].handle = 71;
    state.host[1].mapped = false;
    assert!(GpuState::decode(&mut Decoder::new(&encoded(&state)), &[]).is_err());
    for len in 0..bytes.len() {
        assert!(GpuState::decode(&mut Decoder::new(&bytes[..len]), &[]).is_err());
    }
}

#[test]
fn scanout_retirement_waits_for_replacement_and_survives_uncertain_device_state() {
    use bexos_virtio_gpu::scanout_state::{Retirement, Scanout};
    let mut s = Scanout::default();
    let a = Retirement {
        resource: 3,
        sequence: 7,
        channel: 70,
    };
    let b = Retirement {
        resource: 4,
        sequence: 8,
        channel: 80,
    };
    s.pending = Some(a);
    assert!(s.busy(3));
    // Transfer completion does not retire anything. SET_SCANOUT does.
    assert_eq!(s.release, None);
    assert_eq!(s.switched(3), None);
    assert_eq!(s.release, Some(a));
    assert_eq!(s.switched(3), None);
    s.pending = Some(b);
    assert_eq!(s.switched(4), Some(a));
    assert!(!s.busy(3));
    assert!(s.busy(4));
    assert_eq!(s.switched(2), Some(b));
    assert_eq!(s.release, None);
    s.uncertain = Some(a);
    assert!(s.busy(3));
}

#[test]
fn scanout_import_records_preserve_leases_pins_and_reject_aliases() {
    use bexos_graphics::{Format, Surface};
    use bexos_graphics_runtime::Mapping;
    use bexos_migration::codec::{Decoder, Encoder};
    use bexos_userspace::live_migration::Resource;
    use bexos_virtio_gpu::{
        gpu_state::GpuState,
        scanout_state::{Imported, Retirement, Scanout},
    };
    let mut gpu = GpuState::default();
    let display = Surface {
        width: 800,
        height: 600,
        stride: 3200,
        format: Format::Bgra,
    };
    let mut state = Scanout::default();
    for i in 0..2 {
        let id = gpu.registry.reserve_resource_id().unwrap();
        state.buffers.push(Imported {
            id,
            owner: 11,
            surface: Surface {
                format: Format::Bgrx,
                ..display
            },
            mapping: Mapping {
                handle: 70 + i,
                address: 0x1000000 + i * 0x200000,
                size: 469 * 4096,
                rights: 2,
                owned: false,
            },
            address: 0x8000000 + i * 0x200000,
            token: 90 + i,
            ready: true,
            retiring: false,
        });
    }
    state.pending = Some(Retirement {
        resource: 3,
        sequence: 15,
        channel: 100,
    });
    assert_eq!(state.switched(3), None);
    state.uncertain = Some(Retirement {
        resource: 4,
        sequence: 16,
        channel: 101,
    });
    let encode = |s: &Scanout| {
        let mut w = Encoder::new();
        s.encode(&mut w).unwrap();
        w.finish()
    };
    let bytes = encode(&state);
    let mut r = Decoder::new(&bytes);
    let restored = Scanout::decode(&mut r, &gpu.registry, display).unwrap();
    r.finish().unwrap();
    assert_eq!(encode(&restored), bytes);
    assert!(!restored.buffers[0].mapping.owned);
    assert!(
        restored
            .resources()
            .iter()
            .any(|v| matches!(v, Resource::Pin(90)))
    );
    assert!(
        restored
            .resources()
            .iter()
            .any(|v| matches!(v, Resource::Handle(101)))
    );
    for len in 0..bytes.len() {
        assert!(Scanout::decode(&mut Decoder::new(&bytes[..len]), &gpu.registry, display).is_err());
    }
    state.buffers[1].token = 90;
    assert!(Scanout::decode(&mut Decoder::new(&encode(&state)), &gpu.registry, display).is_err());
    state.buffers[1].token = 91;
    state.buffers[1].address = state.buffers[0].address + 4096;
    assert!(Scanout::decode(&mut Decoder::new(&encode(&state)), &gpu.registry, display).is_err());
}
