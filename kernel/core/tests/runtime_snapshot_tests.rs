use bexos_kernel_core::{
    cpu_features::AsidSupport,
    runtime::{Backend, InterruptDelivery, Result as RuntimeResult, Runtime},
    sched::{DeadlineProfile, FairProfile, SchedulingProfile},
    transplant::{
        codec::{Reader, Writer},
        *,
    },
};
use kernel_fidl::Status;
use std::collections::BTreeMap;

const USER_START: u64 = 0x1000_0000;
const USER_HEAP_VMAR_BASE: u64 = 0x0003_0000_0000;
const USER_HEAP_CHUNK_SIZE: u64 = 2 * 1024 * 1024;

#[test]
fn runtime_interrupts_are_shared_one_shot_and_snapshot_safe() {
    let mut rt = Runtime::new(TestBackend(0x4500_0000));
    rt.create_process("pci", "bexos.driver.pci_root", 2)
        .unwrap();
    let first = rt.bind_interrupt(16, 1).unwrap();
    let second = rt.bind_interrupt(16, 1).unwrap();
    assert_eq!(rt.bind_interrupt(16, 0), Err(Status::ErrAlreadyExists));

    assert_eq!(rt.deliver_interrupt(16, 1), InterruptDelivery::Signaled);
    assert_ne!(
        rt.object_signals(first).unwrap() & bexos_kernel_core::kernel_services::SIGNAL_READABLE,
        0
    );
    assert_eq!(rt.acknowledge_interrupt(first).unwrap(), (16, true));
    assert_eq!(rt.acknowledge_interrupt(second).unwrap(), (16, false));
    assert_eq!(rt.deliver_interrupt(16, 2), InterruptDelivery::Signaled);

    let snapshot = encode(&rt);
    let mut restored =
        Runtime::read_snapshot(TestBackend(0x4700_0000), &mut Reader::new(&snapshot)).unwrap();
    assert_ne!(
        restored.object_signals(first).unwrap()
            & bexos_kernel_core::kernel_services::SIGNAL_READABLE,
        0
    );
    assert_eq!(restored.deliver_interrupt(16, 3), InterruptDelivery::Masked);
    assert_eq!(restored.acknowledge_interrupt(first).unwrap(), (16, true));
    assert_eq!(restored.acknowledge_interrupt(second).unwrap(), (16, false));
}

#[test]
fn channel_identity_requires_an_owned_capability_and_survives_duplication() {
    let mut rt = Runtime::new(TestBackend(0x4500_0000));
    rt.create_process("appd", "appd", 0).unwrap();
    let (a, b) = rt.create_channel();
    let identity = rt.channel_identity(a).unwrap();
    assert_ne!(identity.0, identity.1);
    assert_eq!(rt.channel_identity(b).unwrap(), (identity.1, identity.0));
    let duplicate = rt.duplicate(a, 1 | 2 | 4 | 32).unwrap();
    assert_eq!(rt.channel_identity(duplicate).unwrap(), identity);
    rt.close(a).unwrap();
    assert!(rt.channel_identity(a).is_err());
    assert_eq!(rt.channel_identity(duplicate).unwrap(), identity);
    let (next, _) = rt.create_channel();
    assert_ne!(rt.channel_identity(next).unwrap().0, identity.0);
    let snapshot = encode(&rt);
    let restored =
        Runtime::read_snapshot(TestBackend(0x4700_0000), &mut Reader::new(&snapshot)).unwrap();
    assert_eq!(restored.channel_identity(duplicate).unwrap(), identity);
    let vmo = rt.create_vmo(4096, 0).unwrap();
    assert_eq!(rt.channel_identity(vmo), Err(Status::ErrInvalidArgs));
    rt.create_process("other", "other", 0).unwrap();
    rt.current = 1;
    assert!(rt.channel_identity(duplicate).is_err());
}

#[derive(Clone)]
struct TestBackend(u64);
impl Backend for TestBackend {
    fn allocate(&mut self, pages: u64) -> RuntimeResult<u64> {
        let base = self.0;
        self.0 += pages * 4096;
        Ok(base)
    }
    fn release(&mut self, _: u64, _: u64) {}
    fn new_space(&mut self) -> RuntimeResult<u64> {
        self.allocate(1)
    }
    fn destroy_space(&mut self, _: u64) {}
    fn map_page(&mut self, _: u64, _: u64, _: u64, _: u32, _: bool) -> RuntimeResult<()> {
        Ok(())
    }
    fn unmap_page(&mut self, _: u64, _: u64) {}
    fn read(&self, _: u64, _: &mut [u8]) {
        // Runtime syscall tests use zero-filled mapped pages; snapshot encode/decode
        // still must not depend on physical page contents.
    }
    fn write(&mut self, _: u64, _: &[u8]) {}
    fn free_pages(&self) -> u64 {
        0
    }
    fn reused_pages(&self) -> u64 {
        0
    }
}

#[derive(Clone, Default)]
struct MemoryBackend {
    next: u64,
    pages: BTreeMap<u64, [u8; 4096]>,
    maps: Vec<(u64, u64, u64, u32, bool)>,
    require_scrub: bool,
    revoked_pins: Vec<u64>,
}

impl MemoryBackend {
    fn new(base: u64) -> Self {
        Self {
            next: base,
            pages: BTreeMap::new(),
            maps: Vec::new(),
            require_scrub: false,
            revoked_pins: Vec::new(),
        }
    }
}

impl Backend for MemoryBackend {
    fn revoke_shared_pin(&mut self, token: u64) {
        self.revoked_pins.push(token);
    }
    fn allocate(&mut self, pages: u64) -> RuntimeResult<u64> {
        let base = self.next;
        for index in 0..pages {
            self.pages.insert(base + index * 4096, [0; 4096]);
        }
        self.next += pages * 4096;
        Ok(base)
    }

    fn release(&mut self, base: u64, pages: u64) {
        for index in 0..pages {
            if self.require_scrub {
                assert!(
                    self.pages[&(base + index * 4096)]
                        .iter()
                        .all(|byte| *byte == 0),
                    "private data reached the free-frame allocator"
                );
            }
            self.pages.remove(&(base + index * 4096));
        }
    }

    fn new_space(&mut self) -> RuntimeResult<u64> {
        self.allocate(1)
    }

    fn destroy_space(&mut self, _: u64) {}

    fn map_page(
        &mut self,
        root: u64,
        va: u64,
        pa: u64,
        rights: u32,
        device: bool,
    ) -> RuntimeResult<()> {
        self.maps.push((root, va, pa, rights, device));
        Ok(())
    }

    fn unmap_page(&mut self, _: u64, _: u64) {}

    fn read(&self, pa: u64, out: &mut [u8]) {
        let base = pa & !4095;
        let offset = (pa - base) as usize;
        if let Some(page) = self.pages.get(&base) {
            out.copy_from_slice(&page[offset..offset + out.len()]);
        } else {
            out.fill(0);
        }
    }

    fn write(&mut self, pa: u64, bytes: &[u8]) {
        let base = pa & !4095;
        let offset = (pa - base) as usize;
        let page = self.pages.entry(base).or_insert([0; 4096]);
        page[offset..offset + bytes.len()].copy_from_slice(bytes);
    }

    fn free_pages(&self) -> u64 {
        0
    }

    fn reused_pages(&self) -> u64 {
        0
    }
}

#[test]
fn runtime_asids_and_tls_survive_snapshot_roundtrip() {
    let mut rt = Runtime::with_asid_support(TestBackend(0x4500_0000), AsidSupport::Bits8);
    rt.set_boot_entropy_seed(Some([0x10, 0x20, 0x30, 0x40]));
    rt.create_process("appd", "appd", 0).unwrap();
    rt.processes[0].running = true;
    rt.processes[0].context.thread_pointer = 0x1234_5678;
    let pac_key = rt.processes[0].userspace_pac_key;

    let bytes = encode(&rt);
    let restored = Runtime::read_snapshot(TestBackend(0x4600_0000), &mut Reader::new(&bytes))
        .expect("runtime snapshot");

    assert_eq!(restored.processes[0].asid, 1);
    assert_eq!(restored.processes[0].userspace_pac_key, pac_key);
    assert_eq!(restored.processes[0].context.thread_pointer, 0x1234_5678);
}
fn encode<B: Backend>(rt: &Runtime<B>) -> Vec<u8> {
    let mut bytes = vec![0; 32768];
    let mut writer = Writer::new(&mut bytes);
    rt.write_snapshot(&mut writer).unwrap();
    let len = writer.len();
    bytes.truncate(len);
    bytes
}

#[test]
fn runtime_allows_only_trusted_virtio_drivers_to_map_qemu_pci_ranges() {
    for package in [
        "bexos.driver.network.virtio_net",
        "bexos.driver.display.virtio_gpu",
        "bexos.driver.input.virtio",
        "bexos.driver.serial.virtio_console",
    ] {
        let mut rt = Runtime::new(MemoryBackend::new(0x4500_0000));
        rt.create_process("appd", "appd", 2).unwrap();
        rt.create_process("virtio", package, 2).unwrap();
        rt.current = 1;
        let pci = if bexos_boot::ARCHITECTURE_ID == 1 {
            0x3f00_0000
        } else {
            0xb000_0000
        };
        assert!(rt.physical_vmo(pci, 0x0100_0000).is_ok());
        let mmio = if bexos_boot::ARCHITECTURE_ID == 1 {
            0x1000_0000
        } else {
            0xb100_0000
        };
        assert!(rt.physical_vmo(mmio, 4096).is_ok());
        assert_eq!(
            rt.physical_vmo(0x4000_0000, 4096),
            Err(Status::ErrAccessDenied)
        );
        assert_eq!(
            rt.physical_vmo(0x0e10_0000, 4096),
            Err(Status::ErrAccessDenied)
        );
        rt.current = 0;
        rt.create_process("untrusted_virtio", package, 0).unwrap();
        rt.current = 2;
        assert_eq!(rt.physical_vmo(pci, 4096), Err(Status::ErrAccessDenied));
    }
}

#[test]
fn shared_device_memory_is_cached_nonexecutable_and_never_reclaimed_as_ram() {
    let mut rt = Runtime::new(MemoryBackend::new(0x4500_0000));
    rt.create_process("appd", "appd", 2).unwrap();
    rt.create_process("gpu", "bexos.driver.display.virtio_gpu", 2)
        .unwrap();
    let base = if bexos_boot::ARCHITECTURE_ID == 1 {
        0x2000_0000
    } else {
        0xc000_0000
    };
    assert_eq!(
        rt.shared_device_vmo(base, 4096),
        Err(Status::ErrAccessDenied)
    );
    rt.current = 1;
    let handle = rt.shared_device_vmo(base, 4096).unwrap();
    assert!(rt.shared_device_vmo_idle(handle).unwrap());
    let duplicate = rt.duplicate(handle, 1 | 2 | 4 | 16 | 32).unwrap();
    assert!(!rt.shared_device_vmo_idle(handle).unwrap());
    rt.close(duplicate).unwrap();
    let va = rt.map(None, handle, 0, 4096, 0, 6).unwrap();
    assert!(!rt.shared_device_vmo_idle(handle).unwrap());
    assert!(!rt.backend.maps.last().unwrap().4);
    assert_eq!(
        rt.map(None, handle, 0, 4096, 0, 2 | 8),
        Err(Status::ErrAccessDenied)
    );
    assert!(rt.pin(handle).is_err());
    let snapshot = encode(&rt);
    let mut restored =
        Runtime::read_snapshot(rt.backend.clone(), &mut Reader::new(&snapshot)).unwrap();
    restored.current = 1;
    let alias = restored.map(None, handle, 0, 4096, 0, 6).unwrap();
    assert!(!restored.backend.maps.last().unwrap().4);
    restored.unmap(None, alias, 4096).unwrap();
    restored.unmap(None, va, 4096).unwrap();
    assert!(restored.shared_device_vmo_idle(handle).unwrap());
    let pages = restored.backend.pages.clone();
    restored.backend.require_scrub = true;
    restored.close(handle).unwrap();
    assert_eq!(restored.backend.pages, pages);
}

