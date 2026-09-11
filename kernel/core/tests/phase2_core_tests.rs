mod allocation_failure;
mod quiescence_tests;
mod runtime_snapshot_tests;
use bexos_kernel_core::bootfs::{Bootfs, ENTRY_SIZE, HEADER_SIZE, MAGIC, VERSION};
use bexos_kernel_core::cpu_features::{Aarch64CpuFeatures, AsidSupport, KernelFeatureState};
use bexos_kernel_core::ipc::{Capability, Channel, Endpoint, IpcError, Message};
use bexos_kernel_core::kernel_services::memory::VMO_FLAG_CONTIGUOUS_PHYS;
use bexos_kernel_core::kernel_services::task::ThreadState;
use bexos_kernel_core::kernel_services::vmar::{
    USER_VMAR_BASE, VMAR_FLAG_CAN_MAP_EXECUTE, VMAR_FLAG_CAN_MAP_READ, VMAR_FLAG_CAN_MAP_SPECIFIC,
    VMAR_FLAG_CAN_MAP_WRITE,
};
use bexos_kernel_core::kernel_services::{
    ControlPlane, HardwareAccess, KernelServiceStatus, RIGHT_DUPLICATE, RIGHT_EXECUTE, RIGHT_MAP,
    RIGHT_READ, RIGHT_TRANSFER, RIGHT_WRITE, SIGNAL_READABLE, SIGNAL_TERMINATED, WaitManyItem,
};
use bexos_kernel_core::loader::{
    ElfLoadError, EmbeddedPackageEntry, EmbeddedPackageProvider, LoadPlan, PackageImageMetadata,
    PackageLoadError, PackageProvider, RIGHTS_EXECUTE as LOADER_RIGHT_EXECUTE,
    RIGHTS_READ as LOADER_RIGHT_READ,
};
use bexos_kernel_core::memory::{PAGE_SIZE, PageFrameAllocator};
use bexos_kernel_core::mmu::{
    Access, MemoryAttr, block_descriptor_1g, page_descriptor, table_descriptor,
    user_code_page_descriptor, user_data_page_descriptor,
};
use bexos_kernel_core::nvme;
use bexos_kernel_core::pci::{
    BAR_PREFETCHABLE, BAR_TYPE_64, PciAddress, PciClass, align_resource_base,
    command_with_memory_and_bus_master, decode_bar_size,
};
use bexos_kernel_core::psci::{
    PsciError, PsciFunction, PsciVersion, SystemPowerOperation, decode_feature_status,
    decode_status, function_for_power_operation,
};
use bexos_kernel_core::sched::{
    BlockReason, DEFAULT_FAIR_QUANTUM_NS, DeadlineProfile, FairProfile, RoundRobinScheduler,
    Scheduler, SchedulerError, SchedulerTask, SchedulingProfile, Task, TaskState,
};
use bexos_kernel_core::transplant::{
    Aarch64CpuContextRecord, CapabilityHandleRecord, FrozenTaskRecord, PreservedRegion,
    SnapshotHeader, SnapshotPhase, TransplantError,
};
use kernel_fidl::{
    ChannelControlCreateChannelRequest, ChannelControlPublicServer, ChannelControlSetPolicyRequest,
    ClockGetTimeRequest, ClockPublicServer, ClockType, CpuMask, FidlDecode, FidlWireError,
    KernelDebugControlListProcessesRequest, KernelDebugControlPublicServer,
    ProfileProviderCreateProfileRequest, ProfileProviderPublicServer, SchedulingProfileInfo,
    Status, SystemPrivilegedBexosSystemPrivilegedServer, TaskControlPublicServer,
    TaskControlSetCpuAffinityRequest, TaskControlSetProfileRequest, TaskControlYieldThreadRequest,
    VirtualMemoryCreateSubVmarRequest, VirtualMemoryCreateVmoRequest,
    VirtualMemoryDestroyVmarRequest, VirtualMemoryMapVmoRequest, VirtualMemoryPublicServer,
    VmarFlags, VmoFlags,
};

#[test]
fn frame_allocator_aligns_and_counts_frames() {
    let mut allocator = PageFrameAllocator::new(0x1003, 0x5001);

    assert_eq!(allocator.remaining_frames(), 3);
    let first = allocator.allocate().unwrap();
    assert_eq!(first.start, 0x2000);
    assert_eq!(allocator.allocate().unwrap().start, 0x2000 + PAGE_SIZE);
    assert_eq!(allocator.remaining_frames(), 1);
    assert!(allocator.free(first));
    assert_eq!(allocator.recycled_len(), 1);
    assert_eq!(allocator.allocate().unwrap(), first);
}

#[test]
fn shared_loader_accepts_valid_aarch64_elf() {
    let plan = LoadPlan::parse_elf64_aarch64(&valid_elf()).expect("valid elf should parse");

    assert_eq!(plan.entry_vaddr, 0x401000);
    assert_eq!(plan.segments.len(), 1);
    assert_eq!(plan.segments[0].file_offset, 0);
    assert_eq!(plan.segments[0].file_size, 0x1800);
    assert_eq!(plan.segments[0].vaddr, 0x400000);
    assert_eq!(
        plan.segments[0].rights,
        LOADER_RIGHT_READ | LOADER_RIGHT_EXECUTE
    );
    assert_eq!(plan.tls, None);
}

#[test]
fn shared_loader_accepts_pt_tls_template() {
    let plan =
        LoadPlan::parse_elf64_aarch64(&valid_elf_with_tls()).expect("valid TLS elf should parse");

    let tls = plan.tls.expect("TLS segment");
    assert_eq!(tls.file_offset, 0x2100);
    assert_eq!(tls.file_size, 4);
    assert_eq!(tls.mem_size, 16);
    assert_eq!(tls.align, 16);
}

#[test]
fn shared_loader_rejects_malformed_or_unsafe_elf() {
    let mut bad_magic = valid_elf();
    bad_magic[0] = 0;
    assert_eq!(
        LoadPlan::parse_elf64_aarch64(&bad_magic),
        Err(ElfLoadError::BadMagic)
    );

    let mut bad_machine = valid_elf();
    put_u16(&mut bad_machine, 18, 62);
    assert_eq!(
        LoadPlan::parse_elf64_aarch64(&bad_machine),
        Err(ElfLoadError::UnsupportedMachine)
    );

    let mut wx = valid_elf();
    put_u32(&mut wx, 68, 0x7);
    assert_eq!(
        LoadPlan::parse_elf64_aarch64(&wx),
        Err(ElfLoadError::WriteExecuteSegment)
    );

    let mut bad_entry = valid_elf();
    put_u64(&mut bad_entry, 24, 0x500000);
    assert_eq!(
        LoadPlan::parse_elf64_aarch64(&bad_entry),
        Err(ElfLoadError::EntryOutsideExecutableSegment)
    );
}

#[test]
fn embedded_package_provider_resolves_immutable_executables() {
    static APPD_BYTES: &[u8] = b"appd-elf";
    static ENTRIES: &[EmbeddedPackageEntry] = &[EmbeddedPackageEntry {
        package_name: "system:appd",
        path: "/pkg/bin/appd",
        bytes: APPD_BYTES,
        metadata: PackageImageMetadata {
            immutable: true,
            trusted: true,
        },
    }];

    let provider = EmbeddedPackageProvider::new(ENTRIES);
    let image = provider
        .resolve_executable("system:appd", "/pkg/bin/appd")
        .expect("embedded appd should resolve");

    assert_eq!(image.bytes, APPD_BYTES);
    assert!(image.metadata.immutable);
    assert_eq!(
        provider.resolve_executable("system:appd", "bin/appd"),
        Err(PackageLoadError::InvalidPath)
    );
    assert_eq!(
        provider.resolve_executable("system:other", "/pkg/bin/appd"),
        Err(PackageLoadError::NotFound)
    );
}

#[test]
fn bootfs_parser_resolves_aligned_payloads_by_path() {
    let image = bootfs_image(&[
        ("/boot/pkg/bexos.platform.appd/bin/appd", b"appd".as_slice()),
        ("/boot/manifest/bootfs_manifest.bin", b"manifest".as_slice()),
    ]);

    let bootfs = Bootfs::parse(&image).expect("bootfs");
    assert_eq!(bootfs.count(), 2);
    assert_eq!(bootfs.table_size(), (ENTRY_SIZE * 2) as u32);

    let appd = bootfs
        .find("/boot/pkg/bexos.platform.appd/bin/appd")
        .expect("lookup")
        .expect("appd entry");
    assert_eq!(appd.bytes, b"appd");

    assert!(bootfs.find("/missing").expect("lookup").is_none());
}

#[test]
fn mmu_descriptors_set_expected_valid_address_and_attr_bits() {
    if bexos_kernel_core::runtime::Context::ARCHITECTURE == 2 {
        assert_eq!(table_descriptor(0x4010_1234), 0x4010_1007);
        let device = block_descriptor_1g(0, MemoryAttr::Device, Access::KernelReadWrite);
        assert_eq!(device & 0x9b, 0x9b);
        assert_ne!(device & (1 << 63), 0);
        let normal = page_descriptor(0x4020_0123, MemoryAttr::Normal, Access::KernelReadOnly);
        assert_eq!(normal, 0x4020_0001 | (1 << 63));
        let user = page_descriptor(0x4030_0000, MemoryAttr::Normal, Access::KernelUserReadWrite);
        assert_eq!(user, 0x4030_0007 | (1 << 63));
        return;
    }
    assert_eq!(table_descriptor(0x4010_1234), 0x4010_1003);

    let device_block = block_descriptor_1g(0, MemoryAttr::Device, Access::KernelReadWrite);
    assert_eq!(device_block & 0b11, 0b01);
    assert_ne!(device_block & (1 << 54), 0);

    let normal_page = page_descriptor(0x4020_0123, MemoryAttr::Normal, Access::KernelReadOnly);
    assert_eq!(normal_page & 0x0000_FFFF_FFFF_F000, 0x4020_0000);
    assert_eq!((normal_page >> 2) & 0b111, 1);
    assert_eq!((normal_page >> 6) & 0b11, 0b10);

    let user_page = page_descriptor(0x4030_0000, MemoryAttr::Normal, Access::KernelUserReadWrite);
    assert_eq!((user_page >> 6) & 0b11, 0b01);

    let user_code = user_code_page_descriptor(0x4040_0000);
    assert_eq!(user_code & (1 << 54), 0);
    assert_ne!(user_code & (1 << 53), 0);

    let user_data = user_data_page_descriptor(0x4050_0000);
    assert_ne!(user_data & (1 << 54), 0);
}

#[test]
fn aarch64_cpu_feature_decoding_tracks_tier_state() {
    let features = Aarch64CpuFeatures::from_registers(
        2 << 4 | 1 << 20,
        1 << 56 | 1 << 60,
        1 | 1 << 4,
        1 << 60,
        1 << 4 | 1 << 36,
        1,
        62_500_000,
    );

    assert_eq!(features.asid, AsidSupport::Bits16);
    assert!(features.fp_simd);
    assert!(features.pan);
    assert!(features.bti);
    assert!(features.pointer_auth);
    assert!(features.pac_address);
    assert!(!features.pac_generic);
    assert!(features.speculation_barrier);
    assert!(features.ssbs);
    assert!(features.csv2);
    assert!(features.csv3);
    assert!(features.rndr);
    assert_eq!(features.timer_frequency_hz, 62_500_000);

    let policy = KernelFeatureState::from_detected(features);
    assert!(policy.bti_active);
    assert!(policy.pointer_auth_active);
    assert!(policy.speculation_controls_active);

    let minimal = Aarch64CpuFeatures::from_registers(0, 0xf << 16, 0, 0, 0, 0, 1);
    assert_eq!(minimal.asid, AsidSupport::None);
    assert!(!minimal.fp_simd);
    assert!(!KernelFeatureState::from_detected(minimal).pointer_auth_active);
}

#[test]
fn psci_contract_defines_qemu_aarch64_function_ids() {
    assert_eq!(PsciFunction::Version.id(), 0x8400_0000);
    assert_eq!(PsciFunction::CpuSuspend64.id(), 0xc400_0001);
    assert_eq!(PsciFunction::CpuOn64.id(), 0xc400_0003);
    assert_eq!(PsciFunction::SystemOff.id(), 0x8400_0008);
    assert_eq!(PsciFunction::SystemReset.id(), 0x8400_0009);
    assert_eq!(PsciFunction::Features.id(), 0x8400_000a);
}

