//! Compile-time hardware boundary for shared kernel policy.
use bexos_kernel_core::{
    cpu_features::KernelFeatureState,
    runtime::{AddressSpaceSwitch, Context},
};
pub trait ArchAPI {
    fn early_console() -> crate::early_uart::EarlyUart;
    fn quiesce_firmware_irqs();
    fn initialize_interrupts(cpus: u32);
    fn initialize_secondary_interrupts();
    fn program_scheduler_deadline(deadline: Option<u64>);
    fn request_system_power_state(state: kernel_fidl::SystemPowerState) -> kernel_fidl::Status;
    fn initialize_mmu(root: u64);
    fn stage_replacement(
        rt: &mut crate::syscall::Rt,
        plan: &bexos_kernel_core::loader::LoadPlan,
    ) -> bexos_kernel_core::runtime::Result<()>;
    fn abort_replacement(rt: &mut crate::syscall::Rt);
    fn reclaim_old_kernel(
        rt: &mut crate::syscall::Rt,
        old: bexos_kernel_core::transplant::KernelRange,
    );
    fn park_secondaries(rt: &crate::syscall::Rt) -> Result<(), &'static str>;
    fn capture_system() -> bexos_kernel_core::transplant::Aarch64SystemRegisters;
    fn restore_system(registers: &bexos_kernel_core::transplant::Aarch64SystemRegisters);
    fn capture_cpu_context() -> bexos_kernel_core::transplant::Aarch64CpuContextRecord;
    fn capture_architecture_state() -> [u64; 16];
    fn restore_architecture_state(state: &[u64; 16]);
    fn matches_handoff_address_space(
        context: &bexos_kernel_core::transplant::Aarch64CpuContextRecord,
        root: u64,
        asid: u16,
    ) -> bool;

    fn cmos(register: u8, value: Option<u8>) -> Option<u8> {
        let _ = (register, value);
        None
    }
    fn pci_ecam_base() -> u64;
    fn pci_segment() -> u16;
    fn pci_bus_range() -> (u8, u8);
    fn pci_mmio_window() -> (u64, u64);
    fn configure_interrupt(irq_number: u32, flags: u32) -> bool;
    fn mask_interrupt(irq_number: u32, masked: bool) -> bool;
    fn random_seed() -> Option<[u64; 4]>;
    fn initialize_primary(features: KernelFeatureState, seed: [u64; 4]);
    fn initialize_secondary(features: KernelFeatureState);
    fn initialize_smp(max_cpus: u32);
    fn debug_break();
    fn timer_deadline() -> u64;
    fn restore_timer_deadline(deadline: u64);
    fn prepare_failure(context: &mut Context, entry: u64);

    fn set_timer_interval(ticks: u64);
    fn set_timer_deadline_ns(deadline_ns: u64);
    fn disable_timer();
    fn timer_frequency() -> u64;
    fn timer_ticks() -> u64;
    fn monotonic_ns() -> u64;
    fn wait_for_interrupt();
    fn install_exception_vectors();
    fn jump_to_replacement(entry: u64, handoff: u64) -> !;
    fn switch_address_space(address_space: AddressSpaceSwitch);
    fn active_cpu_mask() -> u64;
    fn current_cpu_id() -> u64;
    fn wait_for_cpu_boot_mask(expected_mask: u64) -> u64;
    fn expected_cpu_mask(max_cpus: u32) -> u64;
    fn set_configured_max_cpus(max_cpus: u32);
    fn configured_cpu_mask() -> u64;
    fn release_secondary_schedulers();
    fn wait_for_scheduler_release();
    fn detect_cpu_features() -> KernelFeatureState;
    fn enable_user_access_protection(features: KernelFeatureState);
    fn configure_secure_monitor(flags: u64, call_header: u64, features: u64) {
        let _ = (flags, call_header, features);
    }
    fn invoke_smc(regs: [u64; 8]) -> [u64; 8];
    unsafe fn enter_context(context: *const Context) -> !;
    fn is_userspace(context: &Context) -> bool;
    fn fault_address() -> u64;
    fn synchronize_code(start: u64, len: u64);
}