#[test]
fn private_vmo_pages_are_scrubbed_only_after_the_last_owner_releases_them() {
    for (flags, fragmented) in [(0, false), (0, true), (2, false)] {
        let mut rt = Runtime::new(MemoryBackend::new(0x4500_0000));
        let (_, space) = rt.create_process("owner", "owner", 2).unwrap();
        let vmo = rt.create_vmo(8192, flags).unwrap();
        let id = rt.vmo_for(vmo, 2).unwrap();
        rt.map(Some(space), vmo, 0, 8192, 0xb000_0000, 6).unwrap();
        rt.copy_to_user(0xb000_0000, b"private key material")
            .unwrap();
        let foreign = fragmented.then(|| {
            let page = rt.backend.allocate(1).unwrap();
            rt.backend.write(page, b"other owner");
            page
        });
        rt.copy_to_user(0xb000_1000, b"private key material")
            .unwrap();
        rt.backend.require_scrub = true;
        rt.close(vmo).unwrap();
        let mut still_owned = [0; 20];
        rt.copy_from_user(0xb000_0000, &mut still_owned).unwrap();
        assert_eq!(&still_owned, b"private key material");
        rt.unmap(Some(space), 0xb000_0000, 8192).unwrap();
        assert!(rt.vmos[id].is_none());
        if let Some(foreign) = foreign {
            assert_eq!(&rt.backend.pages[&foreign][..11], b"other owner");
        }
    }
}

#[test]
fn lazy_anonymous_vmo_reads_zero_and_commits_one_page_on_first_write() {
    use bexos_kernel_core::runtime::VmoBacking;

    let mut rt = Runtime::new(MemoryBackend::new(0x4500_0000));
    let (_, space) = rt.create_process("appd", "appd", 2).unwrap();
    let vmo = rt.create_vmo(8192, 0).unwrap();
    let id = rt.vmo_for(vmo, 2).unwrap();
    assert!(matches!(
        rt.vmos[id].as_ref().unwrap().backing,
        VmoBacking::LazyAnonymous { .. }
    ));
    rt.map(Some(space), vmo, 0, 8192, 0xb000_0000, 6).unwrap();
    assert!(
        rt.backend
            .maps
            .iter()
            .filter(|(_, va, _, rights, _)| *va >= 0xb000_0000 && *rights == 2)
            .count()
            >= 2
    );

    let mut bytes = [0xaa; 16];
    rt.copy_from_user(0xb000_1000, &mut bytes).unwrap();
    assert_eq!(bytes, [0; 16]);
    rt.copy_to_user(0xb000_0008, b"commit").unwrap();
    let mut out = [0; 8];
    rt.copy_from_user(0xb000_0008, &mut out[..6]).unwrap();
    assert_eq!(&out[..6], b"commit");

    let VmoBacking::LazyAnonymous { pages } = &rt.vmos[id].as_ref().unwrap().backing else {
        panic!("lazy backing should remain page-based until pin");
    };
    assert!(pages[0].is_some());
    assert!(pages[1].is_none());
}

#[test]
fn anonymous_vmo_metadata_exhaustion_returns_no_memory_and_can_retry() {
    let mut rt = Runtime::new(MemoryBackend::new(0x4500_0000));
    rt.create_process("appd", "appd", 2).unwrap();
    // Initialize the shared zero page before injecting a metadata failure.
    let initial = rt.create_vmo(4096, 0).unwrap();
    let before = rt.vmos.len();
    let result = crate::allocation_failure::deny_next(
        (64 * 1024 * 1024 / 4096) * core::mem::size_of::<Option<u64>>(),
        || rt.create_vmo(64 * 1024 * 1024, 0),
    );
    assert_eq!(result, Err(Status::ErrNoMemory));
    assert_eq!(rt.vmos.len(), before);
    assert!(rt.vmo_for(initial, 2).is_ok());
    assert!(rt.create_vmo(64 * 1024 * 1024, 0).is_ok());
}

#[test]
fn committing_shared_zero_pages_updates_all_aliases() {
    for operation in 0..3 {
        let mut rt = Runtime::new(MemoryBackend::new(0x4500_0000));
        let (_, writer_space) = rt.create_process("writer", "writer", 2).unwrap();
        let (_, reader_space) = rt.create_process("reader", "reader", 2).unwrap();
        let vmo = rt.create_vmo(8192, 0).unwrap();
        rt.map(Some(writer_space), vmo, 0, 8192, 0xb000_0000, 6)
            .unwrap();
        rt.map(Some(reader_space), vmo, 4096, 4096, 0xc000_0000, 2)
            .unwrap();
        let zero = rt.backend.maps.last().unwrap().2;
        match operation {
            0 => rt.commit_user_write_fault(0xb000_1000).unwrap(),
            1 => rt.copy_to_user(0xb000_1000, b"shared").unwrap(),
            _ => rt.commit_user_range(0xb000_0000, 8192).unwrap(),
        }
        let writer = rt
            .backend
            .maps
            .iter()
            .rev()
            .find(|m| m.1 == 0xb000_1000)
            .unwrap();
        let reader = rt
            .backend
            .maps
            .iter()
            .rev()
            .find(|m| m.1 == 0xc000_0000)
            .unwrap();
        assert_ne!(
            writer.2, zero,
            "operation {operation} must commit a real page"
        );
        assert_eq!(
            writer.2, reader.2,
            "operation {operation} must update the reader alias"
        );
        assert_eq!(writer.3, 6);
        assert_eq!(reader.3, 2, "read-only aliases must stay read-only");
    }
}

#[test]
fn lazy_anonymous_vmo_pin_materializes_contiguous_zero_filled_backing() {
    use bexos_kernel_core::runtime::VmoBacking;

    let mut rt = Runtime::new(MemoryBackend::new(0x4600_0000));
    let (_, space) = rt
        .create_process("driver", "bexos.driver.debugd", 2)
        .unwrap();
    let vmo = rt.create_vmo(8192, 0).unwrap();
    rt.map(Some(space), vmo, 0, 8192, 0xb100_0000, 6).unwrap();
    rt.copy_to_user(0xb100_0000, b"dma").unwrap();
    let (base, pin) = rt.pin(vmo).unwrap();
    let id = rt.vmo_for(vmo, 2).unwrap();
    assert_eq!(
        rt.vmos[id].as_ref().unwrap().backing,
        VmoBacking::Contiguous { base }
    );
    let mut first = [0; 3];
    rt.backend.read(base, &mut first);
    assert_eq!(&first, b"dma");
    let mut second = [0xff; 16];
    rt.backend.read(base + 4096, &mut second);
    assert_eq!(second, [0; 16]);
    rt.unpin(pin).unwrap();
}

#[test]
fn secure_sharing_requires_owned_live_pins_and_revokes_on_exit() {
    use bexos_kernel_core::runtime::handover::AUTH_SECURE_MONITOR;
    let mut rt = Runtime::new(MemoryBackend::new(0x4600_0000));
    rt.create_process("teed", "bexos.service.teed", 0).unwrap();
    rt.processes[0].authority = AUTH_SECURE_MONITOR;
    let vmo = rt.create_vmo(8192, 0).unwrap();
    let (address, pin) = rt.pin(vmo).unwrap();
    assert_eq!(rt.secure_pin_for_range(address, 8192), Ok(pin));
    assert_eq!(rt.secure_pin_for_range(address + 4096, 4096), Ok(pin));
    for (base, size) in [
        (address, 12288),
        (address - 4096, 4096),
        (u64::MAX - 4095, 8192),
        (address + 1, 4096),
        (address, 0),
    ] {
        assert!(rt.secure_pin_for_range(base, size).is_err());
    }
    rt.processes[0].authority |= bexos_kernel_core::runtime::handover::AUTH_APP_MANAGER;
    rt.create_process("candidate", "bexos.service.teed", 0)
        .unwrap();
    rt.processes[1].authority = AUTH_SECURE_MONITOR;
    rt.current = 1;
    assert_eq!(
        rt.secure_pin_for_range(address, 4096),
        Err(Status::ErrAccessDenied)
    );
    assert_eq!(rt.validate_secure_pin(pin), Err(Status::ErrAccessDenied));
    rt.current = 0;
    rt.processes[0].quarantined = true;
    assert_eq!(rt.validate_secure_pin(pin), Err(Status::ErrAccessDenied));
    rt.processes[0].quarantined = false;
    rt.unpin(pin).unwrap();
    assert_eq!(rt.backend.revoked_pins, [pin]);
    assert_eq!(rt.validate_secure_pin(pin), Err(Status::ErrInvalidHandle));
    let (_, next) = rt.pin(vmo).unwrap();
    assert_ne!(next, pin);
    rt.exit_current();
    assert_eq!(rt.backend.revoked_pins, [pin, next]);
    assert!(rt.secure_pin_for_range(address, 4096).is_err());
}

#[test]
fn runtime_commit_user_range_materializes_all_covered_pages() {
    use bexos_kernel_core::runtime::VmoBacking;

    let mut rt = Runtime::new(MemoryBackend::new(0x4680_0000));
    let (_, space) = rt.create_process("app", "com.example.app", 0).unwrap();
    let vmo = rt.create_vmo(8192, 0).unwrap();
    rt.map(Some(space), vmo, 0, 8192, 0xb180_0000, 6).unwrap();

    rt.commit_user_range(0xb180_0080, 5000).unwrap();

    let id = rt.vmo_for(vmo, 2).unwrap();
    let VmoBacking::LazyAnonymous { pages } = &rt.vmos[id].as_ref().unwrap().backing else {
        panic!("ordinary VMO should retain lazy page bookkeeping");
    };
    assert!(pages[0].is_some());
    assert!(pages[1].is_some());
}

#[test]
fn runtime_app_manager_can_create_short_lived_contiguous_transfer_vmos() {
    use bexos_kernel_core::runtime::VmoBacking;

    let mut rt = Runtime::new(MemoryBackend::new(0x4700_0000));
    rt.create_process("appd", "bexos.platform.appd", 0).unwrap();

    let vmo = rt.create_vmo(8192, 2).unwrap();
    let id = rt.vmo_for(vmo, 2).unwrap();
    assert!(matches!(
        rt.vmos[id].as_ref().unwrap().backing,
        VmoBacking::Contiguous { .. }
    ));
}

#[test]
fn runtime_iommu_domains_own_dma_mappings_and_block_legacy_pins() {
    let mut rt = Runtime::new(MemoryBackend::new(0x4500_0000));
    rt.create_process("nvme", "bexos.driver.storage.nvme", 2)
        .unwrap();
    let dma = rt.create_vmo(4096, 2).unwrap();
    let domain = rt.create_iommu_domain(0x10, 48).unwrap();
    assert_eq!(
        rt.create_iommu_domain(0x10, 48),
        Err(Status::ErrAlreadyExists)
    );

    let (device_address, token) = rt.map_dma(domain, dma, 0, 4096, 2 | 4).unwrap();
    assert!(device_address >= 0x4500_0000);
    assert_eq!(device_address % 4096, 0);
    assert_eq!(rt.pin(dma), Err(Status::ErrAccessDenied));
    rt.unmap_dma(token).unwrap();
    rt.close(domain).unwrap();
}

#[test]
fn runtime_app_manager_can_create_driver_iommu_domains() {
    let mut rt = Runtime::new(TestBackend(0x4500_0000));
    rt.create_process("appd", "bexos.platform.appd", 0).unwrap();

    let domain = rt.create_iommu_domain(8, 48).unwrap();

    assert_ne!(domain, 0);
}

#[test]
fn runtime_socket_pair_moves_stream_bytes_and_half_closes() {
    let mut rt = Runtime::new(TestBackend(0x4500_0000));
    rt.create_process("appd", "appd", 0).unwrap();
    let (a, b) = rt.create_socket_pair();

    assert_eq!(rt.write_socket(a, b"hello").unwrap(), 5);
    assert_eq!(rt.socket_info(b).unwrap().readable_bytes, 5);
    assert_eq!(rt.read_socket(b, 2).unwrap(), b"he");
    assert_eq!(rt.read_socket(b, 8).unwrap(), b"llo");
    assert_eq!(rt.read_socket(b, 8), Err(Status::ErrTimedOut));

    rt.shutdown_socket(a, false, true).unwrap();
    let info = rt.socket_info(b).unwrap();
    assert!(info.peer_write_closed);
    assert_eq!(rt.read_socket(b, 8), Err(Status::ErrPeerClosed));
}

