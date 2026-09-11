use crate::arch::ArchAPI;
use crate::mmu::PhysicalBackend;
use crate::state::Global;
use alloc::boxed::Box;
use alloc::vec::Vec;
use bexos_boot::*;
use bexos_kernel_core::bootfs::Bootfs;
use bexos_kernel_core::cpu_features::KernelFeatureState;
use bexos_kernel_core::loader::{LoadPlan, PAGE_SIZE, RIGHTS_READ, RIGHTS_WRITE};
use bexos_kernel_core::runtime::{Backend, Runtime};
use bootstrap_fidl::FidlEncode;
pub static RUNTIME: Global<Option<Box<Runtime<PhysicalBackend>>>> = Global::new(None);
const INITIAL_TLS_BASE: u64 = 0xbe00_0000;
pub fn init(
    first_free: u64,
    features: KernelFeatureState,
    handoff: BootHandoff,
    pac_seed: Option<[u64; 4]>,
) {
    crate::log_line("kernel: userspace bootfs parse begin");
    let bytes = unsafe {
        core::slice::from_raw_parts(
            handoff.bootfs_addr as *const u8,
            handoff.bootfs_len as usize,
        )
    };
    let bootfs = Bootfs::parse(bytes).expect("external BootFS");
    crate::log_line("kernel: userspace bootfs parsed");
    let image = bootfs
        .find(APPD_PATH)
        .expect("BootFS lookup")
        .expect("missing appd");
    crate::log_line("kernel: userspace appd image found");
    let plan =
        LoadPlan::parse(image.bytes, bexos_elf::arch::Machine::current_guest()).expect("appd ELF");
    crate::log_line("kernel: userspace appd load plan parsed");
    crate::log_line("kernel: userspace physical frame map begin");
    let mut rt = Box::new(Runtime::with_cpu_count(
        PhysicalBackend {
            frames: crate::memory::Frames::new(first_free, &handoff),
        },
        handoff.effective_max_cpus() as u32,
        features.detected.asid,
    ));
    crate::log_line("kernel: userspace runtime created");
    rt.set_boot_entropy_seed(pac_seed);
    let (process, space) = rt
        .create_process("appd", "bexos.platform.appd", 0)
        .expect("initial process");
    let machine = bexos_elf::arch::Machine::current_guest();
    let initial_tls = plan.tls.map(|tls| bexos_elf::TlsSegment {
        file_offset: tls.file_offset,
        file_size: tls.file_size,
        mem_size: tls.mem_size,
        align: tls.align,
    });
    let mut tls_layout = bexos_elf::tls::layout(machine, [(image.bytes, initial_tls)])
        .expect("bootstrap TLS layout");
    let linked = bexos_elf::link_executable(image.bytes, machine, tls_layout.modules[0], &[], &[])
        .expect("bootstrap ELF linking");
    if let Some(linked) = &linked {
        tls_layout
            .install_relocated(0, linked)
            .expect("bootstrap relocated TLS");
        for segment in &linked.segments {
            map_initial_segment(
                &mut rt,
                space,
                segment.vaddr,
                &segment.bytes,
                segment.rights,
            );
        }
    } else {
        for segment in &plan.segments {
            if segment.file_size > 0 {
                let start = segment.file_offset as usize;
                let end = start + segment.file_size as usize;
                map_initial_segment(
                    &mut rt,
                    space,
                    segment.vaddr,
                    &image.bytes[start..end],
                    segment.rights,
                );
            }
            if let Some(bss) = segment.zero_fill {
                let bss_map_size = page_round_up(bss.size_bytes);
                let h = rt.create_vmo(bss_map_size, 0).unwrap();
                rt.map(
                    Some(space),
                    h,
                    0,
                    bss_map_size,
                    bss.vaddr,
                    segment.rights & !bexos_elf::load::RIGHTS_EXECUTE,
                )
                .unwrap();
                rt.close(h).unwrap();
            }
        }
    }
    let thread_pointer = if tls_layout.mem_size != 0 {
        map_initial_tls(&mut rt, space, &tls_layout)
    } else {
        0
    };
    install_process_memory(&mut rt, space);
    let (local, remote) = rt.create_channel();
    let boot_vmo_len = page_round_up(handoff.bootfs_len);
    let evidence_vmo_len = page_round_up(handoff.boot_evidence_len);
    let boot = rt.import_vmo(
        handoff.bootfs_addr,
        boot_vmo_len,
        false,
        true,
        2 | 16 | 1 | 32,
    );
    let evidence = rt.import_vmo(
        handoff.boot_evidence_addr,
        evidence_vmo_len,
        false,
        false,
        1 | 2 | 16 | 32,
    );
    let mut refs = alloc::vec![
        bootstrap_fidl::HandleRef { raw: boot },
        bootstrap_fidl::HandleRef { raw: evidence },
    ];
    if handoff.framebuffer.valid() {
        let f = handoff.framebuffer;
        let framebuffer = rt.import_vmo(f.address, f.length, true, false, 1 | 2 | 4 | 16 | 32);
        let descriptor = rt.create_vmo(4096, 0).expect("framebuffer descriptor");
        let va = scratch_vaddr(4096);
        rt.map(Some(space), descriptor, 0, 4096, va, 6)
            .expect("framebuffer descriptor map");
        let descriptor_bytes: Vec<u8> = f
            .words()
            .iter()
            .flat_map(|word| word.to_le_bytes())
            .collect();
        rt.copy_to_user(va, &descriptor_bytes)
            .expect("framebuffer descriptor write");
        rt.unmap(Some(space), va, 4096)
            .expect("framebuffer descriptor unmap");
        refs.push(bootstrap_fidl::HandleRef { raw: framebuffer });
        refs.push(bootstrap_fidl::HandleRef { raw: descriptor });
    }
    let linker_bytes = bexos_elf::runtime::encode_linker_data_v3(
        &[],
        linked
            .as_ref()
            .map_or(&[], |image| image.constructors.as_slice()),
        &tls_layout.template[..tls_layout.mem_size as usize],
        tls_layout.mem_size,
        tls_layout.align,
        bexos_boot::ARCHITECTURE_ID,
        &tls_layout
            .modules
            .iter()
            .map(|module| bexos_elf::runtime::TlsModule {
                thread_offset: module.thread_offset,
                mem_size: module.mem_size,
            })
            .collect::<Vec<_>>(),
    )
    .expect("bootstrap linker metadata");
    let metadata_size = page_round_up(linker_bytes.len() as u64);
    let metadata_vmo = rt
        .create_vmo(metadata_size, 0)
        .expect("bootstrap metadata VMO");
    let scratch = scratch_vaddr(metadata_size);
    rt.map(
        Some(space),
        metadata_vmo,
        0,
        metadata_size,
        scratch,
        RIGHTS_READ | RIGHTS_WRITE,
    )
    .unwrap();
    rt.copy_to_user(scratch, &linker_bytes).unwrap();
    rt.unmap(Some(space), scratch, metadata_size).unwrap();
    let linker_refs = [bootstrap_fidl::HandleRef { raw: metadata_vmo }];
    let namespace: [bootstrap_fidl::NamespaceEntry<'_>; 0] = [];
    let startup = bootstrap_fidl::Startup {
        version: 9,
        resources: &refs,
        arg0: handoff.bootfs_len,
        arg1: handoff.boot_evidence_len,
        namespace: bootstrap_fidl::WireVector::from_slice(&namespace),
        namespace_paths: "",
        migration: &[],
        migration_generation: 0,
        migration_target: false,
        service_grant_endpoints: &[],
        service_grant_descriptors: "",
        incoming_service_endpoints: &[],
        incoming_service_descriptors: "",
        lazy_idle_timeout_ms: 0,
        lazy_generation: 0,
        config: &[],
        config_len: 0,
        config_endpoint: &[],
        linker_data: &linker_refs,
        linker_data_len: linker_bytes.len() as u64,
        trace_producer: &[],
        trace_mapped_len: 0,
        trace_producer_id: 0,
        trace_pid: 0,
        trace_main_tid: 0,
        driver_resources: bootstrap_fidl::WireVector::from_slice(&[]),
        driver_lifecycle: &[],
    };
    let mut payload = [0; 1024];
    let mut handles = [bootstrap_fidl::HandleRef { raw: 0 }; 4];
    let e = startup.encode(&mut payload, &mut handles).unwrap();
    rt.write_message(
        local,
        &payload[..e.bytes],
        &handles[..e.handles]
            .iter()
            .map(|h| h.raw)
            .collect::<Vec<_>>(),
    )
    .unwrap();
    rt.start_with_thread_pointer(
        process,
        space,
        plan.entry_vaddr,
        USER_STACK_TOP,
        thread_pointer,
        remote,
    )
    .unwrap();
    rt.close(local).unwrap();
    let root = rt.processes[0].root;
    crate::log_line("kernel: external bootfs ready; appd loaded");
    unsafe {
        RUNTIME.replace_unlocked(Some(rt));
    }
    crate::log_line("kernel: initial runtime stored");
    crate::arch::CurrentArch::initialize_mmu(root);
}

fn map_initial_segment(
    rt: &mut Runtime<PhysicalBackend>,
    space: u64,
    vaddr: u64,
    bytes: &[u8],
    rights: u32,
) {
    let size = page_round_up(bytes.len() as u64);
    let handle = rt.create_vmo(size, 0).expect("bootstrap segment VMO");
    let scratch = scratch_vaddr(size);
    rt.map(
        Some(space),
        handle,
        0,
        size,
        scratch,
        RIGHTS_READ | RIGHTS_WRITE,
    )
    .expect("bootstrap scratch mapping");
    rt.copy_to_user(scratch, bytes)
        .expect("bootstrap segment copy");
    rt.unmap(Some(space), scratch, size).unwrap();
    rt.map(Some(space), handle, 0, size, vaddr, rights)
        .expect("bootstrap segment mapping");
    rt.close(handle).unwrap();
}

fn page_round_up(size: u64) -> u64 {
    (size + PAGE_SIZE - 1) & !(PAGE_SIZE - 1)
}

fn map_initial_tls(
    rt: &mut Runtime<PhysicalBackend>,
    space: u64,
    layout: &bexos_elf::tls::TlsLayout,
) -> u64 {
    let machine = bexos_elf::arch::Machine::current_guest();
    let vaddr = align_up(INITIAL_TLS_BASE, layout.align);
    let (bytes, thread_pointer) =
        bexos_elf::tls::initialize(&layout, machine, vaddr).expect("initial TLS image");
    let size = bytes.len() as u64;
    let h = rt.create_vmo(size, 0).expect("TLS VMO");
    let scratch = scratch_vaddr(size);
    rt.map(Some(space), h, 0, size, scratch, RIGHTS_READ | RIGHTS_WRITE)
        .expect("TLS scratch mapping");
    rt.copy_to_user(scratch, &bytes).expect("TLS image copy");
    rt.unmap(Some(space), scratch, size).unwrap();
    rt.map(Some(space), h, 0, size, vaddr, RIGHTS_READ | RIGHTS_WRITE)
        .expect("TLS mapping");
    rt.close(h).unwrap();
    thread_pointer
}

fn page_round(value: u64) -> Option<u64> {
    value
        .checked_add(PAGE_SIZE - 1)
        .map(|n| n & !(PAGE_SIZE - 1))
}

fn align_up(value: u64, align: u64) -> u64 {
    if align <= 1 {
        return value;
    }
    (value + align - 1) & !(align - 1)
}

fn scratch_vaddr(size: u64) -> u64 {
    const SCRATCH_GUARD: u64 = 16 * 1024 * 1024;
    (bexos_boot::USER_END - SCRATCH_GUARD - size) & !(PAGE_SIZE - 1)
}

fn future_deadline(deadline: Option<u64>) -> Option<u64> {
    let now = crate::arch::CurrentArch::monotonic_ns();
    deadline.filter(|deadline| *deadline > now.saturating_add(1_000_000))
}

pub fn install_process_memory(rt: &mut Runtime<PhysicalBackend>, space: u64) {
    for (va, size) in [(USER_STACK_TOP - USER_STACK_SIZE, USER_STACK_SIZE)] {
        let h = rt.create_vmo(size, 0).expect("process memory");
        rt.map(Some(space), h, 0, size, va, 6)
            .expect("process memory mapping");
        rt.close(h).unwrap();
    }
}
pub fn launch() -> ! {
    crate::log_line("kernel: userspace launch begin");
    let (context, address_space, deadline) = RUNTIME.with(|s| {
        let rt = s.as_mut().unwrap();
        let _ = rt.bind_current_cpu(0);
        (
            rt.threads[rt.current_thread].context,
            bexos_kernel_core::runtime::AddressSpaceSwitch {
                root_table_phys: rt.processes[rt.current].root,
                asid: rt.processes[rt.current].asid,
                userspace_pac_key: rt.processes[rt.current].userspace_pac_key,
            },
            rt.next_deadline_on_cpu(0),
        )
    });
    crate::log_line("kernel: userspace launch state ready");
    crate::arch::CurrentArch::switch_address_space(address_space);
    crate::log_line("kernel: userspace address space selected");
    crate::arch::CurrentArch::program_scheduler_deadline(future_deadline(deadline));
    crate::log_line("kernel: userspace scheduler deadline programmed");
    #[cfg(target_arch = "aarch64")]
    crate::log_line("userspace: entering el0 appd");
    #[cfg(target_arch = "x86_64")]
    crate::log_line("userspace: entering ring3 appd");
    unsafe { crate::arch::CurrentArch::enter_context(&context) }
}