#[test]
fn psci_contract_decodes_version_feature_and_signed_errors() {
    assert_eq!(
        PsciVersion::decode(0x0001_0001),
        Ok(PsciVersion { major: 1, minor: 1 })
    );
    assert_eq!(decode_status(0), Ok(()));
    assert_eq!(decode_status(u64::MAX), Err(PsciError::NotSupported));
    assert_eq!(
        decode_status((-2_i64) as u64),
        Err(PsciError::InvalidParameters)
    );
    assert_eq!(
        decode_status((-9_i64) as u64),
        Err(PsciError::InvalidAddress)
    );
    assert_eq!(
        decode_status((-42_i64) as u64),
        Err(PsciError::Unknown(-42))
    );
    assert_eq!(
        PsciVersion::decode((-1_i64) as u64),
        Err(PsciError::NotSupported)
    );
    assert_eq!(decode_feature_status(0), Ok(()));
    assert_eq!(
        decode_feature_status((-1_i64) as u64),
        Err(PsciError::NotSupported)
    );
}

#[test]
fn psci_contract_maps_supported_power_operations() {
    assert_eq!(
        function_for_power_operation(SystemPowerOperation::Standby),
        PsciFunction::CpuSuspend64
    );
    assert_eq!(
        function_for_power_operation(SystemPowerOperation::Reboot),
        PsciFunction::SystemReset
    );
    assert_eq!(
        function_for_power_operation(SystemPowerOperation::Poweroff),
        PsciFunction::SystemOff
    );
}

#[test]
fn user_page_descriptors_enforce_nx_and_wx_is_rejected() {
    let data = user_data_page_descriptor(0x4050_0000);
    let code = user_code_page_descriptor(0x4040_0000);
    if bexos_kernel_core::runtime::Context::ARCHITECTURE == 2 {
        assert_eq!(data & 7, 7);
        assert_ne!(data & (1 << 63), 0);
        assert_eq!(code & 7, 5);
        assert_eq!(code & (1 << 63), 0);
    } else {
        assert_ne!(data & (1 << 54), 0);
        assert_ne!(data & (1 << 53), 0);
        assert_eq!(code & (1 << 54), 0);
        assert_ne!(code & (1 << 53), 0);
        assert_ne!(code & (0b10 << 6), 0);
    }

    let mut plane = ControlPlane::new();
    let vmo = plane.create_vmo(PAGE_SIZE as u64, 0).expect("vmo");
    let denied = plane.map_vmo(
        vmo,
        0,
        PAGE_SIZE as u64,
        0,
        RIGHT_READ | RIGHT_WRITE | RIGHT_EXECUTE,
    );
    assert_eq!(denied.status, KernelServiceStatus::AccessDenied);
}

#[test]
fn pci_ecam_offsets_and_bar_sizes_are_decoded() {
    let address = PciAddress {
        bus: 2,
        device: 3,
        function: 4,
    };
    assert_eq!(address.ecam_offset(0x10), 0x0021_c010);

    assert!(
        PciClass {
            class: 0x01,
            subclass: 0x08,
            prog_if: 0x02
        }
        .is_nvme()
    );
    assert!(
        !PciClass {
            class: 0x01,
            subclass: 0x06,
            prog_if: 0x01
        }
        .is_nvme()
    );

    assert_eq!(
        decode_bar_size(0xffff_c000 | BAR_PREFETCHABLE, 0, false),
        0x4000
    );
    assert_eq!(
        decode_bar_size(0xffff_c000 | BAR_TYPE_64, 0xffff_ffff, true),
        0x4000
    );
    assert_eq!(align_resource_base(0x1000_1000, 0x4000), 0x1000_4000);
    assert_eq!(command_with_memory_and_bus_master(0xffff), 0xfffe);
}

#[test]
fn nvme_helpers_decode_queue_and_namespace_layout() {
    assert_eq!(nvme::queue_doorbell_stride(0), 4);
    assert_eq!(nvme::queue_doorbell_stride(2u64 << 32), 16);
    assert_eq!(nvme::admin_queue_attrs(16), 0x000f_000f);

    let mut identify_namespace = [0u8; 4096];
    identify_namespace[0..8].copy_from_slice(&128u64.to_le_bytes());
    identify_namespace[26] = 0;
    identify_namespace[128 + 2] = 9;
    assert_eq!(nvme::namespace_block_count(&identify_namespace), Some(128));
    assert_eq!(nvme::namespace_block_size(&identify_namespace), Some(512));
}

#[test]
fn round_robin_scheduler_rotates_tasks() {
    let mut scheduler = RoundRobinScheduler::<3>::new();
    scheduler
        .add_task(Task { id: 1, name: "a" })
        .expect("space for task");
    scheduler
        .add_task(Task { id: 2, name: "b" })
        .expect("space for task");
    scheduler
        .add_task(Task { id: 3, name: "c" })
        .expect("space for task");

    assert_eq!(scheduler.current().unwrap().id, 1);
    assert_eq!(scheduler.tick().unwrap().id, 2);
    assert_eq!(scheduler.tick().unwrap().id, 3);
    assert_eq!(scheduler.tick().unwrap().id, 1);
    assert_eq!(scheduler.ticks(), 3);
}

#[test]
fn round_robin_scheduler_reports_full_queue() {
    let mut scheduler = RoundRobinScheduler::<1>::new();
    scheduler
        .add_task(Task { id: 1, name: "a" })
        .expect("first task fits");

    assert_eq!(
        scheduler.add_task(Task { id: 2, name: "b" }),
        Err(SchedulerError::Full)
    );
}

#[test]
fn transplant_cpu_context_requires_resumable_aarch64_state() {
    let context = Aarch64CpuContextRecord {
        cpu_id: 0,
        program_counter: 0x4020_1000,
        stack_pointer: 0x4600_2000,
        pstate: 0x3c5,
        ttbr0_el1: 0x1000,
        ttbr1_el1: 0x2000,
        vbar_el1: 0x4000_0000,
    };
    assert_eq!(context.validate(), Ok(()));

    let bad = Aarch64CpuContextRecord {
        stack_pointer: 0x4600_2001,
        ..context
    };
    assert_eq!(bad.validate(), Err(TransplantError::InvalidCpuContext));
}

#[test]
fn scheduler_rotates_priorities_blocks_wakes_and_accounts_ticks() {
    let mut scheduler = Scheduler::new();
    scheduler
        .add_task(SchedulerTask::new(1, 1, 1, 1, "background").unwrap())
        .unwrap();
    scheduler
        .add_task(SchedulerTask::new(2, 1, 2, 3, "foreground-a").unwrap())
        .unwrap();
    scheduler
        .add_task(SchedulerTask::new(3, 2, 2, 3, "foreground-b").unwrap())
        .unwrap();

    assert_eq!(scheduler.current().unwrap().id, 1);
    let decision = scheduler.tick();
    assert!(!decision.switched);
    assert_eq!(scheduler.current().unwrap().id, 1);

    scheduler.tick();
    let decision = scheduler.tick();
    assert!(decision.switched);
    assert_eq!(scheduler.current().unwrap().id, 2);

    let blocked = scheduler
        .block_current(BlockReason::Futex { uaddr: 0x2000 })
        .unwrap();
    assert_eq!(blocked.previous_task_id, Some(2));
    assert_eq!(scheduler.task(2).unwrap().state, TaskState::Blocked);
    assert_eq!(scheduler.current().unwrap().id, 3);

    scheduler.wake_task(2).unwrap();
    assert_eq!(scheduler.task(2).unwrap().state, TaskState::Ready);
    assert!(scheduler.group_accounting(1).unwrap().cpu_ticks > 0);
    assert_eq!(scheduler.cleanup_exited(), 0);
}

#[test]
fn scheduler_enforces_resource_group_cpu_shares_for_fair_tasks() {
    let mut scheduler = Scheduler::new();
    scheduler
        .ensure_resource_group_with_shares(1, 2048)
        .unwrap();
    scheduler.ensure_resource_group_with_shares(2, 256).unwrap();
    scheduler
        .add_task(SchedulerTask::new(1, 1, 1, 1, "high-share").unwrap())
        .unwrap();
    scheduler
        .add_task(SchedulerTask::new(2, 2, 2, 1, "low-share").unwrap())
        .unwrap();

    for _ in 0..96 {
        scheduler.tick();
    }

    let high = scheduler.group_accounting(1).unwrap().cpu_ticks;
    let low = scheduler.group_accounting(2).unwrap().cpu_ticks;
    assert!(high > low.saturating_mul(4), "high={high} low={low}");
    assert_eq!(scheduler.group_accounting(1).unwrap().cpu_shares, 2048);
    assert_eq!(scheduler.group_accounting(2).unwrap().cpu_shares, 256);
}

#[test]
fn scheduler_starts_late_resource_groups_at_the_fair_runtime_baseline() {
    let mut scheduler = Scheduler::new();
    scheduler
        .add_task(SchedulerTask::new(1, 1, 1, 1, "bootstrap").unwrap())
        .unwrap();

    scheduler.tick_at_on_cpu(0, 1_000_000_000).unwrap();
    let baseline = scheduler.group_accounting(1).unwrap().fair_vruntime;
    assert!(baseline > 0);

    scheduler.ensure_resource_group(2).unwrap();
    assert_eq!(
        scheduler.group_accounting(2).unwrap().fair_vruntime,
        baseline
    );
    scheduler
        .add_task(SchedulerTask::new(2, 2, 2, 1, "late-worker").unwrap())
        .unwrap();
}

#[test]
fn scheduler_activates_precreated_idle_group_at_the_fair_runtime_baseline() {
    let mut scheduler = Scheduler::new();
    scheduler.ensure_resource_group(2).unwrap();
    scheduler
        .add_task(SchedulerTask::new(1, 1, 1, 1, "bootstrap").unwrap())
        .unwrap();
    for _ in 0..8 {
        scheduler.yield_current_to(None).unwrap();
    }
    let baseline = scheduler.group_accounting(1).unwrap().fair_vruntime;

    scheduler
        .add_task(SchedulerTask::new(2, 2, 2, 1, "late-service").unwrap())
        .unwrap();

    assert_eq!(
        scheduler.group_accounting(2).unwrap().fair_vruntime,
        baseline
    );
    assert_eq!(
        scheduler.yield_current_to(None).unwrap().next_task_id,
        Some(2)
    );
}

#[test]
fn scheduler_selects_highest_priority_task_inside_chosen_resource_group() {
    let mut scheduler = Scheduler::new();
    scheduler
        .ensure_resource_group_with_shares(1, 1024)
        .unwrap();
    scheduler
        .add_task(SchedulerTask::new(1, 1, 1, 1, "low").unwrap())
        .unwrap();
    scheduler
        .add_task(SchedulerTask::new(2, 1, 1, 8, "high").unwrap())
        .unwrap();

    for _ in 0..3 {
        scheduler.tick();
    }

    assert_eq!(scheduler.current().unwrap().id, 2);
}

#[test]
fn scheduler_admits_edf_before_fair_tasks_and_rejects_overcommit() {
    let mut scheduler = Scheduler::new();
    scheduler
        .add_task(SchedulerTask::new(1, 1, 1, 10, "fair").unwrap())
        .unwrap();
    scheduler
        .add_task(SchedulerTask::new(2, 1, 1, 1, "deadline-a").unwrap())
        .unwrap();
    scheduler
        .add_task(SchedulerTask::new(3, 1, 1, 1, "deadline-b").unwrap())
        .unwrap();

    scheduler
        .set_profile(
            2,
            SchedulingProfile::Deadline(DeadlineProfile {
                capacity_ns: 2,
                deadline_ns: 5,
                period_ns: 10,
            }),
        )
        .unwrap();
    scheduler
        .set_profile(
            3,
            SchedulingProfile::Deadline(DeadlineProfile {
                capacity_ns: 3,
                deadline_ns: 4,
                period_ns: 10,
            }),
        )
        .unwrap();
    assert_eq!(
        scheduler.set_profile(
            1,
            SchedulingProfile::Deadline(DeadlineProfile {
                capacity_ns: 6,
                deadline_ns: 10,
                period_ns: 10,
            }),
        ),
        Err(SchedulerError::ResourceExhausted)
    );

    scheduler.yield_current_to(None).unwrap();
    assert_eq!(scheduler.current().unwrap().id, 3);
}

