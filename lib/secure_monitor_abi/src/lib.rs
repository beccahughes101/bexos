#![no_std]

//! Register-only monitor transport. Trusty IPC payloads remain provider-owned.
//! x86 registers are RAX, RBX, RCX, RDX, RSI, RDI, R8, R9 in that order.
//! No Rust enum, pointer, or native struct layout crosses this boundary.

pub mod boot;
pub mod firmware;
pub mod transport;

pub const VERSION: u16 = 1;
pub const ARCH_X86_64: u16 = 2;
pub const HEADER: u64 = (0x4245_584du64 << 32) | ((VERSION as u64) << 16) | ARCH_X86_64 as u64;
/// AArch64 SMCCC envelope used to carry the same register protocol through
/// TF-A's Trusty dispatcher. The permanent S-EL2 owner restores HEADER before
/// decoding, so the public protocol remains architecture-neutral.
pub const AARCH64_FIRMWARE_FID: u64 = 0xfb00_be00;
pub const REGISTER: u64 = 1;
pub const UNREGISTER: u64 = 2;
pub const STAGE_TRUSTY_CORE: u64 = 3;
pub const ACTIVATE_TRUSTY_CORE: u64 = 4;
/// Kernel-only registration, bound to a live physical pin. Userspace submits
/// REGISTER; the kernel validates ownership and supplies the pin identity.
pub const REGISTER_PINNED: u64 = 5;
pub const PINNED_HANDLE_TAG: u64 = 1 << 63;
pub const MAX_SHARED_BYTES: u64 = 64 * 1024;
pub const PAGE_SIZE: u64 = 4096;
pub const SHARED_READ: u64 = 1;
pub const SHARED_WRITE: u64 = 2;
pub const ACTIVATE_LIVE_NOW: u64 = 1;
pub const ACTIVATE_ON_REBOOT: u64 = 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i64)]
pub enum Status {
    Ok = 0,
    InvalidArgs = -1,
    AccessDenied = -2,
    Unsupported = -3,
    NoResources = -4,
    InvalidHandle = -5,
    Busy = -6,
}