#[test]
fn runtime_sync_channel_call_read_reply_roundtrip() {
    let mut rt = Runtime::new(TestBackend(0x4500_0000));
    rt.create_process("appd", "appd", 0).unwrap();
    let (client, server) = rt.create_channel();
    rt.set_channel_policy(client, true, true).unwrap();

    assert_eq!(
        rt.call(client, b"poll", &[], 0, 32, 0),
        Err(Status::ErrTimedOut)
    );
    assert_eq!(rt.read_call(server, 0, 32, 0), Err(Status::ErrTimedOut));

    assert_eq!(
        rt.call(client, b"request", &[], i64::MAX, 32, 0),
        Err(Status::ErrTimedOut)
    );
    assert_eq!(
        rt.call(client, b"duplicate", &[], i64::MAX, 32, 0),
        Err(Status::ErrTimedOut)
    );
    assert_eq!(
        rt.wait_many(
            &[(server, bexos_kernel_core::kernel_services::SIGNAL_READABLE)],
            0
        )
        .unwrap(),
        (
            0,
            bexos_kernel_core::kernel_services::SIGNAL_READABLE
                | bexos_kernel_core::kernel_services::SIGNAL_WRITABLE
        )
    );
    assert_eq!(
        rt.read_call(server, 0, 3, 0),
        Err(Status::ErrBufferTooSmall)
    );
    let (request, token) = rt.read_call(server, 0, 32, 0).unwrap();
    assert_eq!(request.bytes, b"request");
    assert_ne!(token, 0);
    rt.reply_call(token, b"reply", &[]).unwrap();
    assert_eq!(
        rt.wait_many(
            &[(client, bexos_kernel_core::kernel_services::SIGNAL_READABLE)],
            0
        )
        .unwrap(),
        (
            0,
            bexos_kernel_core::kernel_services::SIGNAL_READABLE
                | bexos_kernel_core::kernel_services::SIGNAL_WRITABLE
        )
    );
    let reply = rt.call(client, b"ignored", &[], i64::MAX, 32, 0).unwrap();
    assert_eq!(reply.bytes, b"reply");
    assert_eq!(rt.read_call(server, 0, 32, 0), Err(Status::ErrTimedOut));
    assert_eq!(
        rt.reply_call(token, b"again", &[]),
        Err(Status::ErrInvalidHandle)
    );
}

#[test]
fn invalid_initial_tls_pointer_does_not_start_a_thread_or_transfer_its_channel() {
    let mut rt = Runtime::new(TestBackend(0x4500_0000));
    rt.create_process("appd", "appd", 0).unwrap();
    let (process, space) = rt.create_process("child", "child", 0).unwrap();
    let text = rt.create_vmo(4096, 0).unwrap();
    rt.map(Some(space), text, 0, 4096, 0x8000_0000, 10).unwrap();
    let stack = rt.create_vmo(8192, 0).unwrap();
    rt.map(Some(space), stack, 0, 8192, 0x8001_0000, 6).unwrap();
    let (startup, _) = rt.create_channel();
    for pointer in [bexos_boot::USER_END, 0x0000_8000_0000_0000, u64::MAX] {
        assert_eq!(
            rt.start_with_thread_pointer(
                process,
                space,
                0x8000_0000,
                0x8001_2000,
                pointer,
                startup,
            ),
            Err(Status::ErrInvalidArgs),
        );
        assert!(!rt.processes[1].running);
        assert!(rt.threads.is_empty());
        assert_eq!(rt.capability(startup, 0).unwrap().owner, 0);
    }
    rt.start_with_thread_pointer(
        process,
        space,
        0x8000_0000,
        0x8001_2000,
        0x8001_0000,
        startup,
    )
    .unwrap();
    assert!(rt.processes[1].running);
    assert_eq!(rt.threads[0].context.thread_pointer(), 0x8001_0000);
    rt.current = 1;
    assert_eq!(rt.capability(startup, 0).unwrap().owner, 1);
    assert_eq!(
        rt.create_thread_current_with_thread_pointer(0x8000_0000, 0x8001_1800, u64::MAX, 0,),
        Err(Status::ErrInvalidArgs),
    );
    assert_eq!(rt.threads.len(), 1);
}

#[test]
fn runtime_schedules_created_threads_and_futex_wake_unblocks() {
    let mut rt = Runtime::new(TestBackend(0x4500_0000));
    let (process, space) = rt.create_process("appd", "appd", 0).unwrap();
    let text = rt.create_vmo(4096, 0).unwrap();
    rt.map(Some(space), text, 0, 4096, 0x8000_0000, 10).unwrap();
    let stack = rt.create_vmo(8192, 0).unwrap();
    rt.map(Some(space), stack, 0, 8192, 0x8001_0000, 6).unwrap();
    rt.close(text).unwrap();
    rt.close(stack).unwrap();

    rt.start(process, space, 0x8000_0000, 0x8001_2000, 0)
        .unwrap();
    let second = rt
        .create_thread_current(0x8000_0000, 0x8001_1800, 0)
        .unwrap();
    assert!(matches!(
        rt.capability(second, 0).unwrap().object,
        bexos_kernel_core::runtime::Object::Thread(1)
    ));

    let mut frame = rt.threads[0].context;
    rt.schedule(&mut frame);
    assert_eq!(rt.current_thread, 1);

    rt.futex_wait_current(0x8001_0000, 0).unwrap();
    assert!(rt.current_thread_blocked());
    let original_process = rt.current;
    let unrelated_process = rt.processes.len();
    rt.create_process("unrelated", "unrelated", 0).unwrap();
    rt.current = unrelated_process;
    assert_eq!(rt.futex_wake(0x8001_0000, 1).unwrap(), 0);
    rt.current = original_process;
    assert_eq!(rt.futex_wake(0x8001_0000, 1).unwrap(), 1);
    assert!(!rt.current_thread_blocked());
    assert_eq!(
        rt.futex_wait_current(0x8001_0000, 1),
        Err(kernel_fidl::Status::ErrResourceExhausted)
    );
    assert!(!rt.current_thread_blocked());
    assert!(rt.current_thread_can_return());
    rt.exit_current();
    assert!(
        rt.processes[rt.current].running,
        "sibling thread keeps process alive"
    );
    assert!(
        !rt.current_thread_can_return(),
        "an exited thread cannot return from its syscall"
    );
    rt.schedule_at_on_cpu(0, rt.scheduler.now_ns(), &mut frame);
    assert_eq!(rt.current_thread, 0);
    assert!(rt.current_thread_can_return());
}

#[test]
fn thread_exit_wakes_joiners_and_preserves_blocked_siblings() {
    use bexos_kernel_core::kernel_services::{SIGNAL_READABLE, SIGNAL_TERMINATED};
    for join_worker in [false, true] {
        let mut rt = Runtime::new(TestBackend(0x4500_0000));
        let (process, space) = rt.create_process("appd", "appd", 0).unwrap();
        let text = rt.create_vmo(4096, 0).unwrap();
        rt.map(Some(space), text, 0, 4096, 0x8000_0000, 10).unwrap();
        let stack = rt.create_vmo(8192, 0).unwrap();
        rt.map(Some(space), stack, 0, 8192, 0x8001_0000, 6).unwrap();
        rt.start(process, space, 0x8000_0000, 0x8001_2000, 0)
            .unwrap();
        let worker = rt
            .create_thread_current(0x8000_0000, 0x8001_1800, 0)
            .unwrap();
        let (sender, receiver) = rt.create_channel();
        let waited = if join_worker {
            (worker, SIGNAL_TERMINATED)
        } else {
            (receiver, SIGNAL_READABLE)
        };
        assert_eq!(rt.wait_many(&[waited], -1), Err(Status::ErrTimedOut));
        assert!(rt.current_thread_blocked());
        let mut frame = rt.threads[0].context;
        rt.schedule(&mut frame);
        assert_eq!(rt.current_thread, 1);
        rt.exit_current_with_code(23);
        assert!(!rt.processes[0].exited, "blocked siblings are still alive");
        assert_eq!(rt.threads[1].exit_code, 23);
        assert_eq!(rt.threads[0].blocked_wait_many, !join_worker);
        if !join_worker {
            rt.write_message(sender, b"wake", &[]).unwrap();
        }
        rt.schedule_at_on_cpu(0, rt.scheduler.now_ns(), &mut frame);
        assert_eq!(rt.current_thread, 0);
        assert!(rt.current_thread_can_return());
        assert_eq!(
            rt.wait_many(&[(worker, SIGNAL_TERMINATED)], 0).unwrap(),
            (0, SIGNAL_TERMINATED)
        );
        rt.exit_current();
        assert!(
            rt.processes[0].exited,
            "the final exited thread retires the process"
        );
    }
}

#[test]
fn process_termination_wakes_owners_and_remains_observable_after_snapshot() {
    use bexos_kernel_core::kernel_services::SIGNAL_TERMINATED;
    for forced in [false, true] {
        let mut rt = Runtime::new(TestBackend(0x4500_0000));
        let (manager, manager_space) = rt.create_process("manager", "manager", 0).unwrap();
        let (child, child_space) = rt.create_process("input", "input", 1).unwrap();
        let text = rt.create_vmo(4096, 0).unwrap();
        let stack = rt.create_vmo(8192, 0).unwrap();
        for space in [manager_space, child_space] {
            rt.map(Some(space), text, 0, 4096, 0x8000_0000, 10).unwrap();
            rt.map(Some(space), stack, 0, 8192, 0x8001_0000, 6).unwrap();
        }
        rt.start(manager, manager_space, 0x8000_0000, 0x8001_2000, 0)
            .unwrap();
        rt.start(child, child_space, 0x8000_0000, 0x8001_2000, 0)
            .unwrap();
        assert_eq!(rt.object_signals(child).unwrap(), 0);
        assert_eq!(
            rt.wait_many(&[(child, SIGNAL_TERMINATED)], -1),
            Err(Status::ErrTimedOut)
        );
        assert!(rt.current_thread_blocked());
        if forced {
            rt.terminate_process(child, -1).unwrap();
        } else {
            rt.current = 1;
            rt.current_thread = 1;
            rt.exit_current();
            rt.current = 0;
            rt.current_thread = 0;
        }
        assert!(!rt.current_thread_blocked());
        assert_eq!(
            rt.wait_many(&[(child, SIGNAL_TERMINATED)], 0),
            Ok((0, SIGNAL_TERMINATED))
        );
        let bytes = encode(&rt);
        let mut restored =
            Runtime::read_snapshot(rt.backend.clone(), &mut Reader::new(&bytes)).unwrap();
        assert_eq!(
            restored.wait_many(&[(child, SIGNAL_TERMINATED)], 0),
            Ok((0, SIGNAL_TERMINATED))
        );
    }
}

