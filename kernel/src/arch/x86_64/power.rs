use kernel_fidl::{Status, SystemPowerState};
pub fn request_system_power_state(state: SystemPowerState) -> Status {
    match state {
        SystemPowerState::Active => Status::Ok,
        SystemPowerState::Poweroff => {
            unsafe {
                super::io::out16(0x604, 1 << 13);
            }
            Status::ErrInvalidArgs
        }
        SystemPowerState::Reboot => {
            unsafe {
                super::io::out8(0xcf9, 6);
            }
            Status::ErrInvalidArgs
        }
        _ => Status::ErrInvalidArgs,
    }
}
