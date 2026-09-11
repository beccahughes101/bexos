use bexos_boot::HANDOFF_ADDR;
use bexos_kernel_core::psci::{
    PsciError, PsciFunction, PsciVersion, STANDBY_POWER_STATE, SystemPowerOperation,
    decode_feature_status, decode_status,
};
use core::fmt::Write;
use core::sync::atomic::{AtomicU64, Ordering};

const FEATURE_CPU_SUSPEND: u64 = 1 << 0;
const FEATURE_CPU_ON: u64 = 1 << 1;
const FEATURE_SYSTEM_OFF: u64 = 1 << 2;
const FEATURE_SYSTEM_RESET: u64 = 1 << 3;

static FEATURES: AtomicU64 = AtomicU64::new(0);

unsafe extern "C" {
    fn _start() -> !;
}

pub fn initialize(max_cpus: u32) {
    let version = match PsciVersion::decode(invoke(PsciFunction::Version, [0; 6])[0]) {
        Ok(version) => version,
        Err(error) => {
            log_psci_error("kernel: PSCI version probe failed", error);
            return;
        }
    };
    log_psci_version(version);

    let mut features = 0;
    features |= probe_feature(PsciFunction::CpuSuspend64, FEATURE_CPU_SUSPEND);
    features |= probe_feature(PsciFunction::CpuOn64, FEATURE_CPU_ON);
    features |= probe_feature(PsciFunction::SystemOff, FEATURE_SYSTEM_OFF);
    features |= probe_feature(PsciFunction::SystemReset, FEATURE_SYSTEM_RESET);
    FEATURES.store(features, Ordering::Release);

    if features & FEATURE_CPU_ON == 0 {
        crate::log_line("kernel: PSCI CPU_ON unavailable; continuing with already booted CPUs");
        return;
    }
    start_secondary_cpus(max_cpus);
}

pub fn request_power_operation(operation: SystemPowerOperation) -> Result<(), PsciError> {
    match operation {
        SystemPowerOperation::Standby => cpu_standby(),
        SystemPowerOperation::Reboot => terminal_call(
            PsciFunction::SystemReset,
            FEATURE_SYSTEM_RESET,
            "kernel: PSCI SYSTEM_RESET",
        ),
        SystemPowerOperation::Poweroff => terminal_call(
            PsciFunction::SystemOff,
            FEATURE_SYSTEM_OFF,
            "kernel: PSCI SYSTEM_OFF",
        ),
    }
}

fn cpu_standby() -> Result<(), PsciError> {
    if FEATURES.load(Ordering::Acquire) & FEATURE_CPU_SUSPEND == 0 {
        return Err(PsciError::NotSupported);
    }
    crate::log_line("kernel: PSCI CPU_SUSPEND standby enter");
    let ret = invoke(
        PsciFunction::CpuSuspend64,
        [STANDBY_POWER_STATE, 0, 0, 0, 0, 0],
    )[0];
    let status = decode_status(ret);
    if status.is_ok() {
        crate::log_line("kernel: PSCI CPU_SUSPEND standby returned");
    }
    status
}

fn terminal_call(
    function: PsciFunction,
    feature: u64,
    message: &'static str,
) -> Result<(), PsciError> {
    if FEATURES.load(Ordering::Acquire) & feature == 0 {
        return Err(PsciError::NotSupported);
    }
    crate::log_line(message);
    let ret = invoke(function, [0; 6])[0];
    match decode_status(ret) {
        Ok(()) => Err(PsciError::UnexpectedReturn),
        Err(error) => Err(error),
    }
}

fn start_secondary_cpus(max_cpus: u32) {
    let entry = _start as *const () as usize as u64;
    for cpu in 1..max_cpus.min(64) {
        let ret = invoke(
            PsciFunction::CpuOn64,
            [u64::from(cpu), entry, HANDOFF_ADDR, 0, 0, 0],
        )[0];
        if let Err(error) = decode_status(ret) {
            log_cpu_on_error(cpu, error);
        }
    }
}

fn probe_feature(function: PsciFunction, bit: u64) -> u64 {
    let ret = invoke(PsciFunction::Features, [function.id(), 0, 0, 0, 0, 0])[0];
    match decode_feature_status(ret) {
        Ok(()) => bit,
        Err(PsciError::NotSupported) => 0,
        Err(error) => {
            log_psci_error("kernel: PSCI feature probe failed", error);
            0
        }
    }
}

fn invoke(function: PsciFunction, args: [u64; 6]) -> [u64; 4] {
    let regs = super::invoke_smc([
        function.id(),
        args[0],
        args[1],
        args[2],
        args[3],
        args[4],
        args[5],
        0,
    ]);
    [regs[0], regs[1], regs[2], regs[3]]
}

fn log_psci_version(version: PsciVersion) {
    crate::state::UART.with(|s| {
        if let Some(u) = s.as_mut() {
            let _ = writeln!(
                u,
                "kernel: PSCI v{}.{} detected",
                version.major, version.minor
            );
        }
    });
}

fn log_cpu_on_error(cpu: u32, error: PsciError) {
    crate::state::UART.with(|s| {
        if let Some(u) = s.as_mut() {
            let _ = writeln!(u, "kernel: PSCI CPU_ON cpu{cpu} failed: {error:?}");
        }
    });
}

pub fn log_psci_error(message: &'static str, error: PsciError) {
    crate::state::UART.with(|s| {
        if let Some(u) = s.as_mut() {
            let _ = writeln!(u, "{message}: {error:?}");
        }
    });
}

pub fn power_operation_for_kernel_state(
    state: kernel_fidl::SystemPowerState,
) -> Option<SystemPowerOperation> {
    match state {
        kernel_fidl::SystemPowerState::SuspendToRam => Some(SystemPowerOperation::Standby),
        kernel_fidl::SystemPowerState::Reboot => Some(SystemPowerOperation::Reboot),
        kernel_fidl::SystemPowerState::Poweroff => Some(SystemPowerOperation::Poweroff),
        kernel_fidl::SystemPowerState::Active | kernel_fidl::SystemPowerState::SuspendToDisk => {
            None
        }
    }
}
