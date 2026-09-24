use bexos_kernel_core::{
    cpu_features::KernelFeatureState,
    runtime::{AddressSpaceSwitch, Context},
};
use core::sync::atomic::{AtomicBool, Ordering};
mod acpi;
mod boot;
mod context;
mod cpu;
pub mod early_uart;
mod entry;
mod frame;
pub mod interrupts;
mod io;
mod ioapic;
pub mod mmu;
pub mod power;
mod time;
pub mod transplant;
pub struct X86_64;

static SECURE_MONITOR_ENABLED: AtomicBool = AtomicBool::new(false);
pub fn secure_monitor_ready() -> bool {
    SECURE_MONITOR_ENABLED.load(Ordering::Acquire)
}

impl super::ArchAPI for X86_64 {
    fn early_console() -> early_uart::EarlyUart {
        unsafe { early_uart::EarlyUart::new(early_uart::EARLY_BASE) }
    }
    fn quiesce_firmware_irqs() {
        interrupts::quiesce_firmware_irqs()
    }
    fn initialize_interrupts(cpus: u32) {
        interrupts::init(cpus)
    }
    fn initialize_secondary_interrupts() {
        interrupts::init_secondary()
    }
    fn program_scheduler_deadline(deadline: Option<u64>) {
        interrupts::program_scheduler_deadline(deadline)
    }
    fn request_reschedule(mask: u64) {
        interrupts::request_reschedule(mask)
    }
    fn read_user_readonly_thread_pointer() -> u64 {
        0
    }
    fn write_user_readonly_thread_pointer(_value: u64) {}
    fn request_system_power_state(state: kernel_fidl::SystemPowerState) -> kernel_fidl::Status {
        power::request_system_power_state(state)
    }
    fn initialize_mmu(root: u64) {
        mmu::init(root)
    }
    fn stage_replacement(
        rt: &mut crate::syscall::Rt,
        plan: &bexos_kernel_core::loader::LoadPlan,
    ) -> bexos_kernel_core::runtime::Result<()> {
        mmu::transplant::stage(rt, plan)
    }
    fn abort_replacement(rt: &mut crate::syscall::Rt) {
        mmu::transplant::abort(rt)
    }
    fn reclaim_old_kernel(
        rt: &mut crate::syscall::Rt,
        old: bexos_kernel_core::transplant::KernelRange,
    ) {
        mmu::transplant::reclaim_old_kernel(rt, old)
    }
    fn park_secondaries(rt: &crate::syscall::Rt) -> Result<(), &'static str> {
        transplant::park_secondaries(rt)
    }
    fn capture_system() -> bexos_kernel_core::transplant::Aarch64SystemRegisters {
        transplant::capture_system()
    }
    fn restore_system(registers: &bexos_kernel_core::transplant::Aarch64SystemRegisters) {
        transplant::restore_system(registers)
    }
    fn capture_cpu_context() -> bexos_kernel_core::transplant::Aarch64CpuContextRecord {
        transplant::capture_cpu_context()
    }
    fn capture_architecture_state() -> [u64; 16] {
        transplant::capture_architecture_state()
    }
    fn restore_architecture_state(state: &[u64; 16]) {
        transplant::restore_architecture_state(state)
    }

    fn matches_handoff_address_space(
        context: &bexos_kernel_core::transplant::Aarch64CpuContextRecord,
        root: u64,
        asid: u16,
    ) -> bool {
        let _ = asid;
        root == context.ttbr0_el1 & 0x000f_ffff_ffff_f000
    }

    fn cmos(register: u8, value: Option<u8>) -> Option<u8> {
        if !matches!(register, 0 | 2 | 4 | 7 | 8 | 9 | 0x0a | 0x0b | 0x0d | 0x32)
            || (value.is_some() && matches!(register, 0x0a | 0x0d))
        {
            return None;
        }
        unsafe {
            io::out8(0x70, register | 0x80);
            if let Some(value) = value {
                io::out8(0x71, value);
            }
            let read = io::in8(0x71);
            io::out8(0x70, register);
            Some(read)
        }
    }
    fn pci_ecam_base() -> u64 {
        cpu::pci_ecam_base()
    }
    fn pci_segment() -> u16 {
        cpu::pci_segment()
    }
    fn pci_bus_range() -> (u8, u8) {
        cpu::pci_bus_range()
    }
    fn pci_mmio_window() -> (u64, u64) {
        (0xc000_0000, 0xfebf_0000)
    }
    fn configure_interrupt(irq_number: u32, flags: u32) -> bool {
        ioapic::route(irq_number, cpu::apic_id(0), flags & 2 != 0, flags & 1 != 0)
    }
    fn mask_interrupt(irq_number: u32, masked: bool) -> bool {
        if masked {
            ioapic::mask(irq_number, true)
        } else if ioapic::pending(irq_number) {
            ioapic::acknowledge(irq_number)
        } else {
            ioapic::mask(irq_number, false)
        }
    }
    fn random_seed() -> Option<[u64; 4]> {
        None
    }
    fn initialize_primary(features: KernelFeatureState, seed: [u64; 4]) {
        cpu::initialize_primary(features, seed)
    }
    fn initialize_secondary(features: KernelFeatureState) {
        cpu::initialize_secondary(features)
    }
    fn initialize_smp(max_cpus: u32) {
        cpu::initialize_smp(max_cpus)
    }
    fn debug_break() {
        context::debug_break()
    }
    fn timer_deadline() -> u64 {
        time::timer_deadline()
    }
    fn restore_timer_deadline(deadline: u64) {
        time::set_timer_deadline_ns(deadline)
    }
    fn prepare_failure(context: &mut Context, entry: u64) {
        let frame = frame::TrapFrame::view_mut(context);
        frame.rip = entry;
        frame.rflags &= !(1 << 9);
    }
    fn set_timer_interval(ticks: u64) {
        time::set_timer_interval(ticks)
    }
    fn set_timer_deadline_ns(deadline_ns: u64) {
        time::set_timer_deadline_ns(deadline_ns)
    }
    fn disable_timer() {
        time::disable_timer()
    }
    fn timer_frequency() -> u64 {
        time::timer_frequency()
    }
    fn timer_ticks() -> u64 {
        time::timer_ticks()
    }
    fn monotonic_ns() -> u64 {
        time::monotonic_ns()
    }
    fn wait_for_interrupt() {
        context::wait_for_interrupt()
    }
    fn install_exception_vectors() {
        context::install_exception_vectors()
    }
    fn jump_to_replacement(entry: u64, handoff: u64) -> ! {
        context::jump_to_replacement(entry, handoff)
    }
    fn switch_address_space(address_space: AddressSpaceSwitch) {
        context::switch_address_space(address_space)
    }
    fn active_cpu_mask() -> u64 {
        cpu::active_cpu_mask()
    }
    fn current_cpu_id() -> u64 {
        cpu::current_cpu_id()
    }
    fn wait_for_cpu_boot_mask(expected_mask: u64) -> u64 {
        cpu::wait_for_cpu_boot_mask(expected_mask)
    }
    fn expected_cpu_mask(max_cpus: u32) -> u64 {
        cpu::expected_cpu_mask(max_cpus)
    }
    fn set_configured_max_cpus(max_cpus: u32) {
        cpu::set_configured_max_cpus(max_cpus)
    }
    fn configured_cpu_mask() -> u64 {
        cpu::configured_cpu_mask()
    }
    fn release_secondary_schedulers() {
        cpu::release_secondary_schedulers()
    }
    fn wait_for_scheduler_release() {
        cpu::wait_for_scheduler_release()
    }
    fn detect_cpu_features() -> KernelFeatureState {
        cpu::detect_cpu_features()
    }
    fn enable_user_access_protection(features: KernelFeatureState) {
        let _ = features;
        unsafe {
            let supported = core::arch::x86_64::__cpuid_count(7, 0).ebx;
            let mut cr4: u64;
            core::arch::asm!("mov {}, cr4", out(reg) cr4, options(nomem, nostack));
            if supported & (1 << 7) != 0 {
                cr4 |= 1 << 20;
            }
            if supported & (1 << 20) != 0 {
                cr4 |= 1 << 21;
            }
            core::arch::asm!("mov cr4, {}", in(reg) cr4, options(nostack));
        }
    }
    fn configure_secure_monitor(flags: u64, call_header: u64, features: u64) {
        let present = flags & bexos_boot::BOOT_EVIDENCE_FLAG_X86_SVM_MONITOR != 0
            && flags & bexos_boot::BOOT_EVIDENCE_FLAG_SECURE_BOOT != 0
            && flags & bexos_boot::BOOT_EVIDENCE_FLAG_RPMB_ANTI_ROLLBACK != 0
            && call_header == bexos_secure_monitor_abi::HEADER
            && features != 0;
        SECURE_MONITOR_ENABLED.store(present, Ordering::Release);
        if present {
            crate::log_line("kernel: x86 secure monitor hypercall transport enabled");
        } else {
            crate::log_line("kernel: x86 secure monitor hypercall transport unavailable");
        }
    }
    fn invoke_smc(regs: [u64; 8]) -> [u64; 8] {
        if !SECURE_MONITOR_ENABLED.load(Ordering::Acquire) {
            return [
                bexos_secure_monitor_abi::Status::AccessDenied as i64 as u64,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
            ];
        }
        unsafe { monitor_call(regs) }
    }
    unsafe fn enter_context(context: *const Context) -> ! {
        unsafe { context::enter_context(context) }
    }
    fn is_userspace(context: &Context) -> bool {
        context::is_userspace(context)
    }
    fn fault_address() -> u64 {
        context::fault_address()
    }
    fn synchronize_code(start: u64, len: u64) {
        context::synchronize_code(start, len)
    }
}