#[test]
fn deadline_renderer_exhaustion_leaves_time_for_boot_services() {
    let mut scheduler = Scheduler::new();
    scheduler
        .add_task(SchedulerTask::new(1, 1, 1, 10, "storage").unwrap())
        .unwrap();
    scheduler
        .add_task(SchedulerTask::new(2, 1, 1, 1, "renderer").unwrap())
        .unwrap();
    scheduler.tick_at_on_cpu(0, 1_000_000_000).unwrap();
    scheduler
        .set_deadline_profile(
            2,
            DeadlineProfile {
                capacity_ns: 3_000_000,
                deadline_ns: 16_667_000,
                period_ns: 16_667_000,
            },
        )
        .unwrap();
    assert_eq!(
        scheduler
            .task(2)
            .unwrap()
            .deadline
            .unwrap()
            .absolute_deadline_ns,
        1_016_667_000
    );
    scheduler.yield_current_to(Some(2)).unwrap();
    scheduler.tick_at_on_cpu(0, 1_003_000_000).unwrap();
    assert_eq!(scheduler.current().unwrap().id, 1);
    assert_eq!(
        scheduler
            .task(2)
            .unwrap()
            .deadline
            .unwrap()
            .remaining_budget_ns,
        0
    );
    scheduler.yield_current_to(None).unwrap();
    assert_eq!(scheduler.current().unwrap().id, 1);
    scheduler.tick_at_on_cpu(0, 1_016_667_000).unwrap();
    assert_eq!(scheduler.current().unwrap().id, 2);
    assert_eq!(
        scheduler
            .task(2)
            .unwrap()
            .deadline
            .unwrap()
            .remaining_budget_ns,
        3_000_000
    );
    // A late timer skips missed periods, without accumulating unused capacity.
    scheduler.tick_at_on_cpu(0, 1_100_000_000).unwrap();
    assert!(
        scheduler
            .task(2)
            .unwrap()
            .deadline
            .unwrap()
            .remaining_budget_ns
            <= 3_000_000
    );
}

#[test]
fn yielding_deadline_renderers_cannot_starve_storage() {
    let mut scheduler = Scheduler::new();
    scheduler.account_runtime(0);
    for (id, name) in [(1, "storage"), (2, "splash"), (3, "compositor")] {
        scheduler
            .add_task(SchedulerTask::new(id, 1, 1, 10, name).unwrap())
            .unwrap();
        if id != 1 {
            scheduler
                .set_deadline_profile(
                    id,
                    DeadlineProfile {
                        capacity_ns: 3_000_000,
                        deadline_ns: 16_000_000,
                        period_ns: 16_000_000,
                    },
                )
                .unwrap();
        }
    }
    scheduler.yield_current_to_at_on_cpu(0, Some(2), 0).unwrap();
    for turn in 1..=6 {
        scheduler
            .yield_current_to_at_on_cpu(0, None, turn * 1_000_000)
            .unwrap();
    }
    assert_eq!(scheduler.current().unwrap().id, 1);
    for id in [2, 3] {
        assert_eq!(
            scheduler
                .task(id)
                .unwrap()
                .deadline
                .unwrap()
                .remaining_budget_ns,
            0
        );
    }
    scheduler
        .yield_current_to_at_on_cpu(0, None, 7_000_000)
        .unwrap();
    assert_eq!(scheduler.current().unwrap().id, 1);
    scheduler.tick_at_on_cpu(0, 16_000_000).unwrap();
    assert_ne!(scheduler.current().unwrap().id, 1);
    for id in [2, 3] {
        assert_eq!(
            scheduler
                .task(id)
                .unwrap()
                .deadline
                .unwrap()
                .remaining_budget_ns,
            3_000_000
        );
    }
}

#[test]
fn deadline_budget_is_not_charged_twice_at_a_timer_boundary() {
    let mut scheduler = Scheduler::new();
    scheduler.account_runtime(0);
    scheduler
        .add_task(SchedulerTask::new(1, 1, 1, 10, "storage").unwrap())
        .unwrap();
    scheduler
        .add_task(SchedulerTask::new(2, 1, 1, 10, "renderer").unwrap())
        .unwrap();
    scheduler
        .set_deadline_profile(
            2,
            DeadlineProfile {
                capacity_ns: 3_000_000,
                deadline_ns: 16_000_000,
                period_ns: 16_000_000,
            },
        )
        .unwrap();
    scheduler.yield_current_to_at_on_cpu(0, Some(2), 0).unwrap();
    scheduler.account_runtime(1_000_000);
    scheduler.tick_at_on_cpu(0, 2_000_000).unwrap();
    assert_eq!(scheduler.current().unwrap().id, 2);
    assert_eq!(
        scheduler
            .task(2)
            .unwrap()
            .deadline
            .unwrap()
            .remaining_budget_ns,
        1_000_000
    );
    scheduler
        .yield_current_to_at_on_cpu(0, None, 3_000_000)
        .unwrap();
    assert_eq!(scheduler.current().unwrap().id, 1);
}

#[test]
fn resource_group_handles_create_query_and_update_limits() {
    let mut plane = ControlPlane::new();
    let (id, handle) = plane
        .create_resource_group("camera_fg", 768, 4096)
        .expect("resource group");

    assert_eq!(id, 5);
    let group = plane.get_resource_group(handle).expect("group handle");
    assert_eq!(group.id, id);
    assert_eq!(group.name_str().unwrap(), "camera_fg");
    assert_eq!(group.cpu_shares, 768);
    assert_eq!(group.memory_limit_pages, 4096);
    assert_eq!(
        plane.scheduler.group_accounting(id).unwrap().cpu_shares,
        768
    );

    let updated = plane
        .set_resource_group_limits(handle, 512, 8192)
        .expect("updated limits");
    assert_eq!(updated.cpu_shares, 512);
    assert_eq!(updated.memory_limit_pages, 8192);
    assert_eq!(
        plane.scheduler.group_accounting(id).unwrap().cpu_shares,
        512
    );
    plane
        .create_process("camera", id, "com.example.camera", HardwareAccess::None)
        .unwrap();
    let records = plane.list_process_debug_info().unwrap();
    let process = records.iter().find(|p| p.resource_group_id == id).unwrap();
    assert_eq!(
        &process.resource_group_name[..process.resource_group_name_len as usize],
        b"camera_fg"
    );
    assert_eq!(process.parent_resource_group_id, 2);
}

#[test]
fn resource_group_creation_rejects_invalid_and_duplicate_groups() {
    let mut plane = ControlPlane::new();
    assert_eq!(
        plane.create_resource_group("", 1, 0),
        Err(KernelServiceStatus::InvalidArgs)
    );
    assert_eq!(
        plane.create_resource_group("zero", 0, 0),
        Err(KernelServiceStatus::InvalidArgs)
    );

    plane
        .create_resource_group("background_sync", 64, 0)
        .expect("first group");
    assert_eq!(
        plane.create_resource_group("background_sync", 64, 0),
        Err(KernelServiceStatus::AlreadyExists)
    );
}

#[test]
fn gpu_reservation_close_releases_child_and_ancestor_budget() {
    let mut plane = ControlPlane::new();
    let (process, vm_space) = plane
        .create_process("appd", 1, "bexos.platform.appd", HardwareAccess::None)
        .expect("current process");
    let thread = plane
        .start_thread_in_process(process, vm_space, 0x2000_0000, 0x3000_0000, None)
        .expect("thread");
    let thread_id = plane.handles.get(thread.raw).unwrap().object_id;
    let (group_id, group_handle) = plane
        .create_resource_group_v2("gpu_client", None, 1024, 0, false, 0, 0, 60, 4096)
        .expect("gpu resource group");

    let reservation = plane
        .reserve_gpu_resources(group_handle, 25, 1024)
        .expect("reservation");
    assert_eq!(
        plane
            .resource_groups
            .get(group_id)
            .unwrap()
            .reserved_render_budget_percent,
        25
    );
    assert_eq!(
        plane
            .resource_groups
            .get(2)
            .unwrap()
            .reserved_render_budget_percent,
        25
    );

    plane.close_handle(reservation).expect("close reservation");
    assert_eq!(
        plane
            .resource_groups
            .get(group_id)
            .unwrap()
            .reserved_render_budget_percent,
        0
    );
    assert_eq!(plane.resource_groups.get(2).unwrap().reserved_vram_bytes, 0);

    let reservation = plane
        .reserve_gpu_resources(group_handle, 25, 1024)
        .expect("reservation");
    assert_ne!(reservation.raw, 0);
    assert_eq!(
        plane
            .resource_groups
            .get(group_id)
            .unwrap()
            .reserved_vram_bytes,
        1024
    );
    plane.threads.set_current(thread_id).unwrap();
    assert_eq!(plane.exit_thread(0), KernelServiceStatus::Ok);
    assert_eq!(
        plane
            .resource_groups
            .get(group_id)
            .unwrap()
            .reserved_vram_bytes,
        0
    );
}

#[test]
fn anonymous_vmos_charge_resource_group_hierarchy_until_release() {
    let mut plane = ControlPlane::new();
    let (group_id, _) = plane
        .create_resource_group_v2(
            "mem_client",
            None,
            1024,
            0,
            false,
            0,
            PAGE_SIZE as u64,
            0,
            0,
        )
        .expect("memory resource group");
    plane
        .create_process("app", group_id, "com.example.mem", HardwareAccess::None)
        .expect("current process");

    let vmo = plane.create_vmo(PAGE_SIZE as u64, 0).expect("vmo");
    assert_eq!(
        plane
            .resource_groups
            .get(group_id)
            .unwrap()
            .memory_used_bytes,
        PAGE_SIZE as u64
    );
    assert_eq!(
        plane.resource_groups.get(2).unwrap().memory_used_bytes,
        PAGE_SIZE as u64
    );
    assert_eq!(
        plane.create_vmo(PAGE_SIZE as u64, 0),
        Err(KernelServiceStatus::ResourceExhausted)
    );

    assert_eq!(plane.release_vmo(vmo), KernelServiceStatus::Ok);
    assert_eq!(
        plane
            .resource_groups
            .get(group_id)
            .unwrap()
            .memory_used_bytes,
        0
    );
    assert_eq!(plane.resource_groups.get(2).unwrap().memory_used_bytes, 0);
}

#[test]
fn scheduler_directed_yield_runs_ready_target() {
    let mut scheduler = Scheduler::new();
    scheduler
        .add_task(SchedulerTask::new(1, 1, 1, 5, "caller").unwrap())
        .unwrap();
    scheduler
        .add_task(SchedulerTask::new(2, 1, 1, 1, "server").unwrap())
        .unwrap();

    let decision = scheduler.yield_current_to(Some(2)).unwrap();
    assert!(decision.switched);
    assert_eq!(scheduler.current().unwrap().id, 2);
}

#[test]
fn scheduler_undirected_yield_runs_a_peer_from_an_equal_group() {
    let mut scheduler = Scheduler::new();
    scheduler
        .add_task(SchedulerTask::new(1, 1, 1, 5, "caller").unwrap())
        .unwrap();
    scheduler.ensure_resource_group(2).unwrap();
    scheduler
        .add_task(SchedulerTask::new(2, 2, 2, 5, "peer").unwrap())
        .unwrap();

    let decision = scheduler.yield_current_to(None).unwrap();

    assert!(decision.switched);
    assert_eq!(scheduler.current().unwrap().id, 2);
    assert_eq!(scheduler.task(1).unwrap().state, TaskState::Ready);
}

#[test]
fn scheduler_undirected_yield_rotates_equal_resource_groups() {
    let mut scheduler = Scheduler::new();
    for id in 1..=3 {
        scheduler.ensure_resource_group(id).unwrap();
        scheduler
            .add_task(SchedulerTask::new(id as u64, id as u64, id, 5, "peer").unwrap())
            .unwrap();
    }

    assert_eq!(scheduler.current().unwrap().id, 1);
    assert_eq!(
        scheduler.yield_current_to(None).unwrap().next_task_id,
        Some(2)
    );
    assert_eq!(
        scheduler.yield_current_to(None).unwrap().next_task_id,
        Some(3)
    );
    assert_eq!(
        scheduler.yield_current_to(None).unwrap().next_task_id,
        Some(1)
    );
}