#[test]
fn futex_deadline_survives_snapshot_and_releases_runtime_block_state() {
    use bexos_kernel_core::sched::BlockReason;
    use kernel_fidl::Status;
    let mut rt = Runtime::new(TestBackend(0x4500_0000));
    let (process, space) = rt.create_process("appd", "appd", 0).unwrap();
    let text = rt.create_vmo(4096, 0).unwrap();
    rt.map(Some(space), text, 0, 4096, 0x8000_0000, 10).unwrap();
    let stack = rt.create_vmo(8192, 0).unwrap();
    rt.map(Some(space), stack, 0, 8192, 0x8001_0000, 6).unwrap();
    rt.close(text).unwrap();
    rt.close(stack).unwrap();
    rt.start(process, space, 0x8000_0000, 0x8001_2000, 0)
        .unwrap();
    rt.create_thread_current(0x8000_0000, 0x8001_1800, 0)
        .unwrap();
    let mut frame = rt.threads[0].context;
    rt.schedule(&mut frame);
    let waiter = rt.current_thread;
    assert_eq!(
        rt.futex_wait_current_timeout(0x8001_0000, 0, 0),
        Err(Status::ErrTimedOut)
    );
    assert_eq!(
        rt.futex_wait_current_timeout(0x8001_0000, 0, -2),
        Err(Status::ErrInvalidArgs)
    );
    let deadline = rt.scheduler.now_ns() + 50_000;
    rt.futex_wait_current_timeout(0x8001_0000, 0, 50_000)
        .unwrap();
    assert_eq!(
        rt.scheduler.task(waiter as u64 + 1).unwrap().block_reason,
        Some(BlockReason::FutexUntil {
            uaddr: 0x8001_0000,
            deadline_nanos: deadline
        })
    );
    let bytes = encode(&rt);
    let mut restored =
        Runtime::read_snapshot(TestBackend(0x4600_0000), &mut Reader::new(&bytes)).unwrap();
    restored.schedule_at_on_cpu(0, deadline - 1, &mut frame);
    assert!(restored.threads[waiter].blocked_futex.is_some());
    restored.schedule_at_on_cpu(0, deadline, &mut frame);
    assert!(restored.threads[waiter].blocked_futex.is_none());
    assert!(restored.threads[waiter].running);
    assert!(
        restored
            .scheduler
            .task(waiter as u64 + 1)
            .unwrap()
            .block_reason
            .is_none()
    );
    assert_eq!(restored.futex_wake(0x8001_0000, 1).unwrap(), 0);
}

#[test]
fn runtime_profile_affinity_yield_and_wait_many_are_guest_visible() {
    let mut rt = Runtime::with_cpu_count(TestBackend(0x4500_0000), 4, AsidSupport::None);
    let (process, space) = rt
        .create_process_with_policy("appd", "appd", 0, 1, true)
        .unwrap();
    let text = rt.create_vmo(4096, 0).unwrap();
    rt.map(Some(space), text, 0, 4096, 0x8000_0000, 10).unwrap();
    let stack = rt.create_vmo(8192, 0).unwrap();
    rt.map(Some(space), stack, 0, 8192, 0x8001_0000, 6).unwrap();
    rt.start(process, space, 0x8000_0000, 0x8001_2000, 0)
        .unwrap();
    let second = rt
        .create_thread_current(0x8000_0000, 0x8001_1800, 0)
        .unwrap();

    let profile = rt
        .create_scheduling_profile(SchedulingProfile::Fair(FairProfile {
            priority: 200,
            weight: 4,
        }))
        .unwrap();
    rt.set_thread_profile(second, profile).unwrap();
    rt.set_thread_cpu_affinity(second, 0b0101).unwrap();
    assert_eq!(
        rt.set_thread_cpu_affinity(second, 0b1_0000),
        Err(Status::ErrInvalidArgs)
    );
    rt.yield_current_on_cpu(0, second).unwrap();
    assert_eq!(rt.current_thread, 0);

    let (sender, receiver) = rt.create_channel();
    assert_eq!(
        rt.wait_many(
            &[(
                receiver,
                bexos_kernel_core::kernel_services::SIGNAL_READABLE
            )],
            0
        ),
        Err(Status::ErrTimedOut)
    );
    rt.write_message(sender, b"ready", &[]).unwrap();
    assert_eq!(
        rt.wait_many(
            &[(
                receiver,
                bexos_kernel_core::kernel_services::SIGNAL_READABLE
            )],
            0
        )
        .unwrap(),
        (
            0,
            bexos_kernel_core::kernel_services::SIGNAL_READABLE
                | bexos_kernel_core::kernel_services::SIGNAL_WRITABLE
        )
    );

    let bytes = encode(&rt);
    let restored = Runtime::read_snapshot(TestBackend(0x4600_0000), &mut Reader::new(&bytes))
        .expect("runtime snapshot");
    assert!(matches!(
        restored.capability(profile, 0).unwrap().object,
        bexos_kernel_core::runtime::Object::Profile(0)
    ));
}

#[test]
fn runtime_realtime_profiles_require_manifest_permission() {
    let mut rt = Runtime::new(TestBackend(0x4500_0000));
    rt.create_process_with_policy("appd", "appd", 0, 1, false)
        .unwrap();

    assert_eq!(
        rt.create_scheduling_profile(SchedulingProfile::Fair(FairProfile {
            priority: 128,
            weight: 1,
        })),
        Err(Status::ErrAccessDenied)
    );
    assert_eq!(
        rt.create_scheduling_profile(SchedulingProfile::Deadline(DeadlineProfile {
            capacity_ns: 1_000_000,
            deadline_ns: 4_000_000,
            period_ns: 4_000_000,
        })),
        Err(Status::ErrAccessDenied)
    );
}

#[test]
fn runtime_heap_vmar_maps_chunks_and_rejects_execute() {
    let mut rt = Runtime::new(TestBackend(0x4500_0000));
    let (_, space) = rt.create_process("appd", "appd", 0).unwrap();
    let heap = rt.heap_vmar_handle().unwrap();
    let vmo = rt.create_vmo(USER_HEAP_CHUNK_SIZE, 0).unwrap();

    let mapped = rt
        .map_vmo_in_vmar(heap, vmo, 0, 0, USER_HEAP_CHUNK_SIZE, 0x1 | 0x2)
        .unwrap();
    assert_eq!(mapped, USER_HEAP_VMAR_BASE);
    assert_eq!(
        rt.map_vmo_in_vmar(heap, vmo, 0, 0, 4096, 0x1 | 0x2 | 0x4),
        Err(Status::ErrAccessDenied)
    );
    rt.unmap_in_vmar(heap, mapped, USER_HEAP_CHUNK_SIZE)
        .unwrap();

    let direct = rt.map(Some(space), vmo, 0, 4096, 0, 6).unwrap();
    assert!(direct >= USER_START);
    assert_ne!(direct, USER_HEAP_VMAR_BASE);
}

#[test]
fn runtime_vmar_state_survives_snapshot_roundtrip() {
    let mut rt = Runtime::new(TestBackend(0x4500_0000));
    rt.create_process("appd", "appd", 0).unwrap();
    let heap = rt.heap_vmar_handle().unwrap();
    let vmo = rt.create_vmo(4096, 0).unwrap();
    let mapped = rt
        .map_vmo_in_vmar(heap, vmo, 0, 0, 4096, 0x1 | 0x2)
        .unwrap();

    let bytes = encode(&rt);
    let mut restored =
        Runtime::read_snapshot(TestBackend(0x4600_0000), &mut Reader::new(&bytes)).unwrap();
    let restored_heap = restored.heap_vmar_handle().unwrap();
    restored.unmap_in_vmar(restored_heap, mapped, 4096).unwrap();
}

#[test]
fn runtime_socket_state_survives_snapshot_roundtrip() {
    let mut rt = Runtime::new(TestBackend(0x4500_0000));
    rt.create_process("appd", "appd", 0).unwrap();
    let (a, b) = rt.create_socket_pair();
    rt.write_socket(a, b"snapshot").unwrap();
    rt.shutdown_socket(a, false, true).unwrap();

    let bytes = encode(&rt);
    let mut restored = Runtime::read_snapshot(TestBackend(0x4600_0000), &mut Reader::new(&bytes))
        .expect("runtime snapshot");

    assert_eq!(restored.read_socket(b, 64).unwrap(), b"snapshot");
    assert_eq!(restored.read_socket(b, 64), Err(Status::ErrPeerClosed));
}

#[test]
fn runtime_sync_calls_survive_snapshot_roundtrip() {
    let mut rt = Runtime::new(TestBackend(0x4500_0000));
    rt.create_process("appd", "appd", 0).unwrap();
    let (client, server) = rt.create_channel();

    assert_eq!(
        rt.call(client, b"snapshot request", &[], i64::MAX, 64, 0),
        Err(Status::ErrTimedOut)
    );
    let bytes = encode(&rt);
    let mut restored = Runtime::read_snapshot(TestBackend(0x4600_0000), &mut Reader::new(&bytes))
        .expect("runtime snapshot");
    let (request, token) = restored.read_call(server, 0, 64, 0).unwrap();
    assert_eq!(request.bytes, b"snapshot request");
    restored.reply_call(token, b"snapshot reply", &[]).unwrap();

    let bytes = encode(&restored);
    let mut restored_again =
        Runtime::read_snapshot(TestBackend(0x4700_0000), &mut Reader::new(&bytes))
            .expect("runtime snapshot");
    let reply = restored_again
        .call(client, b"ignored", &[], i64::MAX, 64, 0)
        .unwrap();
    assert_eq!(reply.bytes, b"snapshot reply");
}

fn copy_record<B: Backend>(source: &mut Runtime<B>, target: &mut Runtime<B>, key: u64) {
    let mut bytes = vec![0; 32768];
    let mut w = Writer::new(&mut bytes);
    source.write_record(key, &mut w).unwrap();
    let len = w.len();
    source.record_copied(key);
    target.adopt_record(&bytes[..len]).unwrap();
}

#[path = "reclamation_tests.rs"]
mod reclamation_tests;

#[test]
fn incremental_bulk_and_mutation_deltas_equal_a_frozen_snapshot() {
    let mut source = Runtime::new(TestBackend(0x4500_0000));
    let (_, space) = source.create_process("appd", "appd", 2).unwrap();
    source.processes[0].running = true;
    let (a, b) = source.create_channel();
    let dma = source.create_vmo(4096, 2).unwrap();
    source
        .map(Some(space), dma, 0, 4096, 0xb000_0000, 6)
        .unwrap();
    let (_, pin) = source.pin(dma).unwrap();
    let mut target = Runtime::new(source.backend.clone());
    let mut cursor = source.begin_live_snapshot(1024).unwrap();
    let mut step = 0;
    while let Some(key) = cursor.next() {
        copy_record(&mut source, &mut target, key);
        if step == 0 {
            source.write_message(a, b"bulk mutation", &[]).unwrap();
        }
        if step == 2 {
            let transferred = source.duplicate(dma, 3).unwrap();
            source
                .write_message(a, b"transfer", &[transferred])
                .unwrap();
            source
                .create_process("new client", "new client", 0)
                .unwrap();
        }
        step += 1;
    }
    source.read_message(b, 64, 1).unwrap();
    source.unpin(pin).unwrap();
    source.unmap(None, 0xb000_0000, 4096).unwrap();
    let mut context = source.processes[0].context;
    set_saved_context_word(&mut context, 19, 0xbeef);
    source.save_context(context);
    while let Some(key) = source.dirty_next().unwrap() {
        copy_record(&mut source, &mut target, key);
    }
    target.validate_live_snapshot().unwrap();
    source.end_live_snapshot();
    assert_eq!(encode(&source), encode(&target));
    assert_eq!(target.read_message(b, 64, 1).unwrap().bytes, b"transfer");
}

#[test]
fn incremental_dirty_overflow_rejects_migration_without_rejecting_ipc() {
    let mut source = Runtime::new(TestBackend(0x4500_0000));
    source.create_process("appd", "appd", 0).unwrap();
    let (a, b) = source.create_channel();
    source.begin_live_snapshot(1).unwrap();
    source.write_message(a, b"still accepted", &[]).unwrap();
    assert!(source.dirty_next().is_err());
    assert_eq!(
        source.read_message(b, 64, 0).unwrap().bytes,
        b"still accepted"
    );
    source.end_live_snapshot();
    assert!(source.begin_live_snapshot(1024).is_ok());
}

