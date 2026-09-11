use super::{ControlPlane, KernelServiceStatus};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SystemPowerState {
    Active,
    SuspendToRam,
    SuspendToDisk,
    Reboot,
    Poweroff,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PowerTransitionRecord {
    pub requested: SystemPowerState,
    pub accepted: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PowerState {
    transitions: alloc::vec::Vec<PowerTransitionRecord>,
}

impl PowerState {
    pub const fn new() -> Self {
        Self {
            transitions: alloc::vec::Vec::new(),
        }
    }

    pub fn transitions(&self) -> &[PowerTransitionRecord] {
        &self.transitions
    }
}

impl ControlPlane {
    pub fn request_system_power_state(&mut self, state: SystemPowerState) -> KernelServiceStatus {
        if state == SystemPowerState::SuspendToDisk {
            return KernelServiceStatus::InvalidArgs;
        }
        if self.power.transitions.try_reserve(1).is_err() {
            return KernelServiceStatus::NoMemory;
        }
        self.power.transitions.push(PowerTransitionRecord {
            requested: state,
            accepted: true,
        });
        KernelServiceStatus::Ok
    }

    pub fn power_transitions(&self) -> &[PowerTransitionRecord] {
        self.power.transitions()
    }
}