#[test]
fn scheduler_polling_group_does_not_starve_timer_preempted_peer() {
    let mut scheduler = Scheduler::new();
    scheduler
        .add_task(SchedulerTask::new(1, 1, 1, 5, "appd").unwrap())
        .unwrap();
    scheduler.ensure_resource_group(4).unwrap();
    scheduler
        .add_task(SchedulerTask::new(2, 2, 4, 5, "polling-driver").unwrap())
        .unwrap();

    assert_eq!(
        scheduler
            .tick_at_on_cpu(0, DEFAULT_FAIR_QUANTUM_NS)
            .unwrap()
            .next_task_id,
        Some(2)
    );
    assert_eq!(
        scheduler.yield_current_to(None).unwrap().next_task_id,
        Some(1)
    );
}

#[test]
fn scheduler_bounds_service_lag_after_long_driver_work() {
    let mut scheduler = Scheduler::new();
    scheduler
        .ensure_resource_group_with_limits(1, None, 2048, 0, true)
        .unwrap();
    scheduler
        .ensure_resource_group_with_limits(4, Some(1), 1536, 0, false)
        .unwrap();
    scheduler
        .add_task(SchedulerTask::new(1, 1, 1, 5, "polling-app").unwrap())
        .unwrap();
    scheduler
        .add_task(SchedulerTask::new(2, 2, 4, 5, "driver").unwrap())
        .unwrap();

    assert_eq!(
        scheduler
            .tick_at_on_cpu(0, DEFAULT_FAIR_QUANTUM_NS)
            .unwrap()
            .next_task_id,
        Some(2)
    );
    scheduler.tick_at_on_cpu(0, 10_000_000_000).unwrap();

    let mut driver_scheduled = scheduler.current().is_some_and(|task| task.id == 2);
    for _ in 0..=12 {
        if driver_scheduled {
            break;
        }
        driver_scheduled = scheduler.yield_current_to(None).unwrap().next_task_id == Some(2);
    }
    assert!(driver_scheduled, "driver exceeded bounded fair-service lag");
}

#[test]
fn scheduler_undirected_yield_rotates_peers_within_driver_group() {
    let mut scheduler = Scheduler::with_cpu_count(4).unwrap();
    let mut appd = SchedulerTask::new(1, 1, 1, 1, "appd").unwrap();
    appd.cpu_affinity_mask = 1;
    scheduler.add_task(appd).unwrap();
    scheduler.ensure_resource_group(4).unwrap();
    for id in 2..=4 {
        let mut driver = SchedulerTask::new(id, id, 4, 1, "driver").unwrap();
        driver.cpu_affinity_mask = 1;
        scheduler.add_task(driver).unwrap();
    }

    assert_eq!(scheduler.current().unwrap().id, 1);
    assert_eq!(
        scheduler.yield_current_to(None).unwrap().next_task_id,
        Some(2)
    );
    assert_eq!(
        scheduler.yield_current_to(None).unwrap().next_task_id,
        Some(1)
    );
    assert_eq!(
        scheduler.yield_current_to(None).unwrap().next_task_id,
        Some(3)
    );
    assert_eq!(
        scheduler.yield_current_to(None).unwrap().next_task_id,
        Some(1)
    );
    assert_eq!(
        scheduler.yield_current_to(None).unwrap().next_task_id,
        Some(4)
    );
}

#[test]
fn scheduler_late_task_rotates_with_existing_group() {
    let mut scheduler = Scheduler::new();
    scheduler
        .add_task(SchedulerTask::new(1, 1, 1, 1, "appd").unwrap())
        .unwrap();
    for _ in 0..8 {
        scheduler.yield_current_to(None).unwrap();
    }
    scheduler
        .add_task(SchedulerTask::new(2, 2, 1, 1, "late-service").unwrap())
        .unwrap();

    assert_eq!(
        scheduler.yield_current_to(None).unwrap().next_task_id,
        Some(2)
    );
    assert_eq!(
        scheduler.yield_current_to(None).unwrap().next_task_id,
        Some(1)
    );
}

#[test]
fn scheduler_voluntary_yield_rearms_quantum_from_current_time() {
    let mut scheduler = Scheduler::new();
    scheduler
        .add_task(SchedulerTask::new(1, 1, 1, 1, "first").unwrap())
        .unwrap();
    scheduler
        .add_task(SchedulerTask::new(2, 2, 1, 1, "second").unwrap())
        .unwrap();

    let now_ns = DEFAULT_FAIR_QUANTUM_NS * 10;
    assert_eq!(
        scheduler
            .yield_current_to_at_on_cpu(0, None, now_ns)
            .unwrap()
            .next_task_id,
        Some(2)
    );
    assert_eq!(scheduler.now_ns(), now_ns);
    assert_eq!(
        scheduler.next_deadline_on_cpu(0).unwrap(),
        Some(now_ns + DEFAULT_FAIR_QUANTUM_NS)
    );
}

#[test]
fn smp_scheduler_runs_distinct_tasks_on_distinct_cpus() {
    let mut scheduler = Scheduler::with_cpu_count(2).unwrap();
    scheduler
        .add_task(SchedulerTask::new(1, 1, 1, 5, "cpu0").unwrap())
        .unwrap();
    scheduler
        .add_task(SchedulerTask::new(2, 1, 1, 5, "cpu1").unwrap())
        .unwrap();

    assert_eq!(scheduler.current_on_cpu(0).unwrap().id, 1);
    assert_eq!(scheduler.current_on_cpu(1).unwrap().id, 2);
    assert_ne!(
        scheduler.current_on_cpu(0).unwrap().id,
        scheduler.current_on_cpu(1).unwrap().id
    );
}

#[test]
fn smp_scheduler_respects_cpu_affinity_masks() {
    let mut scheduler = Scheduler::with_cpu_count(2).unwrap();
    let mut cpu1_only = SchedulerTask::new(1, 1, 1, 5, "cpu1-only").unwrap();
    cpu1_only.cpu_affinity_mask = 0b10;
    scheduler.add_task(cpu1_only).unwrap();

    assert!(scheduler.current_on_cpu(0).is_none());
    assert_eq!(scheduler.current_on_cpu(1).unwrap().id, 1);
}

#[test]
fn smp_scheduler_rejects_invalid_affinity_masks() {
    let mut scheduler = Scheduler::with_cpu_count(2).unwrap();
    scheduler
        .add_task(SchedulerTask::new(1, 1, 1, 5, "worker").unwrap())
        .unwrap();

    assert_eq!(
        scheduler.set_cpu_affinity(1, 0),
        Err(SchedulerError::InvalidTask)
    );
    assert_eq!(
        scheduler.set_cpu_affinity(1, 0b100),
        Err(SchedulerError::InvalidTask)
    );
}

#[test]
fn smp_scheduler_rejects_directed_yield_to_ineligible_cpu() {
    let mut scheduler = Scheduler::with_cpu_count(2).unwrap();
    scheduler
        .add_task(SchedulerTask::new(1, 1, 1, 5, "caller").unwrap())
        .unwrap();
    let mut cpu1_only = SchedulerTask::new(2, 1, 1, 5, "server").unwrap();
    cpu1_only.cpu_affinity_mask = 0b10;
    scheduler.add_task(cpu1_only).unwrap();

    assert_eq!(
        scheduler.yield_current_to_on_cpu(0, Some(2)),
        Err(SchedulerError::InvalidTask)
    );
    assert_eq!(scheduler.current_on_cpu(0).unwrap().id, 1);
    assert_eq!(scheduler.current_on_cpu(1).unwrap().id, 2);
}

#[test]
fn smp_scheduler_block_and_exit_are_cpu_local() {
    let mut scheduler = Scheduler::with_cpu_count(2).unwrap();
    scheduler
        .add_task(SchedulerTask::new(1, 1, 1, 5, "first").unwrap())
        .unwrap();
    scheduler
        .add_task(SchedulerTask::new(2, 1, 1, 5, "second").unwrap())
        .unwrap();
    scheduler
        .add_task(SchedulerTask::new(3, 1, 1, 5, "third").unwrap())
        .unwrap();

    let blocked = scheduler
        .block_current_on_cpu(0, BlockReason::Futex { uaddr: 0x2000 })
        .unwrap();
    assert_eq!(blocked.previous_task_id, Some(1));
    assert_eq!(scheduler.current_on_cpu(1).unwrap().id, 2);
    assert_eq!(scheduler.current_on_cpu(0).unwrap().id, 3);

    let exited = scheduler.exit_current_on_cpu(1, 0).unwrap();
    assert_eq!(exited.previous_task_id, Some(2));
    assert_eq!(scheduler.current_on_cpu(0).unwrap().id, 3);
}

#[test]
fn smp_scheduler_accounts_resource_group_ticks_across_cpus() {
    let mut scheduler = Scheduler::with_cpu_count(2).unwrap();
    scheduler
        .ensure_resource_group_with_shares(1, 1024)
        .unwrap();
    scheduler
        .add_task(SchedulerTask::new(1, 1, 1, 5, "first").unwrap())
        .unwrap();
    scheduler
        .add_task(SchedulerTask::new(2, 1, 1, 5, "second").unwrap())
        .unwrap();

    scheduler.tick_on_cpu(0).unwrap();
    scheduler.tick_on_cpu(1).unwrap();

    assert_eq!(scheduler.group_accounting(1).unwrap().cpu_ticks, 2);
}

#[test]
fn channel_transfers_payloads_and_capabilities_point_to_point() {
    let mut channel = Channel::<2>::new();
    let capability = Capability {
        object_id: 42,
        rights: 0b101,
    };
    let message = Message::new(b"hello", Some(capability)).expect("small payload");

    channel
        .send(Endpoint::A, message)
        .expect("receiver queue has space");
    let received = channel
        .receive(Endpoint::B)
        .expect("message for endpoint b");

    assert_eq!(received.payload(), b"hello");
    assert_eq!(received.capability, Some(capability));
    assert_eq!(channel.receive(Endpoint::B), Err(IpcError::Empty));
}

#[test]
fn transplant_snapshot_header_validates_checksum_and_preserved_range() {
    let payload = b"frozen appd broker";
    let preserved = [PreservedRegion::new(0x4600_0000, 0x4000)];
    let header = SnapshotHeader::new(
        SnapshotPhase::SwitchDelta,
        payload,
        0x4600_0100,
        payload.len() as u64,
    )
    .expect("small snapshot");

    assert_eq!(
        header.validate(SnapshotPhase::SwitchDelta, payload, &preserved),
        Ok(())
    );
    assert_eq!(
        header.validate(SnapshotPhase::LiveBulk, payload, &preserved),
        Err(TransplantError::WrongSnapshotPhase)
    );
    assert_eq!(
        header.validate(SnapshotPhase::SwitchDelta, b"changed", &preserved),
        Err(TransplantError::LengthMismatch)
    );

    let outside = SnapshotHeader::new(
        SnapshotPhase::SwitchDelta,
        payload,
        0x4700_0000,
        payload.len() as u64,
    )
    .expect("small snapshot");
    assert_eq!(
        outside.validate(SnapshotPhase::SwitchDelta, payload, &preserved),
        Err(TransplantError::RangeOutsidePreservedRam)
    );
}

#[test]
fn transplant_records_reject_invalid_ranges_and_handles() {
    let preserved = [PreservedRegion::new(0x4600_0000, 0x1000)];
    let task = FrozenTaskRecord {
        task_id: 4,
        entry: 0x4020_0000,
        stack_pointer: 0x4600_0800,
        state_ptr: 0x4600_0100,
        state_len: 0x80,
    };
    let capability = CapabilityHandleRecord {
        object_id: 7,
        rights: 0b11,
        owner_task_id: 4,
    };

    assert_eq!(task.validate(&preserved), Ok(()));
    assert_eq!(capability.validate(), Ok(()));
    assert_eq!(
        FrozenTaskRecord {
            state_ptr: 0x45ff_ffff,
            ..task
        }
        .validate(&preserved),
        Err(TransplantError::RangeOutsidePreservedRam)
    );
    assert_eq!(
        CapabilityHandleRecord {
            object_id: 0,
            ..capability
        }
        .validate(),
        Err(TransplantError::InvalidCapability)
    );
}