#[test]
fn handover_preserves_queued_handles_dma_and_transfers_appd_authority() {
    use bexos_kernel_core::runtime::{Object, handover::AUTH_APP_MANAGER};
    let mut rt = Runtime::new(TestBackend(0x4500_0000));
    let (_, old_space) = rt.create_process("appd", "appd", 2).unwrap();
    rt.processes[0].running = true;
    rt.create_process("client", "client", 0).unwrap();
    rt.processes[1].running = true;
    let (target, _) = rt.create_process("appd-new", "appd", 2).unwrap();
    let (server, client) = rt.create_channel();
    let client_cap = rt.capability(client, 0).unwrap();
    let client_end = rt.grant(1, client_cap.object, client_cap.rights);
    rt.close(client).unwrap();
    let dma = rt.create_vmo(4096, 2).unwrap();
    rt.map(Some(old_space), dma, 0, 4096, 0xb100_0000, 6)
        .unwrap();
    let executable = rt.create_vmo(4096, 0).unwrap();
    rt.map(Some(old_space), executable, 0, 4096, 0xb200_0000, 10)
        .unwrap();
    let (pa, pin) = rt.pin(dma).unwrap();
    rt.begin_handover(0, target, 1, 0, Default::default())
        .unwrap();
    rt.processes[2].running = true;
    rt.preserve_handle(server).unwrap();
    // appd self-replacement keeps its source-owned descriptor for the
    // candidate so the new appd can supervise its next replacement.
    rt.preserve_handle(target).unwrap();
    rt.preserve_mapping(dma, 0, 0xb100_0000, 4096, 6).unwrap();
    assert_eq!(
        rt.preserve_mapping(executable, 0, 0xb200_0000, 4096, 14),
        Err(Status::ErrAccessDenied)
    );
    rt.preserve_mapping(executable, 0, 0xb200_0000, 4096, 10)
        .unwrap();
    rt.preserve_pin(pin).unwrap();
    rt.handover_bulk(1).unwrap();
    rt.current = 1;
    let payload = rt.create_vmo(4096, 0).unwrap();
    rt.write_message(client_end, b"request during bulk", &[payload])
        .unwrap();
    rt.current = 2;
    assert!(rt.capability(server, 2).is_err());
    assert!(rt.pin(dma).is_err());
    assert!(rt.create_process("forged", "appd", 0).is_err());
    assert!(rt.physical_vmo(0x0900_0000, 4096).is_err());
    rt.current = 0;
    rt.handover_catch_up(2).unwrap();
    rt.quiesce_handover(7, 3).unwrap();
    rt.current = 1;
    rt.write_message(client_end, b"request during cutover", &[])
        .unwrap();
    rt.current = 2;
    assert!(rt.ready_handover(6, 4).is_err());
    rt.ready_handover(7, 5).unwrap();
    rt.commit_handover(6).unwrap();
    assert!(matches!(
        rt.capability(target, 0).unwrap().object,
        Object::Process(2)
    ));
    assert!(rt.processes[0].exited);
    assert_eq!(rt.processes[0].root, 0);
    assert!(rt.has_authority(AUTH_APP_MANAGER));
    assert!(rt.create_process("post-handover", "client", 0).is_ok());
    let first = rt.read_message(server, 64, 1).unwrap();
    assert_eq!(first.bytes, b"request during bulk");
    assert_eq!(first.handles, [payload]);
    assert_eq!(
        rt.read_message(server, 64, 0).unwrap().bytes,
        b"request during cutover"
    );
    assert!(rt.read_message(server, 64, 0).is_err());
    let Object::Vmo(id) = rt.capability(dma, 2).unwrap().object else {
        panic!()
    };
    assert_eq!(
        rt.vmos[id].as_ref().unwrap().backing,
        bexos_kernel_core::runtime::VmoBacking::Contiguous { base: pa }
    );
    assert_eq!(rt.pins[pin as usize - 1], Some((2, id)));
    assert_eq!(rt.processes[2].mappings[0].vmo, id);
    assert!(
        rt.processes[2]
            .mappings
            .iter()
            .any(|mapping| mapping.va == 0xb200_0000 && mapping.rights == 10)
    );
    rt.unpin(pin).unwrap();
    // Authority, DMA mappings, and dead process tombstones also survive a kernel update.
    let bytes = encode(&rt);
    let restored = Runtime::read_snapshot(rt.backend.clone(), &mut Reader::new(&bytes)).unwrap();
    assert!(restored.has_authority(AUTH_APP_MANAGER));
}

#[test]
fn handover_dma_tokens_do_not_alias_other_process_pins() {
    for commit in [false, true] {
        let mut rt = Runtime::new(TestBackend(0x4500_0000));
        rt.create_process("appd", "appd", 2).unwrap();
        rt.processes[0].running = true;
        let pinned = rt.create_vmo(4096, 2).unwrap();
        let (_, physical_pin) = rt.pin(pinned).unwrap();
        let (source, _) = rt.create_process("console", "console", 2).unwrap();
        let (target, _) = rt.create_process("console-new", "console", 2).unwrap();
        rt.processes[1].running = true;
        rt.current = 1;
        let dma = rt.create_vmo(4096, 2).unwrap();
        let domain = rt.create_iommu_domain(0x10, 48).unwrap();
        let (_, dma_pin) = rt.map_dma(domain, dma, 0, 4096, 6).unwrap();
        assert_ne!(physical_pin, dma_pin);
        assert_eq!(rt.unmap_dma(physical_pin), Err(Status::ErrInvalidHandle));
        rt.current = 0;
        rt.begin_handover(source, target, 1, 0, Default::default())
            .unwrap();
        rt.processes[2].running = true;
        rt.current = 1;
        assert_eq!(rt.preserve_pin(physical_pin), Err(Status::ErrAccessDenied));
        rt.preserve_handle(dma).unwrap();
        rt.preserve_handle(domain).unwrap();
        rt.preserve_pin(dma_pin).unwrap();
        if commit {
            rt.handover_bulk(1).unwrap();
            rt.handover_catch_up(2).unwrap();
            rt.quiesce_handover(7, 3).unwrap();
            rt.current = 2;
            rt.ready_handover(7, 4).unwrap();
            rt.commit_handover(5).unwrap();
        } else {
            rt.abort_handover().unwrap();
        }
        rt.unmap_dma(dma_pin).unwrap();
        rt.close(domain).unwrap();
        rt.current = 0;
        rt.unpin(physical_pin).unwrap();
    }
}

#[test]
fn aborted_handover_never_schedules_retired_candidate_threads() {
    use bexos_kernel_core::sched::TaskState;

    for candidate_exits in [false, true] {
        let mut rt = Runtime::new(TestBackend(0x4500_0000));
        let (source, source_space) = rt.create_process("old", "service", 0).unwrap();
        let text = rt.create_vmo(4096, 0).unwrap();
        let stack = rt.create_vmo(8192, 0).unwrap();
        rt.map(Some(source_space), text, 0, 4096, 0x8000_0000, 10)
            .unwrap();
        rt.map(Some(source_space), stack, 0, 8192, 0x8001_0000, 6)
            .unwrap();
        rt.start(source, source_space, 0x8000_0000, 0x8001_2000, 0)
            .unwrap();

        let (target, target_space) = rt.create_process("new", "service", 0).unwrap();
        rt.map(Some(target_space), text, 0, 4096, 0x8000_0000, 10)
            .unwrap();
        rt.map(Some(target_space), stack, 0, 8192, 0x8001_0000, 6)
            .unwrap();
        rt.begin_handover(0, target, 1, 0, Default::default())
            .unwrap();
        rt.start(target, target_space, 0x8000_0000, 0x8001_2000, 0)
            .unwrap();
        rt.handover_bulk(1).unwrap();
        rt.handover_catch_up(2).unwrap();
        rt.quiesce_handover(0, 3).unwrap();
        let mut frame = rt.threads[0].context;
        for _ in 0..8 {
            rt.schedule(&mut frame);
            assert_eq!(rt.current_thread, 1, "quiesced source ran before abort");
        }
        if candidate_exits {
            let mut frame = rt.threads[0].context;
            rt.schedule(&mut frame);
            assert_eq!(rt.current_thread, 1);
            rt.exit_current();
            assert_ne!(rt.scheduler.task(1).unwrap().state, TaskState::Exited);
            rt.schedule_at_on_cpu(0, rt.scheduler.now_ns(), &mut frame);
        } else {
            rt.abort_handover().unwrap();
        }

        assert_eq!(rt.processes[1].root, 0);
        assert_eq!(rt.scheduler.task(2).unwrap().state, TaskState::Exited);
        let mut frame = rt.threads[0].context;
        for _ in 0..4 {
            let address_space = rt.schedule(&mut frame);
            assert_eq!(rt.current_thread, 0);
            assert_ne!(address_space.root_table_phys, 0);
        }
    }
}

#[test]
fn worker_exit_preserves_handover_and_client_endpoints() {
    use bexos_kernel_core::runtime::Object;

    for worker_in_candidate in [false, true] {
        let mut rt = Runtime::new(TestBackend(0x4500_0000));
        let (source, source_space) = rt.create_process("old", "service", 0).unwrap();
        let text = rt.create_vmo(4096, 0).unwrap();
        let stack = rt.create_vmo(8192, 0).unwrap();
        rt.map(Some(source_space), text, 0, 4096, 0x8000_0000, 10)
            .unwrap();
        rt.map(Some(source_space), stack, 0, 8192, 0x8001_0000, 6)
            .unwrap();
        rt.start(source, source_space, 0x8000_0000, 0x8001_2000, 0)
            .unwrap();
        let (server, client) = rt.create_channel();
        let (target, target_space) = rt.create_process("new", "service", 0).unwrap();
        rt.map(Some(target_space), text, 0, 4096, 0x8000_0000, 10)
            .unwrap();
        rt.map(Some(target_space), stack, 0, 8192, 0x8001_0000, 6)
            .unwrap();
        rt.begin_handover(0, target, 1, 0, Default::default())
            .unwrap();
        rt.start(target, target_space, 0x8000_0000, 0x8001_2000, 0)
            .unwrap();
        rt.preserve_handle(server).unwrap();
        rt.handover_bulk(1).unwrap();
        rt.handover_catch_up(2).unwrap();

        rt.current = usize::from(worker_in_candidate);
        let worker = rt
            .create_thread_current(0x8000_0000, 0x8001_1800, 0)
            .unwrap();
        let Object::Thread(worker_id) = rt.capability(worker, 0).unwrap().object else {
            panic!("worker capability is not a thread");
        };
        rt.current_thread = worker_id;
        rt.exit_current_with_code(23);
        assert!(rt.handover.is_some(), "worker exit aborted the transplant");
        assert!(rt.threads[worker_id].exited);
        assert!(rt.processes.iter().all(|process| !process.exited));

        rt.current = 0;
        rt.current_thread = 0;
        rt.write_message(client, b"during preparation", &[])
            .unwrap();
        assert_eq!(
            rt.read_message(server, 32, 0).unwrap().bytes,
            b"during preparation"
        );
        rt.write_message(client, b"across commit", &[]).unwrap();
        rt.quiesce_handover(7, 3).unwrap();
        rt.current = 1;
        rt.current_thread = 1;
        rt.ready_handover(7, 4).unwrap();
        rt.commit_handover(5).unwrap();
        assert!(rt.processes[0].exited);
        assert!(!rt.processes[1].exited);
        // Queued messages and the endpoint move to the surviving candidate.
        assert!(rt.capability(server, 0).is_ok());
        assert_eq!(
            rt.read_message(server, 32, 0).unwrap().bytes,
            b"across commit"
        );
    }
}

#[test]
fn handover_timeout_and_candidate_exit_resume_source_without_peer_closure() {
    let mut rt = Runtime::new(TestBackend(0x4500_0000));
    rt.create_process("old", "service", 0).unwrap();
    rt.processes[0].running = true;
    let (target, _) = rt.create_process("new", "service", 0).unwrap();
    let (server, client) = rt.create_channel();
    rt.begin_handover(0, target, 1, 0, Default::default())
        .unwrap();
    rt.preserve_handle(server).unwrap();
    rt.handover_bulk(1).unwrap();
    rt.handover_catch_up(2).unwrap();
    rt.quiesce_handover(0, 3).unwrap();
    rt.current = 1;
    rt.poll_handover(153);
    assert!(rt.handover.is_none());
    assert!(rt.processes[0].running);
    assert!(rt.processes[1].exited);
    rt.current = 0;
    rt.write_message(client, b"still serving", &[]).unwrap();
    assert_eq!(
        rt.read_message(server, 32, 0).unwrap().bytes,
        b"still serving"
    );
    let (retry, _) = rt.create_process("retry", "service", 0).unwrap();
    rt.begin_handover(0, retry, 1, 200, Default::default())
        .unwrap();
    rt.current = 2;
    rt.exit_current();
    assert!(rt.handover.is_none());
    assert!(rt.processes[0].running);
    assert!(rt.processes[2].exited);
    assert!(!rt.processes[2].quarantined);
}