impl Status {
    pub const fn registers(self, value: u64) -> [u64; 8] {
        [self as i64 as u64, value, 0, 0, 0, 0, 0, 0]
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Request {
    Register {
        address: u64,
        length: u64,
        access: u64,
    },
    RegisterPinned {
        token: u64,
        address: u64,
        length: u64,
        access: u64,
    },
    Unregister {
        handle: u64,
    },
    StageTrustyCore {
        handle: u64,
        length: u64,
        generation: u64,
    },
    ActivateTrustyCore {
        generation: u64,
        activation: u64,
    },
}

impl Request {
    pub fn decode(registers: [u64; 8]) -> Result<Self, Status> {
        if registers[0] != HEADER {
            return Err(Status::Unsupported);
        }
        match registers[1] {
            REGISTER_PINNED if registers[6..].iter().all(|value| *value == 0) => {
                let [_, _, token, address, length, access, ..] = registers;
                if token == 0 || token >= PINNED_HANDLE_TAG - 1 {
                    return Err(Status::InvalidArgs);
                }
                Self::decode([HEADER, REGISTER, address, length, access, 0, 0, 0])?;
                Ok(Self::RegisterPinned {
                    token,
                    address,
                    length,
                    access,
                })
            }
            REGISTER if registers[5..].iter().all(|value| *value == 0) => {
                let [_, _, address, length, access, ..] = registers;
                if address & (PAGE_SIZE - 1) != 0
                    || length == 0
                    || length > MAX_SHARED_BYTES
                    || length & (PAGE_SIZE - 1) != 0
                    || address.checked_add(length).is_none()
                    || access == 0
                    || access & !(SHARED_READ | SHARED_WRITE) != 0
                {
                    return Err(Status::InvalidArgs);
                }
                Ok(Self::Register {
                    address,
                    length,
                    access,
                })
            }
            UNREGISTER if registers[3..].iter().all(|value| *value == 0) => Ok(Self::Unregister {
                handle: registers[2],
            }),
            STAGE_TRUSTY_CORE if registers[5..].iter().all(|value| *value == 0) => {
                let handle = registers[2];
                let length = registers[3];
                let generation = registers[4];
                if handle == 0 || length == 0 || length > MAX_SHARED_BYTES || generation == 0 {
                    return Err(Status::InvalidArgs);
                }
                Ok(Self::StageTrustyCore {
                    handle,
                    length,
                    generation,
                })
            }
            ACTIVATE_TRUSTY_CORE if registers[4..].iter().all(|value| *value == 0) => {
                let generation = registers[2];
                let activation = registers[3];
                if generation == 0 || !matches!(activation, ACTIVATE_LIVE_NOW | ACTIVATE_ON_REBOOT)
                {
                    return Err(Status::InvalidArgs);
                }
                Ok(Self::ActivateTrustyCore {
                    generation,
                    activation,
                })
            }
            REGISTER | REGISTER_PINNED | UNREGISTER | STAGE_TRUSTY_CORE | ACTIVATE_TRUSTY_CORE => {
                Err(Status::InvalidArgs)
            }
            _ => Err(Status::Unsupported),
        }
    }

    pub const fn encode(self) -> [u64; 8] {
        match self {
            Self::RegisterPinned {
                token,
                address,
                length,
                access,
            } => [
                HEADER,
                REGISTER_PINNED,
                token,
                address,
                length,
                access,
                0,
                0,
            ],
            Self::Register {
                address,
                length,
                access,
            } => [HEADER, REGISTER, address, length, access, 0, 0, 0],
            Self::Unregister { handle } => [HEADER, UNREGISTER, handle, 0, 0, 0, 0, 0],
            Self::StageTrustyCore {
                handle,
                length,
                generation,
            } => [
                HEADER,
                STAGE_TRUSTY_CORE,
                handle,
                length,
                generation,
                0,
                0,
                0,
            ],
            Self::ActivateTrustyCore {
                generation,
                activation,
            } => [
                HEADER,
                ACTIVATE_TRUSTY_CORE,
                generation,
                activation,
                0,
                0,
                0,
                0,
            ],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incompatible_architecture_version_and_reserved_words_fail_closed() {
        let valid = Request::Register {
            address: 0x1000,
            length: 4096,
            access: SHARED_READ,
        }
        .encode();
        assert!(Request::decode(valid).is_ok());
        for header in [HEADER ^ 1, HEADER ^ (1 << 16), HEADER ^ (1 << 32), 0] {
            let mut r = valid;
            r[0] = header;
            assert_eq!(Request::decode(r), Err(Status::Unsupported));
        }
        for index in 5..8 {
            let mut r = valid;
            r[index] = 1;
            assert_eq!(Request::decode(r), Err(Status::InvalidArgs));
        }
    }

    #[test]
    fn refuses_overflow_unbounded_unaligned_and_executable_buffers() {
        for (address, length, access) in [
            (u64::MAX - 4095, 4096, 1),
            (0, 0, 1),
            (0, 65537, 1),
            (1, 4096, 1),
            (4096, 4095, 1),
            (0, 4096, 0),
            (0, 4096, 4),
        ] {
            assert_eq!(
                Request::decode(
                    Request::Register {
                        address,
                        length,
                        access
                    }
                    .encode()
                ),
                Err(Status::InvalidArgs)
            );
        }
    }

    #[test]
    fn pinned_registration_cannot_acquire_the_resident_boot_identity() {
        for token in [0, PINNED_HANDLE_TAG - 1, PINNED_HANDLE_TAG, u64::MAX] {
            assert_eq!(
                Request::decode(
                    Request::RegisterPinned {
                        token,
                        address: 4096,
                        length: 4096,
                        access: SHARED_READ | SHARED_WRITE,
                    }
                    .encode()
                ),
                Err(Status::InvalidArgs)
            );
        }
        assert!(
            Request::decode(
                Request::RegisterPinned {
                    token: PINNED_HANDLE_TAG - 2,
                    address: 4096,
                    length: 4096,
                    access: SHARED_READ | SHARED_WRITE,
                }
                .encode()
            )
            .is_ok()
        );
    }

    #[test]
    fn decodes_versioned_trusty_core_requests() {
        assert_eq!(
            Request::decode(
                Request::StageTrustyCore {
                    handle: 7,
                    length: 4096,
                    generation: 8,
                }
                .encode()
            ),
            Ok(Request::StageTrustyCore {
                handle: 7,
                length: 4096,
                generation: 8,
            })
        );
        assert_eq!(
            Request::decode(
                Request::ActivateTrustyCore {
                    generation: 8,
                    activation: ACTIVATE_LIVE_NOW,
                }
                .encode()
            ),
            Ok(Request::ActivateTrustyCore {
                generation: 8,
                activation: ACTIVATE_LIVE_NOW,
            })
        );
        assert_eq!(
            Request::decode([HEADER, STAGE_TRUSTY_CORE, 0, 4096, 8, 0, 0, 0]),
            Err(Status::InvalidArgs)
        );
        assert_eq!(
            Request::decode([HEADER, ACTIVATE_TRUSTY_CORE, 8, 99, 0, 0, 0, 0]),
            Err(Status::InvalidArgs)
        );
    }
}