#[test]
fn kernel_channel_control_transfers_bytes_and_handles() {
    let mut plane = ControlPlane::new();
    let (local, remote) = plane.create_channel().expect("channel pair");
    let vmo = plane.create_vmo(PAGE_SIZE as u64, 0).expect("vmo handle");

    assert_eq!(
        plane.write_message(local, b"ping", &[vmo]),
        KernelServiceStatus::Ok
    );
    let result = plane.read_message(remote, 8, 1);

    assert_eq!(result.status, KernelServiceStatus::Ok);
    assert_eq!(&plane.scratch.bytes[..result.bytes_len], b"ping");
    assert_eq!(plane.scratch.handles[0], vmo);
}

#[test]
fn virtual_memory_rejects_write_execute_and_tracks_clone() {
    let mut plane = ControlPlane::new();
    let vmo = plane.create_vmo(1, 0).expect("rounded vmo");

    let denied = plane.map_vmo(
        vmo,
        0,
        PAGE_SIZE as u64,
        0,
        RIGHT_READ | RIGHT_WRITE | RIGHT_EXECUTE,
    );
    assert_eq!(denied.status, KernelServiceStatus::AccessDenied);

    let mapped = plane.map_vmo(vmo, 0, PAGE_SIZE as u64, 0, RIGHT_READ);
    assert_eq!(mapped.status, KernelServiceStatus::Ok);
    assert_eq!(
        plane.unmap(mapped.mapped_vaddr, PAGE_SIZE as u64),
        KernelServiceStatus::Ok
    );

    let cloned = plane
        .clone_vmo(vmo, 0, PAGE_SIZE as u64)
        .expect("cow clone metadata");
    assert_ne!(cloned.raw, vmo.raw);
    assert_eq!(plane.vmos.get(1).unwrap().frames.len(), 1);
    assert_eq!(plane.vmos.get(2).unwrap().parent_vmo, Some(1));
    assert_eq!(plane.vmos.get(2).unwrap().frames.len(), 1);
}

#[test]
fn kernel_services_grow_past_the_former_fixed_limits() {
    let mut plane = ControlPlane::new();
    let mut handles = Vec::new();
    for _ in 0..128 {
        let (local, remote) = plane.create_channel().expect("dynamic channel table");
        handles.push(local);
        handles.push(remote);
    }
    assert!(plane.handles.len() > 96);

    let payload = vec![0x5a; 1024];
    assert_eq!(
        plane.write_message(handles[0], &payload, &handles[2..34]),
        KernelServiceStatus::Ok
    );
    let read = plane.read_message(handles[1], payload.len(), 32);
    assert_eq!(read.status, KernelServiceStatus::Ok);
    assert_eq!(plane.scratch.bytes, payload);
    assert_eq!(plane.scratch.handles.len(), 32);
}

#[test]
fn cloned_vmos_share_then_split_frames() {
    let mut plane = ControlPlane::new();
    let parent = plane
        .create_vmo(PAGE_SIZE as u64, VMO_FLAG_CONTIGUOUS_PHYS)
        .expect("parent");
    let clone = plane.clone_vmo(parent, 0, PAGE_SIZE as u64).expect("clone");
    let parent_id = plane.handle_record(parent).unwrap().object_id;
    let clone_id = plane.handle_record(clone).unwrap().object_id;
    let shared = plane.vmos.get(parent_id).unwrap().frames[0].unwrap();
    assert_eq!(plane.vmos.get(clone_id).unwrap().frames[0], Some(shared));
    assert_eq!(plane.vmos.frame_ref_count(shared), 2);

    let (replacement, copied_from) = plane.vmos.resolve_cow_page(clone_id, 0).expect("split");
    assert_eq!(copied_from, Some(shared));
    assert_ne!(replacement, shared);
    assert_eq!(plane.vmos.frame_ref_count(shared), 1);
    assert_eq!(plane.vmos.frame_ref_count(replacement), 1);
}

#[test]
fn vm_space_mapping_is_process_owned_and_releases_backing_frames() {
    let mut plane = ControlPlane::new();
    let (process, vm_space) = plane
        .create_process("appd", 1, "bexos.platform.appd", HardwareAccess::None)
        .expect("process and vm space");
    let vmo = plane
        .create_vmo((PAGE_SIZE * 2) as u64, 0)
        .expect("backed vmo");

    let mapped = plane.map_vmo_in_vm_space(
        vm_space,
        vmo,
        0,
        PAGE_SIZE as u64,
        USER_VMAR_BASE + 0x1000_0000,
        RIGHT_READ,
    );
    assert_eq!(mapped.status, KernelServiceStatus::Ok);

    let overlap = plane.map_vmo_in_vm_space(
        vm_space,
        vmo,
        0,
        PAGE_SIZE as u64,
        USER_VMAR_BASE + 0x1000_0000,
        RIGHT_READ,
    );
    assert_eq!(overlap.status, KernelServiceStatus::AlreadyExists);
    assert_eq!(
        plane.unmap_in_vm_space(vm_space, mapped.mapped_vaddr, PAGE_SIZE as u64),
        KernelServiceStatus::Ok
    );

    assert_eq!(plane.vmos.free_frame_count(), 0);
    assert_eq!(plane.release_vmo(vmo), KernelServiceStatus::Ok);
    assert_eq!(plane.vmos.free_frame_count(), 0);

    let thread = plane
        .start_thread_in_process(
            process,
            vm_space,
            USER_VMAR_BASE + 0x1000_0000,
            USER_VMAR_BASE + 0x2000_0000,
            None,
        )
        .expect("thread in process");
    assert_ne!(thread.raw, 0);
}

#[test]
fn start_thread_in_process_records_initial_thread_pointer() {
    let mut plane = ControlPlane::new();
    let (process, vm_space) = plane
        .create_process("app", 1, "com.example.app", HardwareAccess::None)
        .expect("process and vm space");

    let thread = plane
        .start_thread_in_process_with_thread_pointer(
            process,
            vm_space,
            0x2000_0000,
            0x3000_0000,
            0xbe00_0000,
            None,
        )
        .expect("thread in process");
    let thread_id = plane
        .handle_record(thread)
        .expect("thread handle")
        .object_id;

    assert_eq!(
        plane.threads.get(thread_id).unwrap().saved_tpidr_el0,
        0xbe00_0000
    );
}

#[test]
fn process_creation_installs_root_vmar_for_vm_space() {
    let mut plane = ControlPlane::new();
    let (_, vm_space) = plane
        .create_process("app", 1, "com.example.app", HardwareAccess::None)
        .expect("process and vm space");

    let root = plane
        .root_vmar_for_vm_space(vm_space)
        .expect("root vmar record");
    let root_handle = plane
        .root_vmar_handle_for_vm_space(vm_space)
        .expect("root vmar handle");

    assert_eq!(plane.vmars.len(), 1);
    assert_eq!(root.base, USER_VMAR_BASE);
    assert!(root.parent_vmar_id.is_none());
    assert_ne!(root_handle.raw, 0);
}

#[test]
fn sub_vmars_validate_bounds_overlaps_and_mapping_permissions() {
    let mut plane = ControlPlane::new();
    let (_, vm_space) = plane
        .create_process("app", 1, "com.example.app", HardwareAccess::None)
        .expect("process and vm space");
    let root = plane
        .root_vmar_handle_for_vm_space(vm_space)
        .expect("root vmar");
    let flags = VMAR_FLAG_CAN_MAP_READ | VMAR_FLAG_CAN_MAP_WRITE | VMAR_FLAG_CAN_MAP_SPECIFIC;
    let (sandbox, base) = plane
        .create_sub_vmar(root, 0x0100_0000, (PAGE_SIZE * 4) as u64, flags)
        .expect("child vmar");
    assert_eq!(base, USER_VMAR_BASE + 0x0100_0000);

    assert_eq!(
        plane.create_sub_vmar(root, 0x0100_0000, PAGE_SIZE as u64, flags),
        Err(KernelServiceStatus::AlreadyExists)
    );
    assert_eq!(
        plane.create_sub_vmar(root, u64::MAX, PAGE_SIZE as u64, flags),
        Err(KernelServiceStatus::InvalidArgs)
    );

    let vmo = plane
        .create_vmo((PAGE_SIZE * 2) as u64, 0)
        .expect("backed vmo");
    let executable = plane.map_vmo_in_vmar(
        sandbox,
        vmo,
        0,
        0,
        PAGE_SIZE as u64,
        VMAR_FLAG_CAN_MAP_READ | VMAR_FLAG_CAN_MAP_EXECUTE,
    );
    assert_eq!(executable.status, KernelServiceStatus::AccessDenied);

    let wx = plane.map_vmo_in_vmar(
        sandbox,
        vmo,
        0,
        0,
        PAGE_SIZE as u64,
        VMAR_FLAG_CAN_MAP_READ | VMAR_FLAG_CAN_MAP_WRITE | VMAR_FLAG_CAN_MAP_EXECUTE,
    );
    assert_eq!(wx.status, KernelServiceStatus::AccessDenied);

    let mapped = plane.map_vmo_in_vmar(
        sandbox,
        vmo,
        0,
        PAGE_SIZE as u64,
        PAGE_SIZE as u64,
        VMAR_FLAG_CAN_MAP_READ | VMAR_FLAG_CAN_MAP_WRITE,
    );
    assert_eq!(mapped.status, KernelServiceStatus::Ok);
    assert_eq!(mapped.mapped_vaddr, base + PAGE_SIZE as u64);
}

#[test]
fn vmar_specific_mapping_requires_specific_flag() {
    let mut plane = ControlPlane::new();
    let (_, vm_space) = plane
        .create_process("app", 1, "com.example.app", HardwareAccess::None)
        .expect("process and vm space");
    let root = plane
        .root_vmar_handle_for_vm_space(vm_space)
        .expect("root vmar");
    let (arena, base) = plane
        .create_sub_vmar(
            root,
            0x0200_0000,
            (PAGE_SIZE * 2) as u64,
            VMAR_FLAG_CAN_MAP_READ,
        )
        .expect("child vmar");
    let vmo = plane.create_vmo(PAGE_SIZE as u64, 0).expect("vmo");

    let explicit = plane.map_vmo_in_vmar(
        arena,
        vmo,
        0,
        PAGE_SIZE as u64,
        PAGE_SIZE as u64,
        VMAR_FLAG_CAN_MAP_READ,
    );
    assert_eq!(explicit.status, KernelServiceStatus::AccessDenied);

    let auto = plane.map_vmo_in_vmar(arena, vmo, 0, 0, PAGE_SIZE as u64, VMAR_FLAG_CAN_MAP_READ);
    assert_eq!(auto.status, KernelServiceStatus::Ok);
    assert_eq!(auto.mapped_vaddr, base);
}

#[test]
fn vmar_auto_mapping_skips_reserved_child_regions() {
    let mut plane = ControlPlane::new();
    let (_, vm_space) = plane
        .create_process("app", 1, "com.example.app", HardwareAccess::None)
        .expect("process and vm space");
    let root = plane
        .root_vmar_handle_for_vm_space(vm_space)
        .expect("root vmar");
    plane
        .create_sub_vmar(
            root,
            0,
            PAGE_SIZE as u64,
            VMAR_FLAG_CAN_MAP_READ | VMAR_FLAG_CAN_MAP_SPECIFIC,
        )
        .expect("reserved child vmar");
    let vmo = plane.create_vmo(PAGE_SIZE as u64, 0).expect("vmo");

    let mapped = plane.map_vmo_in_vmar(root, vmo, 0, 0, PAGE_SIZE as u64, VMAR_FLAG_CAN_MAP_READ);

    assert_eq!(mapped.status, KernelServiceStatus::Ok);
    assert_eq!(mapped.mapped_vaddr, USER_VMAR_BASE + PAGE_SIZE as u64);
}