#[test]
fn package_names_do_not_grant_update_authority_and_stale_resources_block_commit() {
    use bexos_kernel_core::runtime::handover::AUTH_PLATFORM_UPDATE;
    let mut rt = Runtime::new(TestBackend(0x4500_0000));
    rt.create_process("appd", "appd", 0).unwrap();
    rt.processes[0].running = true;
    rt.create_process("debugd", "bexos.driver.debugd", 2)
        .unwrap();
    rt.current = 1;
    assert!(!rt.has_authority(AUTH_PLATFORM_UPDATE));
    rt.current = 0;
    let (target, _) = rt.create_process("new-appd", "appd", 0).unwrap();
    let vmo = rt.create_vmo(4096, 0).unwrap();
    rt.begin_handover(0, target, 1, 0, Default::default())
        .unwrap();
    rt.preserve_handle(vmo).unwrap();
    rt.handover_bulk(1).unwrap();
    rt.close(vmo).unwrap();
    rt.handover_catch_up(2).unwrap();
    rt.quiesce_handover(0, 3).unwrap();
    rt.current = 2;
    rt.ready_handover(0, 4).unwrap();
    assert!(rt.commit_handover(5).is_err());
    rt.abort_handover().unwrap();
    assert!(rt.processes[0].running);
}

#[test]
fn secure_monitor_authority_controls_teed_pinning_and_survives_handover() {
    use bexos_kernel_core::runtime::handover::{AUTH_APP_MANAGER, AUTH_SECURE_MONITOR};

    let mut rt = Runtime::new(MemoryBackend::new(0x4500_0000));
    rt.create_process("appd", "bexos.service.appd", 0).unwrap();
    rt.processes[0].running = true;
    rt.processes[0].authority = AUTH_APP_MANAGER;
    let (teed_handle, _) = rt.create_process("teed", "bexos.service.teed", 0).unwrap();
    let teed_pid = rt.processes.len() - 1;
    rt.processes[teed_pid].running = true;
    rt.current = teed_pid;
    let vmo = rt.create_vmo(4096, 0).unwrap();
    assert_eq!(rt.pin(vmo), Err(Status::ErrAccessDenied));

    rt.current = 0;
    let teed_admin = rt.grant(
        0,
        bexos_kernel_core::runtime::Object::Process(teed_pid),
        bexos_kernel_core::kernel_services::RIGHT_ADMIN,
    );
    rt.set_process_authority(teed_admin, AUTH_SECURE_MONITOR)
        .unwrap();
    rt.current = teed_pid;
    let (address, token) = rt.pin(vmo).unwrap();
    assert_ne!(token, 0);

    rt.current = 0;
    let (candidate_handle, _) = rt
        .create_process("teed-new", "bexos.service.teed", 0)
        .unwrap();
    let candidate_pid = rt.processes.len() - 1;
    rt.begin_handover(teed_handle, candidate_handle, 7, 0, Default::default())
        .unwrap();
    rt.current = teed_pid;
    rt.preserve_handle(vmo).unwrap();
    rt.preserve_pin(token).unwrap();
    rt.handover_bulk(1).unwrap();
    rt.handover_catch_up(2).unwrap();
    rt.current = teed_pid;
    rt.quiesce_handover(0, 3).unwrap();
    rt.current = candidate_pid;
    rt.ready_handover(0, 4).unwrap();
    rt.commit_handover(5).unwrap();
    assert!(rt.has_authority(AUTH_SECURE_MONITOR));
    assert_eq!(rt.validate_secure_pin(token), Ok(()));
    assert_eq!(rt.secure_pin_for_range(address, 4096), Ok(token));
    assert!(rt.backend.revoked_pins.is_empty());
    rt.unpin(token).unwrap();
    assert_eq!(rt.backend.revoked_pins, [token]);
}
#[test]
fn secure_monitor_authority_survives_full_and_incremental_kernel_snapshots() {
    use bexos_kernel_core::runtime::{handover::AUTH_SECURE_MONITOR, incremental};
    let mut source = Runtime::new(TestBackend(0x4500_0000));
    source
        .create_process("teed", "bexos.service.teed", 0)
        .unwrap();
    source.processes[0].authority = AUTH_SECURE_MONITOR;
    let bytes = encode(&source);
    let restored =
        Runtime::read_snapshot(source.backend.clone(), &mut Reader::new(&bytes)).unwrap();
    assert!(restored.has_authority(AUTH_SECURE_MONITOR));

    let mut target = Runtime::new(source.backend.clone());
    let mut cursor = source.begin_live_snapshot(1024).unwrap();
    while let Some(key) = cursor.next() {
        copy_record(&mut source, &mut target, key);
    }
    target.validate_live_snapshot().unwrap();
    assert!(target.has_authority(AUTH_SECURE_MONITOR));
    source.end_live_snapshot();

    source.processes[0].authority |= 0x80;
    let bytes = encode(&source);
    assert!(Runtime::read_snapshot(source.backend.clone(), &mut Reader::new(&bytes)).is_err());
    let mut bytes = vec![0; 32768];
    let mut writer = Writer::new(&mut bytes);
    source
        .write_record(incremental::key(incremental::PROCESS, 0), &mut writer)
        .unwrap();
    let len = writer.len();
    assert!(target.adopt_record(&bytes[..len]).is_err());
}

#[test]
fn small_channel_read_preserves_large_message_and_transferred_handles() {
    let mut rt = Runtime::new(TestBackend(0x4500_0000));
    rt.create_process("ipc", "ipc", 0).unwrap();
    let (sender, receiver) = rt.create_channel();
    let vmo = rt.create_vmo(4096, 0).unwrap();
    let bytes = vec![0xa5; 32768];
    rt.write_message(sender, &bytes, &[vmo]).unwrap();
    assert_eq!(
        rt.read_message(receiver, 512, 32).err(),
        Some(Status::ErrBufferTooSmall)
    );
    assert!(rt.capability(vmo, 0).is_err());
    let received = rt.read_message(receiver, 65408, 32).unwrap();
    assert_eq!(received.bytes, bytes);
    assert_eq!(received.handles, [vmo]);
    assert!(rt.capability(vmo, 0).is_ok());
    assert_eq!(
        rt.read_message(receiver, 512, 32).err(),
        Some(Status::ErrTimedOut)
    );
}

#[test]
fn kernel_snapshots_preserve_dma_and_live_scheduler_state() {
    let mut source = Runtime::new(TestBackend(0x4500_0000));
    let (process, space) = source
        .create_process("nvme", "bexos.driver.storage.nvme", 2)
        .unwrap();
    let text = source.create_vmo(4096, 0).unwrap();
    source
        .map(Some(space), text, 0, 4096, 0x8000_0000, 10)
        .unwrap();
    let stack = source.create_vmo(8192, 0).unwrap();
    source
        .map(Some(space), stack, 0, 8192, 0x8001_0000, 6)
        .unwrap();
    source
        .start(process, space, 0x8000_0000, 0x8001_2000, 0)
        .unwrap();
    source
        .create_thread_current(0x8000_0000, 0x8001_1800, 0)
        .unwrap();
    source
        .scheduler
        .set_fair_profile(
            2,
            FairProfile {
                priority: 7,
                weight: 3,
            },
        )
        .unwrap();
    let mut frame = source.threads[0].context;
    source.schedule(&mut frame);
    source.futex_wait_current(0x8001_0000, 0).unwrap();
    source.schedule(&mut frame);

    let dma = source.create_vmo(4096, 2).unwrap();
    let domain = source.create_iommu_domain(0x10, 48).unwrap();
    let (_, token) = source.map_dma(domain, dma, 0, 4096, 6).unwrap();
    let bytes = encode(&source);
    let mut restored =
        Runtime::read_snapshot(source.backend.clone(), &mut Reader::new(&bytes)).unwrap();
    assert_eq!(restored.iommu_domains, source.iommu_domains);
    assert_eq!(restored.dma_mappings, source.dma_mappings);
    assert_eq!(restored.scheduler, source.scheduler);
    restored.unmap_dma(token).unwrap();

    let mut target = Runtime::new(source.backend.clone());
    let mut cursor = source.begin_live_snapshot(1024).unwrap();
    while let Some(key) = cursor.next() {
        copy_record(&mut source, &mut target, key);
    }
    target.validate_live_snapshot().unwrap();
    assert_eq!(target.scheduler, source.scheduler);
    assert_eq!(target.dma_mappings, source.dma_mappings);
    assert_eq!(target.iommu_domains, source.iommu_domains);
    source.unmap_dma(token).unwrap();
    source.close(domain).unwrap();
    assert_eq!(source.futex_wake(0x8001_0000, 1).unwrap(), 1);
    // A thread created after the bulk cursor was captured must arrive as a
    // delta, along with its scheduler entry and capability.
    source
        .create_thread_current(0x8000_0000, 0x8001_1000, 0)
        .unwrap();
    while let Some(key) = source.dirty_next().unwrap() {
        copy_record(&mut source, &mut target, key);
    }
    target.validate_live_snapshot().unwrap();
    assert_eq!(target.scheduler, source.scheduler);
    assert_eq!(target.dma_mappings, source.dma_mappings);
    assert_eq!(target.iommu_domains, source.iommu_domains);
    assert_eq!(target.unmap_dma(token), Err(Status::ErrInvalidHandle));
    source.end_live_snapshot();
    assert_eq!(encode(&source), encode(&target));
    for _ in 0..5 {
        assert_eq!(source.scheduler.tick(), target.scheduler.tick());
    }
    let mut cursor = source.begin_live_snapshot(1024).unwrap();
    while let Some(key) = cursor.next() {
        copy_record(&mut source, &mut target, key);
    }
    source.terminate_process(process, 7).unwrap();
    while let Some(key) = source.dirty_next().unwrap() {
        copy_record(&mut source, &mut target, key);
    }
    target.validate_live_snapshot().unwrap();
    source.end_live_snapshot();
    assert_eq!(encode(&source), encode(&target));
}