// All callers gate this instruction on the exact resident-monitor contract.
unsafe fn monitor_call(regs: [u64; 8]) -> [u64; 8] {
    let mut out = [0u64; 8];
    unsafe {
        core::arch::asm!(
            "push rbx",
            "mov rbx, {rbx_value}",
            "vmmcall",
            "mov qword ptr [{out_ptr} + 0], rax",
            "mov qword ptr [{out_ptr} + 8], rbx",
            "mov qword ptr [{out_ptr} + 16], rcx",
            "mov qword ptr [{out_ptr} + 24], rdx",
            "mov qword ptr [{out_ptr} + 32], rsi",
            "mov qword ptr [{out_ptr} + 40], rdi",
            "mov qword ptr [{out_ptr} + 48], r8",
            "mov qword ptr [{out_ptr} + 56], r9",
            "pop rbx",
            inlateout("rax") regs[0] => _,
            rbx_value = in(reg) regs[1],
            inlateout("rcx") regs[2] => _,
            inlateout("rdx") regs[3] => _,
            inlateout("rsi") regs[4] => _,
            inlateout("rdi") regs[5] => _,
            inlateout("r8") regs[6] => _,
            inlateout("r9") regs[7] => _,
            out_ptr = in(reg) out.as_mut_ptr(),
        );
    }
    out
}

pub fn confirm_boot_evidence(digest: [u8; 32]) -> bool {
    use bexos_secure_monitor_abi::{boot, transport};
    let discovery = unsafe { core::arch::x86_64::__cpuid_count(transport::DISCOVERY_LEAF, 0) };
    if discovery.eax != transport::DISCOVERY_MAGIC
        || discovery.ebx != 1
        || discovery.ecx != 2
        || discovery.edx & !transport::DISCOVERY_CAPABILITIES != 0
    {
        return false;
    }
    unsafe { monitor_call(boot::evidence_query(digest)) == [0; 8] }
}