#[test]
fn destroy_vmar_recursively_removes_mappings_and_blocks_reuse() {
    let mut plane = ControlPlane::new();
    let (_, vm_space) = plane
        .create_process("app", 1, "com.example.app", HardwareAccess::None)
        .expect("process and vm space");
    let root = plane
        .root_vmar_handle_for_vm_space(vm_space)
        .expect("root vmar");
    let flags = VMAR_FLAG_CAN_MAP_READ | VMAR_FLAG_CAN_MAP_SPECIFIC;
    let (parent, _) = plane
        .create_sub_vmar(root, 0x0300_0000, (PAGE_SIZE * 4) as u64, flags)
        .expect("parent vmar");
    let (child, child_base) = plane
        .create_sub_vmar(parent, PAGE_SIZE as u64, (PAGE_SIZE * 2) as u64, flags)
        .expect("child vmar");
    let vmo = plane.create_vmo(PAGE_SIZE as u64, 0).expect("vmo");
    let mapped = plane.map_vmo_in_vmar(child, vmo, 0, 0, PAGE_SIZE as u64, VMAR_FLAG_CAN_MAP_READ);

    assert_eq!(mapped.status, KernelServiceStatus::Ok);
    assert_eq!(mapped.mapped_vaddr, child_base);
    assert_eq!(plane.mappings.len(), 1);
    assert_eq!(plane.destroy_vmar(root), KernelServiceStatus::AccessDenied);
    assert_eq!(plane.destroy_vmar(parent), KernelServiceStatus::Ok);
    assert_eq!(plane.mappings.len(), 0);
    assert_eq!(
        plane
            .map_vmo_in_vmar(child, vmo, 0, 0, PAGE_SIZE as u64, VMAR_FLAG_CAN_MAP_READ)
            .status,
        KernelServiceStatus::InvalidHandle
    );
}

#[test]
fn task_control_handles_futexes_and_wait_many_signals() {
    let mut plane = ControlPlane::new();
    let (local, remote) = plane.create_channel().expect("channel pair");
    let (process, vm_space) = plane
        .create_process("appd", 1, "bexos.platform.appd", HardwareAccess::None)
        .expect("process and vm space");
    plane
        .start_thread_in_process(process, vm_space, 0x2000_0000, 0x3000_0000, None)
        .expect("initial thread");

    assert_eq!(
        plane.futex_wait(0x2000, 7, 1, None),
        KernelServiceStatus::Ok
    );
    let wake = plane.futex_wake(0x2000, 4);
    assert_eq!(wake.status, KernelServiceStatus::Ok);
    assert_eq!(wake.woken_count, 1);

    assert_eq!(
        plane.write_message(local, b"ready", &[]),
        KernelServiceStatus::Ok
    );
    let wait = plane.wait_many(
        &[WaitManyItem {
            handle: remote,
            signals: SIGNAL_READABLE,
        }],
        1,
    );
    assert_eq!(wait.status, KernelServiceStatus::Ok);
    assert_eq!(wait.satisfied_index, 0);
}

#[test]
fn wait_many_blocks_until_channel_signal_or_deadline() {
    let mut plane = ControlPlane::new();
    let (local, remote) = plane.create_channel().expect("channel pair");
    let (process, vm_space) = plane
        .create_process("appd", 1, "bexos.platform.appd", HardwareAccess::None)
        .expect("process and vm space");
    plane
        .start_thread_in_process(process, vm_space, 0x2000_0000, 0x3000_0000, None)
        .expect("initial thread");

    let wait = plane.wait_many(
        &[WaitManyItem {
            handle: remote,
            signals: SIGNAL_READABLE,
        }],
        1_000,
    );
    assert_eq!(wait.status, KernelServiceStatus::TimedOut);
    assert!(
        plane
            .scheduler
            .task(plane.threads.current_thread_id())
            .is_some_and(|task| task.block_reason.is_some())
    );

    assert_eq!(
        plane.write_message(local, b"wake", &[]),
        KernelServiceStatus::Ok
    );
    assert!(
        plane
            .scheduler
            .task(plane.threads.current_thread_id())
            .is_some_and(|task| task.block_reason.is_none())
    );
    let retry = plane.wait_many(
        &[WaitManyItem {
            handle: remote,
            signals: SIGNAL_READABLE,
        }],
        0,
    );
    assert_eq!(retry.status, KernelServiceStatus::Ok);
    assert_eq!(retry.satisfied_index, 0);

    let _ = plane.read_message(remote, 16, 0);
    let timeout = plane.wait_many(
        &[WaitManyItem {
            handle: remote,
            signals: SIGNAL_READABLE,
        }],
        2_000,
    );
    assert_eq!(timeout.status, KernelServiceStatus::TimedOut);
    let _ = plane.scheduler.tick_at_on_cpu(0, 2_000);
    assert!(
        plane
            .scheduler
            .task(plane.threads.current_thread_id())
            .is_some_and(|task| task.block_reason.is_none())
    );
}

#[test]
fn privileged_terminate_process_exits_threads_and_releases_handles() {
    let mut plane = ControlPlane::new();
    let (process, vm_space) = plane
        .create_process("worker", 1, "com.example.worker", HardwareAccess::None)
        .expect("process and vm space");
    plane
        .start_thread_in_process(process, vm_space, 0x2000_0000, 0x3000_0000, None)
        .expect("thread");

    assert_eq!(
        plane.terminate_process(process, -9),
        KernelServiceStatus::Ok
    );
    assert_eq!(
        plane.terminate_process(process, -9),
        KernelServiceStatus::Ok
    );
    assert!(
        plane
            .threads
            .get(plane.threads.current_thread_id())
            .is_some_and(|thread| matches!(thread.state, ThreadState::Exited))
    );
    let signals = plane
        .handles
        .get(process.raw)
        .map(|record| record.signals)
        .unwrap_or(0);
    assert_ne!(signals & SIGNAL_TERMINATED, 0);
}

#[test]
fn scheduling_profiles_are_gated_and_apply_to_threads() {
    let mut plane = ControlPlane::new();
    let (process, vm_space) = plane
        .create_process("app", 1, "com.example.app", HardwareAccess::None)
        .expect("process and vm space");
    let thread = plane
        .start_thread_in_process(process, vm_space, 0x2000_0000, 0x3000_0000, None)
        .expect("thread");

    assert_eq!(
        plane.create_scheduling_profile(SchedulingProfile::Deadline(DeadlineProfile {
            capacity_ns: 1,
            deadline_ns: 1,
            period_ns: 1,
        })),
        Err(KernelServiceStatus::AccessDenied)
    );

    let profile = plane
        .create_scheduling_profile(SchedulingProfile::Fair(FairProfile {
            priority: 240,
            weight: 10,
        }))
        .expect("clamped fair profile");
    assert_eq!(
        plane.set_thread_profile(thread, profile),
        KernelServiceStatus::Ok
    );
    let thread_id = plane.handle_record(thread).unwrap().object_id;
    assert_eq!(plane.threads.get(thread_id).unwrap().base_priority, 127);
    assert_eq!(plane.scheduler.task(thread_id).unwrap().base_priority, 127);
}

#[test]
fn direct_process_can_apply_deadline_profile() {
    let mut plane = ControlPlane::new();
    let (process, vm_space) = plane
        .create_process("audio", 4, "bexos.driver.audio", HardwareAccess::Direct)
        .expect("process and vm space");
    let thread = plane
        .start_thread_in_process(process, vm_space, 0x2000_0000, 0x3000_0000, None)
        .expect("thread");
    let profile = plane
        .create_scheduling_profile(SchedulingProfile::Deadline(DeadlineProfile {
            capacity_ns: 2,
            deadline_ns: 5,
            period_ns: 10,
        }))
        .expect("deadline profile");

    assert_eq!(
        plane.set_thread_profile(thread, profile),
        KernelServiceStatus::Ok
    );
    let thread_id = plane.handle_record(thread).unwrap().object_id;
    assert!(plane.scheduler.task(thread_id).unwrap().deadline.is_some());
}

#[test]
fn futex_priority_inheritance_boosts_owner_until_wake() {
    let mut plane = ControlPlane::new();
    let (process, vm_space) = plane
        .create_process("driver", 4, "bexos.driver.test", HardwareAccess::Direct)
        .expect("process and vm space");
    let owner = plane
        .start_thread_in_process(process, vm_space, 0x2000_0000, 0x3000_0000, None)
        .expect("owner");
    let waiter = plane
        .start_thread_in_process(process, vm_space, 0x2000_1000, 0x3000_1000, None)
        .expect("waiter");
    let owner_profile = plane
        .create_scheduling_profile(SchedulingProfile::Fair(FairProfile {
            priority: 10,
            weight: 1,
        }))
        .expect("owner profile");
    let waiter_profile = plane
        .create_scheduling_profile(SchedulingProfile::Fair(FairProfile {
            priority: 220,
            weight: 1,
        }))
        .expect("waiter profile");
    assert_eq!(
        plane.set_thread_profile(owner, owner_profile),
        KernelServiceStatus::Ok
    );
    assert_eq!(
        plane.set_thread_profile(waiter, waiter_profile),
        KernelServiceStatus::Ok
    );

    assert_eq!(plane.yield_thread(Some(waiter)), KernelServiceStatus::Ok);
    assert_eq!(
        plane.futex_wait(0x4000, 1, 1, Some(owner)),
        KernelServiceStatus::Ok
    );
    let owner_id = plane.handle_record(owner).unwrap().object_id;
    assert_eq!(plane.threads.get(owner_id).unwrap().effective_priority, 220);
    assert_eq!(
        plane.scheduler.task(owner_id).unwrap().effective_priority,
        220
    );

    let wake = plane.futex_wake(0x4000, 1);
    assert_eq!(wake.status, KernelServiceStatus::Ok);
    assert_eq!(plane.threads.get(owner_id).unwrap().effective_priority, 10);
    assert_eq!(
        plane.scheduler.task(owner_id).unwrap().effective_priority,
        10
    );
}

#[test]
fn control_plane_accepts_configured_cpu_affinity_only() {
    let mut plane = ControlPlane::with_cpu_count(4);
    let (process, vm_space) = plane
        .create_process("app", 1, "com.example.app", HardwareAccess::None)
        .expect("process and vm space");
    let thread = plane
        .start_thread_in_process(process, vm_space, 0x2000_0000, 0x3000_0000, None)
        .expect("thread");
    let thread_id = plane.handle_record(thread).unwrap().object_id;

    assert_eq!(
        plane.threads.get(thread_id).unwrap().cpu_affinity_mask,
        0b1111
    );
    assert_eq!(
        plane.scheduler.task(thread_id).unwrap().cpu_affinity_mask,
        0b1111
    );
    assert_eq!(
        plane.set_thread_cpu_affinity(thread, 0b0101),
        KernelServiceStatus::Ok
    );
    assert_eq!(
        plane.set_thread_cpu_affinity(thread, 0),
        KernelServiceStatus::InvalidArgs
    );
    assert_eq!(
        plane.set_thread_cpu_affinity(thread, 0b1_0000),
        KernelServiceStatus::InvalidArgs
    );
}

#[test]
fn control_plane_allocates_asids_when_supported_and_falls_back_to_zero() {
    let mut disabled = ControlPlane::new();
    let (_, disabled_space) = disabled
        .create_process("app", 1, "com.example.app", HardwareAccess::None)
        .expect("process");
    assert_eq!(
        disabled.vm_space_for_handle(disabled_space).unwrap().asid,
        0
    );

    let mut enabled = ControlPlane::with_cpu_count_and_asids(1, AsidSupport::Bits8);
    let (_, first_space) = enabled
        .create_process("first", 1, "com.example.first", HardwareAccess::None)
        .expect("first process");
    let (_, second_space) = enabled
        .create_process("second", 1, "com.example.second", HardwareAccess::None)
        .expect("second process");

    assert_eq!(enabled.vm_space_for_handle(first_space).unwrap().asid, 1);
    assert_eq!(enabled.vm_space_for_handle(second_space).unwrap().asid, 2);
}

#[test]
fn context_preserves_user_tls_thread_pointer() {
    let mut context = bexos_kernel_core::runtime::Context::zero();
    context.thread_pointer = 0xfeed_beef;
    context.set_initial_argument(7);

    assert_eq!(context.thread_pointer, 0xfeed_beef);
    assert_eq!(context.syscall_words()[0], 7);
}