#[test]
fn runtime_roundtrip_preserves_objects_rights_queues_pins_and_contexts() {
    let mut rt = Runtime::new(TestBackend(0x4500_0000));
    let (_, space) = rt
        .create_process("debugd", "bexos.driver.debugd", 2)
        .unwrap();
    rt.create_process("app", "com.example.app", 0).unwrap();
    set_saved_context_word(&mut rt.processes[0].context, 19, 0x123456);
    set_saved_context_word(&mut rt.processes[0].context, 34 + 33, 0xabcdef);
    rt.processes[0].context.stack_pointer = 0xff00_0000;
    rt.processes[0].running = true;
    rt.bootfs_pages = 8;
    rt.reclaimed_pages = 42;
    let vmo = rt.create_vmo(4096, 2).unwrap();
    rt.map(Some(space), vmo, 0, 4096, 0xb000_0000, 6).unwrap();
    let (_, pin) = rt.pin(vmo).unwrap();
    let vacant = rt.duplicate(vmo, 2).unwrap();
    rt.close(vacant).unwrap();
    let transfer = rt.duplicate(vmo, 2 | 1).unwrap();
    let (a, b) = rt.create_channel();
    rt.write_message(a, b"queued before takeover", &[transfer])
        .unwrap();
    let bytes = encode(&rt);
    let mut reader = Reader::new(&bytes);
    let mut restored = Runtime::read_snapshot(rt.backend.clone(), &mut reader).unwrap();
    assert!(reader.finished());
    assert_eq!(bytes, encode(&restored));
    assert_eq!(
        saved_context_word(&restored.processes[0].context, 19),
        0x123456
    );
    assert_eq!(
        saved_context_word(&restored.processes[0].context, 34 + 33),
        0xabcdef
    );
    assert!(restored.capability(vacant, 0).is_err());
    let message = restored.read_message(b, 64, 1).unwrap();
    assert_eq!(message.bytes, b"queued before takeover");
    assert_eq!(message.handles, [transfer]);
    assert_eq!(restored.capability(transfer, 2).unwrap().rights, 3);
    assert!(restored.capability(transfer, 4).is_err());
    restored.unpin(pin).unwrap();
    restored.write_message(a, b"after takeover", &[]).unwrap();
    assert_eq!(
        restored.read_message(b, 64, 0).unwrap().bytes,
        b"after takeover"
    );
}
#[test]
fn snapshot_rejects_truncation_bad_version_and_invalid_object_references() {
    let mut rt = Runtime::new(TestBackend(0x4500_0000));
    rt.create_process("test", "test", 0).unwrap();
    let bytes = encode(&rt);
    for len in 0..bytes.len() {
        assert!(
            Runtime::read_snapshot(rt.backend.clone(), &mut Reader::new(&bytes[..len])).is_err()
        );
    }
    let mut bad = bytes.clone();
    bad[0] ^= 1;
    assert!(matches!(
        Runtime::read_snapshot(rt.backend.clone(), &mut Reader::new(&bad)),
        Err(TransplantError::BadMagic)
    ));
    bad = bytes.clone();
    bad[8] = 255;
    assert!(matches!(
        Runtime::read_snapshot(rt.backend.clone(), &mut Reader::new(&bad)),
        Err(TransplantError::UnsupportedVersion)
    ));
    rt.handles[0].as_mut().unwrap().owner = 900;
    assert!(Runtime::read_snapshot(rt.backend.clone(), &mut Reader::new(&encode(&rt))).is_err());
}
fn handoff() -> KernelTransplantHandoff {
    let mut h = KernelTransplantHandoff {
        magic: HANDOFF_MAGIC,
        version: HANDOFF_VERSION,
        architecture: bexos_kernel_core::runtime::Context::ARCHITECTURE,
        architecture_state: [0; 16],
        generation: 21,
        old_kernel: KernelRange::new(0x4020_0000, 0xa0_0000),
        old_reclaim: KernelRange::new(0x4020_0000, 0xa0_0000),
        replacement: KernelRange::new(bexos_boot::UPDATE_BASE, 0x20_0000),
        handoff: KernelRange::new(0x4010_1000, 4096),
        snapshot: KernelRange::new(bexos_boot::UPDATE_SNAPSHOT, 3),
        replacement_entry: bexos_boot::UPDATE_BASE + 4096,
        cpu: Aarch64CpuContextRecord {
            cpu_id: 0,
            program_counter: 0x4020_1000,
            stack_pointer: 0x4030_0000,
            pstate: 0,
            ttbr0_el1: 0x4500_0000,
            ttbr1_el1: 0,
            vbar_el1: 0x4020_0000,
        },
        system: Aarch64SystemRegisters::default(),
        switch_authorized: 1,
        artifact_hash: [1; 32],
        snapshot_checksum: checksum(b"abc") as u64,
        checksum: 0,
    };
    h.seal();
    h
}
#[test]
fn handoff_checks_identity_checksum_ranges_and_snapshot_integrity() {
    let h = handoff();
    h.validate_snapshot(b"abc").unwrap();
    let mut bad = h;
    bad.magic ^= 1;
    assert_eq!(bad.validate(), Err(TransplantError::BadMagic));
    bad = h;
    bad.version += 1;
    assert_eq!(bad.validate(), Err(TransplantError::UnsupportedVersion));
    bad = h;
    bad.generation += 1;
    assert_eq!(bad.validate(), Err(TransplantError::ChecksumMismatch));
    assert_eq!(
        h.validate_snapshot(b"abd"),
        Err(TransplantError::ChecksumMismatch)
    );
    for range in [
        KernelRange::new(0x4020_0000, 4096),
        KernelRange::new(0x4400_0000, 4096),
        KernelRange::new(0x4010_1000, 4096),
        KernelRange::new(0x4300_0000, 4096),
        KernelRange::new(u64::MAX - 4095, 4096),
    ] {
        bad = h;
        bad.replacement = range;
        bad.seal();
        assert!(bad.validate().is_err());
    }
    bad = h;
    bad.old_reclaim.len += 4096;
    bad.seal();
    assert!(bad.validate().is_err());
    bad = h;
    bad.snapshot.len = u64::MAX;
    bad.seal();
    assert!(bad.validate().is_err());
}

#[test]
fn platform_request_requires_a_transferred_artifact_handle() {
    use kernel_fidl::{
        FidlDecode, FidlEncode, HandleRef, KernelDebugControlApplyPlatformUpdateRequest as Request,
        KernelUpdateKind,
    };
    let request = Request {
        kind: KernelUpdateKind::Microkernel,
        generation: 1,
        target: "qemu-aarch64-kernel",
        artifact_hash: &[1; 32],
        artifact_len: 4096,
        artifact: HandleRef { raw: 7 },
    };
    let mut bytes = [0; 1024];
    let mut handles = [HandleRef { raw: 0 }; 1];
    let encoded = request.encode(&mut bytes, &mut handles).unwrap();
    assert_eq!(encoded.handles, 1);
    assert!(Request::decode(&bytes[..encoded.bytes], &[]).is_err());
    let decoded = Request::decode(&bytes[..encoded.bytes], &handles).unwrap();
    assert_eq!(decoded.artifact.raw, 7);
}

#[test]
fn random_requires_entropy_and_preserves_stream_position() {
    let mut source = Runtime::new(TestBackend(0x4500_0000));
    source.create_process("test", "test", 0).unwrap();
    assert!(source.random_bytes(32).is_err());
    source.set_boot_entropy_seed(Some([1, 2, 3, 4]));
    let first = source.random_bytes(32).unwrap();
    assert!(source.random_bytes(32769).is_err());
    let bytes = encode(&source);
    let mut target =
        Runtime::read_snapshot(TestBackend(0x4600_0000), &mut Reader::new(&bytes)).unwrap();
    let expected = source.random_bytes(32).unwrap();
    assert_ne!(first, expected);
    assert_eq!(target.random_bytes(32).unwrap(), expected);
}

#[test]
fn automatic_child_vmars_avoid_heap_mappings_and_siblings() {
    let mut rt = Runtime::new(TestBackend(0x4500_0000));
    rt.create_process("wasm", "wasm", 0).unwrap();
    let heap = rt.heap_vmar_handle().unwrap();
    let vmo = rt.create_vmo(USER_HEAP_CHUNK_SIZE, 0).unwrap();
    let heap_base = rt
        .map_vmo_in_vmar(heap, vmo, 0, 0, USER_HEAP_CHUNK_SIZE, 3)
        .unwrap();
    let flags = 3 | 8 | 0x20;
    let (a, a_base) = rt.create_sub_vmar(heap, 0, 16384, flags).unwrap();
    let (b, b_base) = rt.create_sub_vmar(heap, 0, 16384, flags).unwrap();
    assert!(a_base >= heap_base + USER_HEAP_CHUNK_SIZE);
    assert!(b_base >= a_base + 16384);
    assert_eq!(
        rt.create_sub_vmar(heap, 4096, 4096, flags),
        Err(Status::ErrInvalidArgs)
    );
    rt.destroy_vmar(a).unwrap();
    let (_, reused) = rt.create_sub_vmar(heap, 0, 16384, flags).unwrap();
    assert_eq!(reused, a_base);
    rt.destroy_vmar(b).unwrap();
}

#[test]
fn time_page_cache_does_not_reference_a_released_vmo() {
    let mut rt = Runtime::new(TestBackend(0x4500_0000));
    rt.create_process("command", "command", 0).unwrap();
    let allocation_start = rt.backend.0;
    let first = rt.get_vdso_time_page(0, 1_000_000).unwrap();
    rt.close(first).unwrap();
    let second = rt.get_vdso_time_page(1, 1_000_000).unwrap();
    let address = rt.map(None, second, 0, 4096, 0, 2).unwrap();
    rt.close(second).unwrap();
    rt.unmap(None, address, 4096).unwrap();
    let third = rt.get_vdso_time_page(2, 1_000_000).unwrap();
    // Capability and VMO slots may be recycled; backing must be fresh after
    // each last reference disappears.
    assert_eq!(rt.backend.0, allocation_start + 3 * 4096);
    rt.close(third).unwrap();
}

#[test]
fn incremental_records_reject_foreign_architecture_before_mutation() {
    use bexos_kernel_core::runtime::{Context, incremental};
    let source = Runtime::new(TestBackend(0x4500_0000));
    let mut target = Runtime::new(TestBackend(0x4600_0000));
    let mut bytes = vec![0; 32768];
    let mut writer = Writer::new(&mut bytes);
    source
        .write_record(incremental::key(incremental::META, 0), &mut writer)
        .unwrap();
    let len = writer.len();
    bytes[16..24].copy_from_slice(
        &(if Context::ARCHITECTURE == 1 {
            2u64
        } else {
            1u64
        })
        .to_le_bytes(),
    );
    target.current = 77;
    assert!(target.adopt_record(&bytes[..len]).is_err());
    assert_eq!(target.current, 77);
    bytes[16..24].copy_from_slice(&Context::ARCHITECTURE.to_le_bytes());
    bytes[8..16].copy_from_slice(&99u64.to_le_bytes());
    assert!(target.adopt_record(&bytes[..len]).is_err());
    assert_eq!(target.current, 77);
}

fn saved_context_bytes(context: &bexos_kernel_core::runtime::Context) -> [u8; 816] {
    let mut bytes = [0; 816];
    bexos_kernel_core::runtime::snapshot::write_context(&mut Writer::new(&mut bytes), context)
        .unwrap();
    bytes
}
fn saved_context_word(context: &bexos_kernel_core::runtime::Context, index: usize) -> u64 {
    u64::from_le_bytes(
        saved_context_bytes(context)[index * 8..index * 8 + 8]
            .try_into()
            .unwrap(),
    )
}
fn set_saved_context_word(
    context: &mut bexos_kernel_core::runtime::Context,
    index: usize,
    value: u64,
) {
    let mut bytes = saved_context_bytes(context);
    bytes[index * 8..index * 8 + 8].copy_from_slice(&value.to_le_bytes());
    *context =
        bexos_kernel_core::runtime::snapshot::read_context(&mut Reader::new(&bytes)).unwrap();
}

#[test]
fn legacy_entropy_snapshot_remains_arm_only() {
    use bexos_kernel_core::runtime::Context;
    let mut source = Runtime::new(TestBackend(0x4500_0000));
    source.create_process("legacy", "legacy", 0).unwrap();
    source.set_boot_entropy_seed(Some([1, 2, 3, 4]));
    source.random_bytes(32).unwrap();
    // Remove the header and sole process context architecture tags to recreate
    // the legacy v15 layout, including its entropy stream position.
    let mut bytes = encode(&source);
    let scheduler_start = bytes
        .windows(8)
        .rposition(|part| part == b"BEXSCH01" || part == b"BEXSCH02")
        .unwrap();
    // v20 appends the interrupt table immediately before the scheduler.  The
    // empty-table count is not present in the v15 fixture reconstructed here.
    bytes.drain(scheduler_start - 8..scheduler_start);
    let context = saved_context_bytes(&source.processes[0].context);
    let start = bytes
        .windows(context.len())
        .position(|part| part == context)
        .unwrap();
    bytes.drain(start + 808..start + 816);
    bytes[8..16].copy_from_slice(&15u64.to_le_bytes());
    bytes.drain(16..24);
    let restored = Runtime::read_snapshot(TestBackend(0x4600_0000), &mut Reader::new(&bytes));
    if Context::ARCHITECTURE == 1 {
        assert_eq!(
            restored.unwrap().random_bytes(32).unwrap(),
            source.random_bytes(32).unwrap()
        );
    } else {
        assert!(restored.is_err());
    }
}

#[test]
fn device_read_dma_accepts_read_only_vmo_and_cannot_escalate_to_device_write() {
    let mut rt = Runtime::new(MemoryBackend::new(0x4500_0000));
    rt.create_process("gpu", "bexos.driver.display.virtio", 2)
        .unwrap();
    let vmo = rt.create_vmo(4096, 2).unwrap();
    let read_only = rt.duplicate(vmo, 2).unwrap();
    let domain = rt.create_iommu_domain(0x10, 48).unwrap();
    assert_eq!(
        rt.map_dma(domain, read_only, 0, 4096, 4),
        Err(Status::ErrAccessDenied)
    );
    assert_eq!(
        rt.map_dma(domain, read_only, 0, 4096, 6),
        Err(Status::ErrAccessDenied)
    );
    let (_, token) = rt.map_dma(domain, read_only, 0, 4096, 2).unwrap();
    rt.unmap_dma(token).unwrap();
    assert_eq!(
        rt.map_dma(domain, read_only, 0, 4096, 0),
        Err(Status::ErrInvalidArgs)
    );
}

