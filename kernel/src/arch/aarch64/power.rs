use kernel_fidl::{Status, SystemPowerState};

pub fn request_system_power_state(state: SystemPowerState) -> Status {
    match state {
        SystemPowerState::Active => Status::Ok,
        SystemPowerState::SuspendToDisk => Status::ErrInvalidArgs,
        SystemPowerState::SuspendToRam | SystemPowerState::Reboot | SystemPowerState::Poweroff => {
            let Some(operation) =
                crate::arch::aarch64::psci::power_operation_for_kernel_state(state)
            else {
                return Status::ErrInvalidArgs;
            };
            match crate::arch::aarch64::psci::request_power_operation(operation) {
                Ok(()) => Status::Ok,
                Err(error) => {
                    crate::arch::aarch64::psci::log_psci_error(
                        "kernel: PSCI power transition failed",
                        error,
                    );
                    Status::ErrInvalidArgs
                }
            }
        }
    }
}
