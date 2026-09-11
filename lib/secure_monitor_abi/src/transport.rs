//! Bounded, asynchronous copied messages. Submit snapshots normal RAM; a
//! completion is copied back only by Poll while the caller's pin is live.
use crate::{HEADER, MAX_SHARED_BYTES, PAGE_SIZE, PINNED_HANDLE_TAG, Status};
pub const SUBMIT: u64 = 0x100;
pub const POLL: u64 = 0x101;
pub const FETCH: u64 = 0x102;
pub const COMPLETE: u64 = 0x103;
pub const IS_REGISTERED: u64 = 0x104;
pub const ROOT_FETCH: u64 = 0x105;
pub const ROOT_COMPLETE: u64 = 0x106;
pub const DISCOVERY_LEAF: u32 = 0x40000100;
pub const DISCOVERY_MAGIC: u32 = 0x4245584d;
/// CPUID EDX: this product initializes boot-dependent services in BexOS.
/// Standalone monitor diagnostics have no normal-world boot owner to wait for.
pub const DISCOVERY_BOOT_SERVICES: u32 = 1;
pub const DISCOVERY_ROOT_CONTROL: u32 = 2;
pub const DISCOVERY_CAPABILITIES: u32 = DISCOVERY_BOOT_SERVICES | DISCOVERY_ROOT_CONTROL;
/// Upper operations belong to the resident boot owner, never a guest caller.
pub const BOOT_OWNER_OPERATION_BASE: u32 = 0x80000000;
pub const BOOT_OWNER_HANDLE: u64 = u64::MAX;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Call {
    Submit {
        handle: u64,
        operation: u32,
        length: u64,
    },
    Poll {
        handle: u64,
        ticket: u64,
    },
    Fetch {
        address: u64,
    },
    Complete {
        ticket: u64,
        result: i64,
    },
    RootFetch {
        address: u64,
    },
    RootComplete {
        ticket: u64,
        result: i64,
    },
    IsRegistered {
        handle: u64,
    },
}
impl Call {
    pub fn decode(r: [u64; 8]) -> Result<Self, Status> {
        if r[0] != HEADER {
            return Err(Status::Unsupported);
        }
        if r[5..].iter().any(|v| *v != 0) {
            return Err(Status::InvalidArgs);
        }
        match r[1] {
            SUBMIT
                if r[2] & PINNED_HANDLE_TAG != 0
                    && r[2] != PINNED_HANDLE_TAG
                    && r[2] != BOOT_OWNER_HANDLE
                    && r[3] > 0
                    && r[3] < u64::from(BOOT_OWNER_OPERATION_BASE)
                    && r[4] > 0
                    && r[4] <= MAX_SHARED_BYTES =>
            {
                Ok(Self::Submit {
                    handle: r[2],
                    operation: r[3] as u32,
                    length: r[4],
                })
            }
            POLL if r[2] & PINNED_HANDLE_TAG != 0
                && r[2] != PINNED_HANDLE_TAG
                && r[2] != BOOT_OWNER_HANDLE
                && r[3] != 0
                && r[4] == 0 =>
            {
                Ok(Self::Poll {
                    handle: r[2],
                    ticket: r[3],
                })
            }
            FETCH | ROOT_FETCH
                if r[2] != 0
                    && r[2] & (PAGE_SIZE - 1) == 0
                    && r[2].checked_add(MAX_SHARED_BYTES).is_some()
                    && r[3] == 0
                    && r[4] == 0 =>
            {
                Ok(if r[1] == ROOT_FETCH {
                    Self::RootFetch { address: r[2] }
                } else {
                    Self::Fetch { address: r[2] }
                })
            }
            COMPLETE if r[2] != 0 && r[4] == 0 => Ok(Self::Complete {
                ticket: r[2],
                result: r[3] as i64,
            }),
            ROOT_COMPLETE if r[2] != 0 && r[4] == 0 => Ok(Self::RootComplete {
                ticket: r[2],
                result: r[3] as i64,
            }),
            IS_REGISTERED
                if r[2] & PINNED_HANDLE_TAG != 0
                    && r[2] != PINNED_HANDLE_TAG
                    && r[3] == 0
                    && r[4] == 0 =>
            {
                Ok(Self::IsRegistered { handle: r[2] })
            }
            SUBMIT | POLL | FETCH | COMPLETE | ROOT_FETCH | ROOT_COMPLETE | IS_REGISTERED => {
                Err(Status::InvalidArgs)
            }
            _ => Err(Status::Unsupported),
        }
    }
    pub const fn encode(self) -> [u64; 8] {
        match self {
            Self::Submit {
                handle,
                operation,
                length,
            } => [HEADER, SUBMIT, handle, operation as u64, length, 0, 0, 0],
            Self::Poll { handle, ticket } => [HEADER, POLL, handle, ticket, 0, 0, 0, 0],
            Self::Fetch { address } => [HEADER, FETCH, address, 0, 0, 0, 0, 0],
            Self::Complete { ticket, result } => {
                [HEADER, COMPLETE, ticket, result as u64, 0, 0, 0, 0]
            }
            Self::RootFetch { address } => [HEADER, ROOT_FETCH, address, 0, 0, 0, 0, 0],
            Self::RootComplete { ticket, result } => {
                [HEADER, ROOT_COMPLETE, ticket, result as u64, 0, 0, 0, 0]
            }
            Self::IsRegistered { handle } => [HEADER, IS_REGISTERED, handle, 0, 0, 0, 0, 0],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn transport_rejects_unregistered_unbounded_and_incompatible_messages() {
        let valid = Call::Submit {
            handle: PINNED_HANDLE_TAG | 1,
            operation: 1,
            length: 8192,
        };
        assert_eq!(Call::decode(valid.encode()), Ok(valid));
        for (index, value) in [
            (0, HEADER ^ 1),
            (2, 1),
            (2, PINNED_HANDLE_TAG),
            (2, BOOT_OWNER_HANDLE),
            (3, 1 << 32),
            (3, u64::from(BOOT_OWNER_OPERATION_BASE)),
            (3, u64::from(u32::MAX)),
            (4, MAX_SHARED_BYTES + 1),
            (5, 1),
            (6, 1),
            (7, 1),
        ] {
            let mut r = valid.encode();
            r[index] = value;
            assert!(Call::decode(r).is_err());
        }
        for address in [0, 1, u64::MAX - 4095] {
            assert_eq!(
                Call::decode(Call::Fetch { address }.encode()),
                Err(Status::InvalidArgs)
            );
            assert_eq!(
                Call::decode(Call::RootFetch { address }.encode()),
                Err(Status::InvalidArgs)
            );
        }
        for call in [
            Call::RootFetch { address: PAGE_SIZE },
            Call::RootComplete {
                ticket: 1,
                result: -7,
            },
        ] {
            assert_eq!(Call::decode(call.encode()), Ok(call));
            let mut invalid = call.encode();
            invalid[7] = 1;
            assert_eq!(Call::decode(invalid), Err(Status::InvalidArgs));
        }
    }
}
