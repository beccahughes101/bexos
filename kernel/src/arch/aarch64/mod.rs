use bexos_kernel_core::cpu_features::{Aarch64CpuFeatures, KernelFeatureState};
use bexos_kernel_core::runtime::AddressSpaceSwitch;
use bexos_kernel_core::runtime::Context;
use core::arch::asm;
use core::sync::atomic::Ordering;
mod boot;
mod context;
mod cpu;
mod frame;
pub mod hardening;
pub mod psci;
mod time;
use boot::*;
pub use context::*;
pub use cpu::*;
use frame::TrapFrame;
pub use time::*;
pub struct Aarch64;
pub mod early_uart;
pub mod interrupts;
pub mod mmu;
pub mod power;

impl super::ArchAPI for Aarch64 {
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
        root == context.ttbr0_el1 & 0x0000_ffff_ffff_f000
            && u64::from(asid) == context.ttbr0_el1 >> 48
    }

    fn pci_ecam_base() -> u64 {
        0x3f00_0000
    }
    fn pci_segment() -> u16 {
        0
    }
    fn pci_bus_range() -> (u8, u8) {
        (0, 15)
    }
    fn pci_mmio_window() -> (u64, u64) {
        (0x1000_0000, 0x3eff_0000)
    }
    fn configure_interrupt(irq_number: u32, _flags: u32) -> bool {
        interrupts::configure_device_irq(irq_number)
    }
    fn mask_interrupt(irq_number: u32, masked: bool) -> bool {
        interrupts::mask_device_irq(irq_number, masked)
    }
    fn timer_deadline() -> u64 {
        let value;
        unsafe {
            asm!("mrs {}, cntv_cval_el0", out(reg) value, options(nomem, nostack));
        }
        value
    }
    fn restore_timer_deadline(value: u64) {
        unsafe {
            asm!("msr cntv_cval_el0, {}", in(reg) value, options(nomem, nostack));
        }
    }
    fn prepare_failure(context: &mut Context, entry: u64) {
        let frame = TrapFrame::view_mut(context);
        frame.elr = entry;
        frame.spsr |= 0x3c0;
    }

    fn random_seed() -> Option<[u64; 4]> {
        hardening::read_rndr_seed()
    }
    fn initialize_primary(features: KernelFeatureState, seed: [u64; 4]) {
        hardening::initialize_primary(features, seed);
    }
    fn initialize_secondary(features: KernelFeatureState) {
        hardening::initialize_secondary(features);
    }
    fn initialize_smp(max_cpus: u32) {
        psci::initialize(max_cpus);
    }
    fn debug_break() {
        unsafe {
            asm!("brk #0xbee", options(nostack));
        }
    }

    fn set_timer_interval(ticks: u64) {
        set_timer_interval(ticks)
    }
    fn set_timer_deadline_ns(deadline_ns: u64) {
        set_timer_deadline_ns(deadline_ns)
    }
    fn disable_timer() {
        disable_timer()
    }
    fn timer_frequency() -> u64 {
        timer_frequency()
    }
    fn timer_ticks() -> u64 {
        timer_ticks()
    }
    fn monotonic_ns() -> u64 {
        monotonic_ns()
    }
    fn wait_for_interrupt() {
        wait_for_interrupt()
    }
    fn install_exception_vectors() {
        install_exception_vectors()
    }
    fn jump_to_replacement(entry: u64, handoff: u64) -> ! {
        jump_to_replacement(entry, handoff)
    }
    fn switch_address_space(address_space: AddressSpaceSwitch) {
        switch_address_space(address_space)
    }
    fn active_cpu_mask() -> u64 {
        active_cpu_mask()
    }
    fn current_cpu_id() -> u64 {
        current_cpu_id()
    }
    fn wait_for_cpu_boot_mask(expected_mask: u64) -> u64 {
        wait_for_cpu_boot_mask(expected_mask)
    }
    fn expected_cpu_mask(max_cpus: u32) -> u64 {
        expected_cpu_mask(max_cpus)
    }
    fn set_configured_max_cpus(max_cpus: u32) {
        set_configured_max_cpus(max_cpus)
    }
    fn configured_cpu_mask() -> u64 {
        configured_cpu_mask()
    }
    fn release_secondary_schedulers() {
        release_secondary_schedulers()
    }
    fn wait_for_scheduler_release() {
        wait_for_scheduler_release()
    }
    fn detect_cpu_features() -> KernelFeatureState {
        detect_cpu_features()
    }
    fn enable_user_access_protection(features: KernelFeatureState) {
        enable_user_access_protection(features)
    }
    fn invoke_smc(regs: [u64; 8]) -> [u64; 8] {
        invoke_smc(regs)
    }
    unsafe fn enter_context(context: *const Context) -> ! {
        unsafe { boot::enter_context(context) }
    }
    fn is_userspace(context: &Context) -> bool {
        TrapFrame::view(context).spsr & 15 == 0
    }
    fn fault_address() -> u64 {
        let value;
        unsafe {
            asm!("mrs {}, far_el1", out(reg) value, options(nomem, nostack));
        }
        value
    }
    fn synchronize_code(start: u64, len: u64) {
        for address in (start & !63..start + len).step_by(64) {
            unsafe {
                asm!("dc cvau, {}", in(reg) address, options(nostack));
            }
        }
        unsafe {
            asm!("dsb ish; ic iallu; dsb ish; isb", options(nostack));
        }
    }
}

pub mod transplant;
