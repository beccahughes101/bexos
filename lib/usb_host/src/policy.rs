use crate::descriptor::{Configuration, InterfaceDescriptor};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InterfaceClass {
    Hub,
    BootKeyboard,
    BootMouse,
    MassStorageBulkOnly,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PolicyDecision {
    Admit(InterfaceClass),
    Reject,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClassPolicy {
    pub hubs: bool,
    pub boot_hid: bool,
    pub bulk_storage: bool,
}

impl Default for ClassPolicy {
    fn default() -> Self {
        Self {
            hubs: true,
            boot_hid: true,
            bulk_storage: true,
        }
    }
}

impl ClassPolicy {
    pub fn decide_interface(self, interface: &InterfaceDescriptor) -> PolicyDecision {
        match (interface.class_code, interface.subclass, interface.protocol) {
            (0x09, _, _) if self.hubs => PolicyDecision::Admit(InterfaceClass::Hub),
            (0x03, 0x01, 0x01) if self.boot_hid => {
                PolicyDecision::Admit(InterfaceClass::BootKeyboard)
            }
            (0x03, 0x01, 0x02) if self.boot_hid => PolicyDecision::Admit(InterfaceClass::BootMouse),
            (0x08, 0x06, 0x50) if self.bulk_storage => {
                PolicyDecision::Admit(InterfaceClass::MassStorageBulkOnly)
            }
            _ => PolicyDecision::Reject,
        }
    }

    pub fn admits_composite(self, configuration: &Configuration) -> bool {
        !configuration.interfaces.is_empty()
            && configuration.interfaces.iter().all(|interface| {
                matches!(self.decide_interface(interface), PolicyDecision::Admit(_))
            })
    }
}