#[test]
fn kernel_clock_is_monotonic_and_vdso_page_is_read_only() {
    let mut plane = ControlPlane::new();
    plane.set_monotonic_nanos_for_test(25);
    plane.set_monotonic_nanos_for_test(10);

    let time = plane.clock_get_time(ClockType::Monotonic);
    assert_eq!(time.status, KernelServiceStatus::Ok);
    assert_eq!(time.nanos, 25);
    assert_eq!(
        plane.clock_get_time(ClockType::BootTime).status,
        KernelServiceStatus::Ok
    );
    let vmo = plane.clock_get_vdso_time_page().expect("time page vmo");
    let record = plane.handle_record(vmo).expect("time page handle");
    assert_eq!(
        record.rights,
        RIGHT_READ | RIGHT_MAP | RIGHT_DUPLICATE | RIGHT_TRANSFER
    );
    assert_eq!(
        record.kind,
        bexos_kernel_core::kernel_services::handle::ObjectKind::Vmo
    );
}

#[test]
fn kernel_clock_adjusts_realtime_without_touching_monotonic() {
    let mut plane = ControlPlane::new();
    plane.set_monotonic_nanos_for_test(1_000);

    assert_eq!(
        plane.clock_adjust(ClockType::Realtime, 4_000, 0),
        KernelServiceStatus::Ok
    );
    assert_eq!(plane.clock_get_time(ClockType::Realtime).nanos, 5_000);
    assert_eq!(plane.clock_get_time(ClockType::Monotonic).nanos, 1_000);
    assert_eq!(
        plane.clock_adjust(ClockType::Monotonic, 1, 0),
        KernelServiceStatus::InvalidArgs
    );
    assert_eq!(
        plane.clock_adjust(ClockType::Realtime, 0, 501),
        KernelServiceStatus::InvalidArgs
    );
    assert_eq!(
        plane.clock_adjust(ClockType::Realtime, 1_000_000, 500),
        KernelServiceStatus::Ok
    );
    plane.set_monotonic_nanos_for_test(1_001_000);
    assert_eq!(plane.clock_get_time(ClockType::Realtime).nanos, 1_005_500);
    assert_eq!(
        plane.clock_adjust(ClockType::Realtime, 1, -500),
        KernelServiceStatus::InvalidArgs
    );
    assert_eq!(
        plane.clock_adjust(ClockType::Realtime, -1_000_000, -500),
        KernelServiceStatus::Ok
    );
    assert!(plane.clock_get_time(ClockType::Realtime).nanos >= 1_005_500);
}

#[test]
fn invalid_clock_type_fidl_decode_fails() {
    let mut bytes = [0u8; 8];
    bytes[0] = 99;

    assert_eq!(
        ClockGetTimeRequest::decode(&bytes, &[]),
        Err(FidlWireError::InvalidPresence)
    );
}

#[test]
fn system_privileged_tracks_process_interrupt_and_checkpoint_metadata() {
    let mut plane = ControlPlane::new();
    let (process, vm_space) = plane
        .create_process(
            "nvme",
            3,
            "bexos.driver.storage.nvme",
            HardwareAccess::Direct,
        )
        .expect("process and vm space");
    assert_ne!(process.raw, 0);
    assert_ne!(vm_space.raw, 0);
    let thread = plane
        .start_thread_in_process(process, vm_space, 0x2000_0000, 0x3000_0000, None)
        .expect("process-owned thread");
    assert_ne!(thread.raw, 0);

    let irq = plane.bind_interrupt(32, 0).expect("irq handle");
    assert_ne!(irq.raw, 0);
    assert_eq!(
        plane.bind_interrupt(32, 0),
        Err(KernelServiceStatus::AlreadyExists)
    );

    let vmo = plane
        .create_vmo(PAGE_SIZE as u64, 0)
        .expect("checkpoint vmo");
    let checkpoint = plane.checkpoint_system_state(vmo);
    assert_eq!(checkpoint.status, KernelServiceStatus::Ok);
    assert!(checkpoint.serialized_bytes > 0);
}

#[test]
fn kernel_debug_lists_process_records() {
    let mut plane = ControlPlane::new();
    let (process, vm_space) = plane
        .create_process("debugd", 4, "bexos.driver.debugd", HardwareAccess::Direct)
        .expect("debug process");
    let thread = plane
        .start_thread_in_process(process, vm_space, 0x2000_0000, 0x3000_0000, None)
        .expect("debug thread");
    assert_ne!(thread.raw, 0);

    let response = plane
        .list_processes(KernelDebugControlListProcessesRequest {})
        .expect("debug FIDL response");
    assert_eq!(response.status, Status::Ok);
    assert_eq!(response.processes.len(), 1);
    let process = response.processes.get(0).unwrap();
    assert_eq!(process.pid, 1);
    assert_eq!(process.main_thread_id, 1);
    assert_eq!(process.resource_group_id, 4);
    assert_eq!(process.state, kernel_fidl::DebugProcessState::Running);
    assert_eq!(&process.name[..process.name_len as usize], b"debugd");
    assert_eq!(
        &process.package_id[..process.package_id_len as usize],
        b"bexos.driver.debugd"
    );
}

#[test]
fn kernel_denies_hardware_vmos_without_direct_access() {
    let mut plane = ControlPlane::new();
    let (process, vm_space) = plane
        .create_process("app", 3, "com.example.app", HardwareAccess::None)
        .expect("process and vm space");
    plane
        .start_thread_in_process(process, vm_space, 0x2000_0000, 0x3000_0000, None)
        .expect("thread in process");

    assert_eq!(
        plane.create_vmo(PAGE_SIZE as u64, kernel_fidl::VmoFlags::CONTIGUOUS_PHYS.0),
        Err(KernelServiceStatus::AccessDenied)
    );
    assert_eq!(
        plane.create_vmo(PAGE_SIZE as u64, kernel_fidl::VmoFlags::CACHE_POLICY_UC.0),
        Err(KernelServiceStatus::AccessDenied)
    );
}

#[test]
fn kernel_allows_direct_hardware_vmos_and_interrupts() {
    let mut plane = ControlPlane::new();
    let (process, vm_space) = plane
        .create_process(
            "nvme",
            4,
            "bexos.driver.storage.nvme",
            HardwareAccess::Direct,
        )
        .expect("process and vm space");
    plane
        .start_thread_in_process(process, vm_space, 0x2000_0000, 0x3000_0000, None)
        .expect("thread in process");

    assert!(
        plane
            .create_vmo(
                PAGE_SIZE as u64,
                kernel_fidl::VmoFlags::CONTIGUOUS_PHYS.0 | kernel_fidl::VmoFlags::CACHE_POLICY_UC.0,
            )
            .is_ok()
    );
    assert!(plane.bind_interrupt(40, 0).is_ok());
}

#[test]
fn kernel_denies_interrupt_binding_without_direct_access() {
    let mut plane = ControlPlane::new();
    let (process, vm_space) = plane
        .create_process(
            "usb_wifi",
            4,
            "bexos.driver.net.usb",
            HardwareAccess::Isolated,
        )
        .expect("process and vm space");
    plane
        .start_thread_in_process(process, vm_space, 0x2000_0000, 0x3000_0000, None)
        .expect("thread in process");

    assert_eq!(
        plane.bind_interrupt(41, 0),
        Err(KernelServiceStatus::AccessDenied)
    );
}

#[test]
fn generated_kernel_fidl_adapters_return_design_statuses() {
    let mut plane = ControlPlane::new();
    let response = ChannelControlPublicServer::create_channel(
        &mut plane,
        ChannelControlCreateChannelRequest {},
    )
    .expect("adapter response");
    assert_eq!(response.status, Status::Ok);
    assert_ne!(response.local_endpoint.raw, 0);

    let vmo_response = VirtualMemoryPublicServer::create_vmo(
        &mut plane,
        VirtualMemoryCreateVmoRequest {
            size_bytes: PAGE_SIZE as u64,
            flags: VmoFlags(0),
        },
    )
    .expect("vmo adapter response");
    assert_eq!(vmo_response.status, Status::Ok);
    assert_ne!(vmo_response.vmo.raw, 0);

    let (_process, vm_space) = plane
        .create_process("fidl_app", 1, "com.example.fidl", HardwareAccess::None)
        .expect("fidl process");
    let root_vmar = plane
        .root_vmar_handle_for_vm_space(vm_space)
        .expect("root vmar");
    let sub_vmar_response = VirtualMemoryPublicServer::create_sub_vmar(
        &mut plane,
        VirtualMemoryCreateSubVmarRequest {
            parent_vmar: kernel_fidl::HandleRef { raw: root_vmar.raw },
            offset: 0x0400_0000,
            size_bytes: (PAGE_SIZE * 2) as u64,
            flags: VmarFlags(VMAR_FLAG_CAN_MAP_READ | VMAR_FLAG_CAN_MAP_SPECIFIC),
        },
    )
    .expect("sub-vmar adapter response");
    assert_eq!(sub_vmar_response.status, Status::Ok);
    assert_eq!(sub_vmar_response.base_address, USER_VMAR_BASE + 0x0400_0000);

    let map_vmar_response = VirtualMemoryPublicServer::map_vmo(
        &mut plane,
        VirtualMemoryMapVmoRequest {
            vmar: sub_vmar_response.sub_vmar,
            vmo: vmo_response.vmo,
            vmo_offset: 0,
            vmar_offset: 0,
            size_bytes: PAGE_SIZE as u64,
            flags: VmarFlags(VMAR_FLAG_CAN_MAP_READ),
        },
    )
    .expect("vmar map adapter response");
    assert_eq!(map_vmar_response.status, Status::Ok);
    assert_eq!(
        map_vmar_response.mapped_vaddr,
        sub_vmar_response.base_address
    );

    let destroy_vmar_response = VirtualMemoryPublicServer::destroy_vmar(
        &mut plane,
        VirtualMemoryDestroyVmarRequest {
            vmar: sub_vmar_response.sub_vmar,
        },
    )
    .expect("destroy-vmar adapter response");
    assert_eq!(destroy_vmar_response.status, Status::Ok);

    plane.set_monotonic_nanos_for_test(1234);
    let clock_response = ClockPublicServer::get_time(
        &mut plane,
        ClockGetTimeRequest {
            clock_type: ClockType::Monotonic,
        },
    )
    .expect("clock adapter response");
    assert_eq!(clock_response.status, Status::Ok);
    assert_eq!(clock_response.nanos, 1234);

    let vdso_response = ClockPublicServer::get_vdso_time_page(
        &mut plane,
        kernel_fidl::ClockGetVdsoTimePageRequest {},
    )
    .expect("vdso response");
    assert_eq!(vdso_response.status, Status::Ok);
    let vdso_record = plane
        .handle_record(bexos_kernel_core::kernel_services::Handle {
            raw: vdso_response.vmo.raw,
        })
        .expect("vdso handle");
    assert_eq!(
        vdso_record.rights,
        RIGHT_READ | RIGHT_MAP | RIGHT_DUPLICATE | RIGHT_TRANSFER
    );

    let power_response = SystemPrivilegedBexosSystemPrivilegedServer::request_system_power_state(
        &mut plane,
        kernel_fidl::SystemPrivilegedRequestSystemPowerStateRequest {
            state: kernel_fidl::SystemPowerState::SuspendToRam,
        },
    )
    .expect("power response");
    assert_eq!(power_response.status, Status::Ok);
    assert_eq!(plane.power_transitions().len(), 1);

    let hibernate_response =
        SystemPrivilegedBexosSystemPrivilegedServer::request_system_power_state(
            &mut plane,
            kernel_fidl::SystemPrivilegedRequestSystemPowerStateRequest {
                state: kernel_fidl::SystemPowerState::SuspendToDisk,
            },
        )
        .expect("hibernate response");
    assert_eq!(hibernate_response.status, Status::ErrInvalidArgs);
}