#[test]
fn execution_counters_survive_kernel_snapshot_with_exited_threads() {
    use bexos_kernel_core::sched::SchedulerTask;
    let mut rt = Runtime::new(TestBackend(0x4500_0000));
    rt.create_process("appd", "appd", 0).unwrap();
    rt.scheduler.account_runtime(10);
    rt.scheduler
        .add_task(SchedulerTask::new(1, 1, 1, 1, "main").unwrap())
        .unwrap();
    rt.scheduler.account_runtime(20);
    rt.scheduler.exit_task(1, 0).unwrap();
    rt.scheduler.cleanup_exited();
    let bytes = encode(&rt);
    let restored = Runtime::read_snapshot(rt.backend.clone(), &mut Reader::new(&bytes)).unwrap();
    assert_eq!(restored.scheduler.runtime_stats(1), Some((10, 10)));
    assert_eq!(encode(&restored), bytes);
}

#[test]
fn running_execution_counter_continues_after_kernel_snapshot() {
    use bexos_kernel_core::sched::SchedulerTask;
    let mut rt = Runtime::new(TestBackend(0x4500_0000));
    rt.create_process("appd", "appd", 0).unwrap();
    rt.scheduler.account_runtime(10);
    rt.scheduler
        .add_task(SchedulerTask::new(1, 1, 1, 1, "main").unwrap())
        .unwrap();
    rt.scheduler.account_runtime(20);
    let bytes = encode(&rt);
    let mut restored =
        Runtime::read_snapshot(rt.backend.clone(), &mut Reader::new(&bytes)).unwrap();
    assert_eq!(restored.scheduler.runtime_stats(1), Some((10, 10)));
    restored.scheduler.account_runtime(35);
    assert_eq!(restored.scheduler.runtime_stats(1), Some((25, 25)));
    // Reading the same boundary twice must not charge the restored interval twice.
    restored.scheduler.account_runtime(35);
    assert_eq!(restored.scheduler.runtime_stats(1), Some((25, 25)));
}

#[test]
fn legacy_scheduler_snapshot_reencodes_without_diagnostic_fields_until_execution() {
    let mut rt = Runtime::new(TestBackend(0x4500_0000));
    rt.create_process("appd", "appd", 0).unwrap();
    let mut bytes = encode(&rt);
    let offset = bytes.windows(8).position(|b| b == b"BEXSCH02").unwrap();
    bytes[offset..offset + 8].copy_from_slice(b"BEXSCH01");
    // An empty new accounting suffix: absent clock and zero retained threads.
    bytes.truncate(bytes.len() - 16);
    let mut restored =
        Runtime::read_snapshot(rt.backend.clone(), &mut Reader::new(&bytes)).unwrap();
    assert_eq!(encode(&restored), bytes);
    restored.scheduler.account_runtime(100);
    assert!(encode(&restored).windows(8).any(|b| b == b"BEXSCH02"));
}

#[test]
fn graphical_resource_churn_reclaims_slots_without_reusing_endpoint_identity() {
    let mut source = Runtime::new(TestBackend(0x4500_0000));
    source.create_process("appd", "appd", 0).unwrap();
    let (live, live_peer) = source.create_channel();
    let live_identity = source.channel_identity(live).unwrap();
    let live_handles = source.handles.len();
    let mut previous = 0;
    for _ in 0..10_000 {
        let (a, b) = source.create_channel();
        let identity = source.channel_identity(a).unwrap();
        assert!(identity.0 > previous);
        previous = identity.0;
        let vmo = source.create_vmo(4096, 0).unwrap();
        let copy = source.duplicate(vmo, 1 | 16).unwrap();
        source.write_message(a, b"frame", &[copy]).unwrap();
        source.close(vmo).unwrap();
        let frame = source.read_message(b, 64, 1).unwrap();
        source.close(frame.handles[0]).unwrap();
        source.close(a).unwrap();
        source.close(b).unwrap();
    }
    assert!(source.handles.len() <= live_handles + 4);
    assert_eq!(source.channels.len(), 2);
    assert_eq!(source.vmos.len(), 1);
    assert_eq!(source.channel_identity(live).unwrap(), live_identity);
    source.write_message(live, b"still live", &[]).unwrap();
    let snapshot = encode(&source);
    let mut restored =
        Runtime::read_snapshot(TestBackend(0x4800_0000), &mut Reader::new(&snapshot)).unwrap();
    assert_eq!(restored.channel_identity(live).unwrap(), live_identity);
    assert_eq!(
        restored.read_message(live_peer, 64, 0).unwrap().bytes,
        b"still live"
    );
    let (next, _) = restored.create_channel();
    assert!(restored.channel_identity(next).unwrap().0 > previous);

    let mut target = Runtime::new(source.backend.clone());
    let mut cursor = source.begin_live_snapshot(1024).unwrap();
    while let Some(key) = cursor.next() {
        copy_record(&mut source, &mut target, key);
    }
    let (a, b) = source.create_channel();
    let vmo = source.create_vmo(4096, 0).unwrap();
    source.close(vmo).unwrap();
    source.close(a).unwrap();
    source.close(b).unwrap();
    let (new, _) = source.create_channel();
    while let Some(key) = source.dirty_next().unwrap() {
        copy_record(&mut source, &mut target, key);
    }
    target.validate_live_snapshot().unwrap();
    assert_eq!(
        target.channel_identity(new).unwrap(),
        source.channel_identity(new).unwrap()
    );
    assert_eq!(encode(&target), encode(&source));
}

#[test]
fn older_incremental_channel_record_preserves_its_endpoint_identity() {
    use bexos_kernel_core::runtime::incremental;
    let mut source = Runtime::new(TestBackend(0x4500_0000));
    source.create_process("appd", "appd", 0).unwrap();
    source.create_channel();
    let mut bytes = vec![0; 4096];
    let mut writer = Writer::new(&mut bytes);
    source
        .write_record(incremental::key(incremental::CHANNEL, 0), &mut writer)
        .unwrap();
    let len = writer.len();
    let mut target = Runtime::new(TestBackend(0x4800_0000));
    target.create_process("appd", "appd", 0).unwrap();
    target.adopt_record(&bytes[..len - 8]).unwrap();
    let cap = target.grant(0, bexos_kernel_core::runtime::Object::Channel(0, 0), 1);
    assert_eq!(target.channel_identity(cap).unwrap(), (1, 2));
}

fn restricted_state_bytes(pc: u64, sp: u64) -> Vec<u8> {
    if bexos_kernel_core::runtime::Context::ARCHITECTURE == 2 {
        let mut state = bexos_restricted_abi::X86_64StateV1::zeroed();
        state.rip = pc;
        state.rsp = sp;
        state.rflags = u64::MAX;
        state.fs_base = 0x1234_0000;
        unsafe {
            core::slice::from_raw_parts(
                (&state as *const bexos_restricted_abi::X86_64StateV1).cast(),
                core::mem::size_of_val(&state),
            )
            .to_vec()
        }
    } else {
        let mut state = bexos_restricted_abi::Aarch64StateV1::zeroed();
        state.pc = pc;
        state.sp = sp;
        state.pstate = u64::MAX;
        state.tpidr_el0 = 0x1234_0000;
        state.tpidrro_el0 = 0x5678_0000;
        unsafe {
            core::slice::from_raw_parts(
                (&state as *const bexos_restricted_abi::Aarch64StateV1).cast(),
                core::mem::size_of_val(&state),
            )
            .to_vec()
        }
    }
}

#[test]
fn restricted_binding_reflects_exits_kicks_and_survives_snapshots() {
    use bexos_kernel_core::{kernel_services::RIGHT_ADMIN, runtime::Object};
    use bexos_restricted_abi::{Header, Reason, STATE_VMO_SIZE};

    let mut rt = Runtime::new(MemoryBackend::new(0x5100_0000));
    let (process, space) = rt.create_process("restricted", "restricted", 0).unwrap();
    let text = rt.create_vmo(4096, 0).unwrap();
    rt.map(Some(space), text, 0, 4096, 0x8000_0000, 10).unwrap();
    let stack = rt.create_vmo(8192, 0).unwrap();
    rt.map(Some(space), stack, 0, 8192, 0x8001_0000, 6).unwrap();
    rt.start(process, space, 0x8000_0100, 0x8001_2000, 0)
        .unwrap();

    let state = rt.create_vmo(STATE_VMO_SIZE, 0).unwrap();
    let state_va = rt
        .map(Some(space), state, 0, STATE_VMO_SIZE, 0x8002_0000, 6)
        .unwrap();
    rt.copy_to_user(state_va, &restricted_state_bytes(0x8000_0200, 0x8001_1800))
        .unwrap();

    let read_only = rt.duplicate(state, 2).unwrap();
    assert_eq!(
        rt.restricted_bind_state(0, read_only),
        Err(Status::ErrAccessDenied)
    );
    assert_eq!(
        rt.restricted_bind_state(1, state),
        Err(Status::ErrInvalidArgs)
    );
    rt.restricted_bind_state(0, state).unwrap();
    assert_eq!(
        rt.restricted_bind_state(0, state),
        Err(Status::ErrAlreadyExists)
    );

    let host = rt.threads[0].context;
    let mut guest = rt
        .restricted_enter(0, 0x8000_0100, 0xfeed, host, 0x7777_0000)
        .unwrap();
    assert_eq!(guest.instruction_pointer, 0x8000_0200);
    assert_eq!(guest.stack_pointer, 0x8001_1800);
    assert_eq!(guest.thread_pointer(), 0x1234_0000);
    assert_eq!(rt.restricted_unbind_state(0), Err(Status::ErrAlreadyExists));
    guest.instruction_pointer += if bexos_kernel_core::runtime::Context::ARCHITECTURE == 2 {
        2
    } else {
        4
    };
    let callback = rt
        .restricted_exit_current(Reason::Syscall, 0x55, 0, guest)
        .unwrap();
    assert_eq!(callback.instruction_pointer, 0x8000_0100);

    let mut reflected = vec![0; restricted_state_bytes(0, 0).len()];
    rt.copy_from_user(state_va, &mut reflected).unwrap();
    let header = unsafe { core::ptr::read_unaligned(reflected.as_ptr().cast::<Header>()) };
    assert_eq!(header.reason, Reason::Syscall as u32);
    assert_eq!(header.exception_code, 0x55);

    let thread = rt.grant(0, Object::Thread(0), RIGHT_ADMIN);
    assert_eq!(rt.restricted_kick(0, thread), Ok(1));
    assert_eq!(rt.restricted_kick(0, thread), Ok(1));
    let callback = rt
        .restricted_enter(0, 0x8000_0100, 0xbeef, callback, 0x7777_0000)
        .unwrap();
    assert_eq!(callback.instruction_pointer, 0x8000_0100);
    rt.copy_from_user(state_va, &mut reflected).unwrap();
    let header = unsafe { core::ptr::read_unaligned(reflected.as_ptr().cast::<Header>()) };
    assert_eq!(header.reason, Reason::Kick as u32);

    let snapshot = encode(&rt);
    let restored = Runtime::read_snapshot(rt.backend.clone(), &mut Reader::new(&snapshot)).unwrap();
    assert!(restored.threads[0].restricted.is_some());

    let mut target = Runtime::new(rt.backend.clone());
    let mut cursor = rt.begin_live_snapshot(1024).unwrap();
    while let Some(key) = cursor.next() {
        copy_record(&mut rt, &mut target, key);
    }
    target.validate_live_snapshot().unwrap();
    assert!(target.threads[0].restricted.is_some());

    target.threads[0].restricted.as_mut().unwrap().state_vmo = usize::MAX;
    assert!(target.validate_live_snapshot().is_err());
    rt.restricted_unbind_state(0).unwrap();
    assert!(rt.threads[0].restricted.is_none());
}