#[test]
fn generated_scheduler_fidl_adapters_apply_profile_affinity_yield_and_channel_policy() {
    let mut plane = ControlPlane::new();
    let (process, vm_space) = plane
        .create_process("app", 1, "com.example.app", HardwareAccess::None)
        .expect("process and vm space");
    let thread = plane
        .start_thread_in_process(process, vm_space, 0x2000_0000, 0x3000_0000, None)
        .expect("thread");
    let profile_response = ProfileProviderPublicServer::create_profile(
        &mut plane,
        ProfileProviderCreateProfileRequest {
            info: SchedulingProfileInfo::Fair(kernel_fidl::FairProfile {
                priority: 42,
                weight: 3,
            }),
        },
    )
    .expect("profile response");
    assert_eq!(profile_response.status, Status::Ok);

    let set_profile = TaskControlPublicServer::set_profile(
        &mut plane,
        TaskControlSetProfileRequest {
            thread: kernel_fidl::HandleRef { raw: thread.raw },
            profile: profile_response.profile_handle,
        },
    )
    .expect("set profile response");
    assert_eq!(set_profile.status, Status::Ok);

    let set_affinity = TaskControlPublicServer::set_cpu_affinity(
        &mut plane,
        TaskControlSetCpuAffinityRequest {
            thread: kernel_fidl::HandleRef { raw: thread.raw },
            affinity: CpuMask { mask: 1 },
        },
    )
    .expect("set affinity response");
    assert_eq!(set_affinity.status, Status::Ok);

    let yield_response = TaskControlPublicServer::yield_thread(
        &mut plane,
        TaskControlYieldThreadRequest {
            target_thread: kernel_fidl::HandleRef { raw: 0 },
        },
    )
    .expect("yield response");
    assert_eq!(yield_response.status, Status::Ok);

    let (local, _) = plane.create_channel().expect("channel pair");
    let policy = ChannelControlPublicServer::set_policy(
        &mut plane,
        ChannelControlSetPolicyRequest {
            channel: kernel_fidl::HandleRef { raw: local.raw },
            enable_priority_inheritance: true,
            enable_timeslice_donation: true,
        },
    )
    .expect("set policy response");
    assert_eq!(policy.status, Status::Ok);
}

#[test]
fn synchronous_channel_call_uses_single_use_reply_token() {
    let mut plane = ControlPlane::new();
    let (client, server) = plane.create_channel().expect("channel pair");
    assert_eq!(
        plane.call(client, b"request", &[], i64::MAX).status,
        KernelServiceStatus::TimedOut
    );
    let (request, token) = plane.read_call(server, 32, 0, i64::MAX);
    assert_eq!(request.status, KernelServiceStatus::Ok);
    assert_eq!(&plane.scratch.bytes[..request.bytes_len], b"request");
    assert_ne!(token.raw, 0);
    assert_eq!(
        plane.reply_call(token, b"reply", &[]),
        KernelServiceStatus::Ok
    );
    let reply = plane.call(client, &[], &[], i64::MAX);
    assert_eq!(reply.status, KernelServiceStatus::Ok);
    assert_eq!(&plane.scratch.bytes[..reply.bytes_len], b"reply");
    assert_eq!(
        plane.reply_call(token, b"again", &[]),
        KernelServiceStatus::InvalidHandle
    );
}

#[test]
fn synchronous_channel_call_zero_deadline_polls_without_enqueueing() {
    let mut plane = ControlPlane::new();
    let (client, server) = plane.create_channel().expect("channel pair");

    assert_eq!(
        plane.call(client, b"poll", &[], 0).status,
        KernelServiceStatus::TimedOut
    );
    let (request, token) = plane.read_call(server, 32, 0, 0);
    assert_eq!(request.status, KernelServiceStatus::TimedOut);
    assert_eq!(token.raw, 0);

    assert_eq!(
        plane.call(client, b"request", &[], i64::MAX).status,
        KernelServiceStatus::TimedOut
    );
    let (request, token) = plane.read_call(server, 32, 0, 0);
    assert_eq!(request.status, KernelServiceStatus::Ok);
    assert_eq!(&plane.scratch.bytes[..request.bytes_len], b"request");
    assert_ne!(token.raw, 0);
}

#[test]
fn synchronous_ipc_donates_and_unwinds_server_priority() {
    let mut plane = ControlPlane::new();
    let (caller_process, caller_space) = plane
        .create_process("caller", 1, "com.example.caller", HardwareAccess::None)
        .expect("caller process");
    let caller = plane
        .start_thread_in_process(caller_process, caller_space, 0x2000_0000, 0x3000_0000, None)
        .expect("caller thread");
    let caller_id = plane.handles.get(caller.raw).unwrap().object_id;
    let (server_process, server_space) = plane
        .create_process("server", 1, "com.example.server", HardwareAccess::None)
        .expect("server process");
    let server = plane
        .start_thread_in_process(server_process, server_space, 0x2100_0000, 0x3100_0000, None)
        .expect("server thread");
    let server_id = plane.handles.get(server.raw).unwrap().object_id;

    plane.threads.set_current(caller_id).unwrap();
    let caller_profile = plane
        .create_scheduling_profile(SchedulingProfile::Fair(FairProfile {
            priority: 90,
            weight: 1,
        }))
        .expect("caller profile");
    assert_eq!(
        plane.set_thread_profile(caller, caller_profile),
        KernelServiceStatus::Ok
    );
    plane.threads.set_current(server_id).unwrap();
    let server_profile = plane
        .create_scheduling_profile(SchedulingProfile::Fair(FairProfile {
            priority: 1,
            weight: 1,
        }))
        .expect("server profile");
    assert_eq!(
        plane.set_thread_profile(server, server_profile),
        KernelServiceStatus::Ok
    );

    let (local, remote) = plane.create_channel().expect("channel pair");
    assert_eq!(
        plane.set_channel_policy(
            local,
            bexos_kernel_core::kernel_services::ipc::ChannelPolicy {
                enable_priority_inheritance: true,
                enable_timeslice_donation: true,
            },
        ),
        KernelServiceStatus::Ok
    );
    plane.threads.set_current(caller_id).unwrap();
    assert_eq!(
        plane.call(local, b"request", &[], i64::MAX).status,
        KernelServiceStatus::TimedOut
    );

    plane.threads.set_current(server_id).unwrap();
    let (request, token) = plane.read_call(remote, 64, 0, i64::MAX);
    assert_eq!(request.status, KernelServiceStatus::Ok);
    assert_eq!(plane.threads.get(server_id).unwrap().effective_priority, 90);
    assert_eq!(
        plane.reply_call(token, b"reply", &[]),
        KernelServiceStatus::Ok
    );
    assert_eq!(plane.threads.get(server_id).unwrap().effective_priority, 1);

    plane.threads.set_current(caller_id).unwrap();
    let reply = plane.call(local, b"ignored", &[], i64::MAX);
    assert_eq!(reply.status, KernelServiceStatus::Ok);
    assert_eq!(&plane.scratch.bytes[..reply.bytes_len], b"reply");
}

fn valid_elf() -> Vec<u8> {
    let mut bytes = vec![0; 0x2200];
    bytes[0..4].copy_from_slice(b"\x7fELF");
    bytes[4] = 2;
    bytes[5] = 1;
    bytes[6] = 1;
    put_u16(&mut bytes, 16, 2);
    put_u16(&mut bytes, 18, 183);
    put_u32(&mut bytes, 20, 1);
    put_u64(&mut bytes, 24, 0x401000);
    put_u64(&mut bytes, 32, 64);
    put_u16(&mut bytes, 52, 64);
    put_u16(&mut bytes, 54, 56);
    put_u16(&mut bytes, 56, 1);

    put_u32(&mut bytes, 64, 1);
    put_u32(&mut bytes, 68, 0x5);
    put_u64(&mut bytes, 72, 0);
    put_u64(&mut bytes, 80, 0x400000);
    put_u64(&mut bytes, 88, 0x400000);
    put_u64(&mut bytes, 96, 0x1800);
    put_u64(&mut bytes, 104, 0x1800);
    put_u64(&mut bytes, 112, 0x1000);
    bytes
}

fn valid_elf_with_tls() -> Vec<u8> {
    let mut bytes = valid_elf();
    put_u16(&mut bytes, 56, 2);

    let offset = 64 + 56;
    put_u32(&mut bytes, offset, 7);
    put_u64(&mut bytes, offset + 8, 0x2100);
    put_u64(&mut bytes, offset + 16, 0x500000);
    put_u64(&mut bytes, offset + 24, 0x500000);
    put_u64(&mut bytes, offset + 32, 4);
    put_u64(&mut bytes, offset + 40, 16);
    put_u64(&mut bytes, offset + 48, 16);
    bytes[0x2100..0x2104].copy_from_slice(&[1, 2, 3, 4]);
    bytes
}

fn put_u16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn put_u64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

fn bootfs_image(entries: &[(&str, &[u8])]) -> Vec<u8> {
    const ALIGN: usize = 4096;
    let table_size = ENTRY_SIZE * entries.len();
    let payload_offset = align_up_usize(HEADER_SIZE + table_size, ALIGN);
    let mut image = Vec::new();
    image.extend_from_slice(MAGIC);
    image.extend_from_slice(&VERSION.to_le_bytes());
    image.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    image.extend_from_slice(&(table_size as u32).to_le_bytes());
    image.extend_from_slice(&(payload_offset as u32).to_le_bytes());

    let mut payload_cursor = payload_offset;
    let mut payloads = Vec::new();
    for (path, payload) in entries {
        let mut encoded = [0; 256];
        encoded[..path.len()].copy_from_slice(path.as_bytes());
        image.extend_from_slice(&encoded);
        image.extend_from_slice(&(payload_cursor as u64).to_le_bytes());
        image.extend_from_slice(&(payload.len() as u64).to_le_bytes());

        let mut stored = Vec::from(*payload);
        stored.resize(align_up_usize(stored.len(), ALIGN), 0);
        payload_cursor += stored.len();
        payloads.push(stored);
    }

    image.resize(payload_offset, 0);
    for payload in payloads {
        image.extend_from_slice(&payload);
    }
    image
}

const fn align_up_usize(value: usize, alignment: usize) -> usize {
    (value + alignment - 1) & !(alignment - 1)
}

#[test]
fn runtime_cpu_excludes_sleep_and_retains_exited_workers() {
    use bexos_kernel_core::sched::{BlockReason, Scheduler, SchedulerTask};
    let mut s = Scheduler::with_cpu_count(2).unwrap();
    s.account_runtime(100);
    let mut a = SchedulerTask::new(1, 1, 1, 1, "main").unwrap();
    a.cpu_affinity_mask = 1;
    let mut b = SchedulerTask::new(2, 1, 1, 1, "worker").unwrap();
    b.cpu_affinity_mask = 2;
    s.add_task(a).unwrap();
    s.add_task(b).unwrap();
    s.account_runtime(150);
    assert_eq!(s.runtime_stats(1), Some((50, 100)));
    s.block_current_on_cpu(0, BlockReason::Futex { uaddr: 16 })
        .unwrap();
    s.account_runtime(200);
    assert_eq!(s.runtime_stats(1), Some((50, 150)));
    s.exit_task(2, 0).unwrap();
    s.cleanup_exited();
    s.account_runtime(250);
    assert_eq!(s.runtime_stats(1), Some((50, 150)));
    s.wake_task(1).unwrap();
    s.tick_at_on_cpu(0, 250).unwrap();
    s.account_runtime(300);
    assert_eq!(s.runtime_stats(1), Some((100, 200)));
    s.account_runtime(299); // A stale clock cannot subtract CPU time.
    assert_eq!(s.runtime_stats(1), Some((100, 200)));
}

#[test]
fn syscall_epilogue_is_charged_to_outgoing_thread_after_logical_block() {
    use bexos_kernel_core::sched::{BlockReason, Scheduler, SchedulerTask};
    let mut s = Scheduler::new();
    s.account_runtime(0);
    for id in 1..=2 {
        s.add_task(SchedulerTask::new(id, id, 1, 1, "task").unwrap())
            .unwrap();
    }
    s.account_runtime(10);
    s.block_current(BlockReason::Futex { uaddr: 16 }).unwrap();
    assert_eq!(s.current().unwrap().id, 2);
    s.account_executing(0, 1, 15);
    assert_eq!(s.runtime_stats(1), Some((15, 15)));
    assert_eq!(s.runtime_stats(2), Some((0, 0)));
    s.account_runtime(20);
    assert_eq!(s.runtime_stats(2), Some((5, 5)));
}
