//! Version-one candidate transfer. The registered owner buffer is reused for
//! each chunk; its revocation cancels the transfer and erases the private copy.
use crate::{HEADER, MAX_SHARED_BYTES, PINNED_HANDLE_TAG, Status};
// 0x200 belongs to boot evidence confirmation. Keep the whole update range
// disjoint so even malformed requests cannot be routed to another authority.
pub const BEGIN: u64 = 0x210;
pub const WRITE: u64 = 0x211;
pub const SEAL: u64 = 0x212;
pub const ABORT: u64 = 0x213;
pub const ACTIVATE: u64 = 0x214;
pub const QUERY: u64 = 0x215;
pub const RESOLVE: u64 = 0x216;
const _: () = assert!(crate::boot::EVIDENCE_QUERY < BEGIN || crate::boot::EVIDENCE_QUERY > RESOLVE);
pub const MAX_BUNDLE_BYTES: u64 = 64 * 1024 * 1024 + 64 * 1024 + 32;
pub const TRUSTY: u64 = 1;
pub const HYPERVISOR: u64 = 2;
pub const LIVE: u64 = 1;
pub const ON_REBOOT: u64 = 2;
pub const RESOLVE_COMMIT: u64 = 1;
pub const RESOLVE_ROLLBACK: u64 = 2;
pub const IDLE: u64 = 0;
pub const STAGED: u64 = 1;
pub const APPLYING: u64 = 2;
pub const PENDING: u64 = 3;
pub const COMMITTED: u64 = 4;
pub const ROLLED_BACK: u64 = 5;
pub const RECOVERY_REQUIRED: u64 = 6;
pub const REJECTED: u64 = 7;
pub const QUERY_OUTCOME: u64 = 0;
pub const QUERY_GENERATION: u64 = 1;
pub const QUERY_COMPONENT: u64 = 2;
pub const QUERY_SLOT: u64 = 3;
pub const QUERY_TRUSTY_GENERATION: u64 = 4;
pub const QUERY_MONITOR_GENERATION: u64 = 5;
pub const QUERY_REVISION: u64 = 6;
pub const QUERY_CAPABILITIES: u64 = 7;
pub const QUERY_MIGRATION_ABI: u64 = 8;
pub const QUERY_TRANSPORT_GENERATION: u64 = 9;
pub const CAP_TRUSTY_REBOOT: u64 = 1;
pub const CAP_TRUSTY_LIVE: u64 = 2;
pub const CAP_MONITOR_REBOOT: u64 = 4;
pub const CAP_MONITOR_LIVE: u64 = 8;

/// An advertised capability describes the installed execution owner, not the
/// provider's compiled-in protocol vocabulary. A missing owner advertises none.
pub const fn activation_capability(component: u64, mode: u64) -> Option<u64> {
    match (component, mode) {
        (TRUSTY, ON_REBOOT) => Some(CAP_TRUSTY_REBOOT),
        (TRUSTY, LIVE) => Some(CAP_TRUSTY_LIVE),
        (HYPERVISOR, ON_REBOOT) => Some(CAP_MONITOR_REBOOT),
        (HYPERVISOR, LIVE) => Some(CAP_MONITOR_LIVE),
        _ => None,
    }
}

/// Provider preflight, not authorization: Activate is always checked again by
/// the execution owner. Owners predating capability discovery can still handle
/// the existing reboot and monitor-policy modes. Live Trusty has no legacy
/// fallback, and an explicit capability response is never overridden.
pub fn activation_preflight(
    component: u64,
    mode: u64,
    capabilities: Result<u64, Status>,
) -> Result<(), Status> {
    let bit = activation_capability(component, mode).ok_or(Status::InvalidArgs)?;
    match capabilities {
        Ok(mask) if mask & bit != 0 => Ok(()),
        Ok(_) => Err(Status::Unsupported),
        Err(Status::InvalidArgs | Status::Unsupported) if bit != CAP_TRUSTY_LIVE => Ok(()),
        Err(status) => Err(status),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Call {
    Begin {
        owner: u64,
        length: u64,
        generation: u64,
        component: u64,
    },
    Write {
        owner: u64,
        offset: u64,
        length: u64,
    },
    Seal {
        owner: u64,
    },
    Abort {
        owner: u64,
    },
    Activate {
        owner: u64,
        mode: u64,
    },
    Resolve {
        owner: u64,
        disposition: u64,
    },
    Query {
        owner: u64,
        field: u64,
    },
}
impl Call {
    pub fn owner(self) -> u64 {
        match self {
            Self::Begin { owner, .. }
            | Self::Write { owner, .. }
            | Self::Seal { owner }
            | Self::Abort { owner }
            | Self::Activate { owner, .. }
            | Self::Resolve { owner, .. }
            | Self::Query { owner, .. } => owner,
        }
    }
    pub fn decode(r: [u64; 8]) -> Result<Self, Status> {
        if r[0] != HEADER {
            return Err(Status::Unsupported);
        }
        if r[2] & PINNED_HANDLE_TAG == 0 || r[2] == PINNED_HANDLE_TAG || r[2] == u64::MAX {
            return Err(Status::InvalidHandle);
        }
        match r[1] {
            BEGIN
                if r[3] > 0
                    && r[3] <= MAX_BUNDLE_BYTES
                    && r[4] > 0
                    && matches!(r[5], TRUSTY | HYPERVISOR)
                    && r[6..] == [0; 2] =>
            {
                Ok(Self::Begin {
                    owner: r[2],
                    length: r[3],
                    generation: r[4],
                    component: r[5],
                })
            }
            WRITE
                if r[4] > 0
                    && r[4] <= MAX_SHARED_BYTES
                    && r[3]
                        .checked_add(r[4])
                        .is_some_and(|n| n <= MAX_BUNDLE_BYTES)
                    && r[5..] == [0; 3] =>
            {
                Ok(Self::Write {
                    owner: r[2],
                    offset: r[3],
                    length: r[4],
                })
            }
            SEAL if r[3..] == [0; 5] => Ok(Self::Seal { owner: r[2] }),
            ABORT if r[3..] == [0; 5] => Ok(Self::Abort { owner: r[2] }),
            ACTIVATE if matches!(r[3], LIVE | ON_REBOOT) && r[4..] == [0; 4] => {
                Ok(Self::Activate {
                    owner: r[2],
                    mode: r[3],
                })
            }
            RESOLVE if matches!(r[3], RESOLVE_COMMIT | RESOLVE_ROLLBACK) && r[4..] == [0; 4] => {
                Ok(Self::Resolve {
                    owner: r[2],
                    disposition: r[3],
                })
            }
            QUERY if r[3] <= QUERY_TRANSPORT_GENERATION && r[4..] == [0; 4] => Ok(Self::Query {
                owner: r[2],
                field: r[3],
            }),
            BEGIN | WRITE | SEAL | ABORT | ACTIVATE | QUERY | RESOLVE => Err(Status::InvalidArgs),
            _ => Err(Status::Unsupported),
        }
    }
    pub fn encode(self) -> [u64; 8] {
        match self {
            Self::Begin {
                owner,
                length,
                generation,
                component,
            } => [HEADER, BEGIN, owner, length, generation, component, 0, 0],
            Self::Write {
                owner,
                offset,
                length,
            } => [HEADER, WRITE, owner, offset, length, 0, 0, 0],
            Self::Seal { owner } => [HEADER, SEAL, owner, 0, 0, 0, 0, 0],
            Self::Abort { owner } => [HEADER, ABORT, owner, 0, 0, 0, 0, 0],
            Self::Activate { owner, mode } => [HEADER, ACTIVATE, owner, mode, 0, 0, 0, 0],
            Self::Resolve { owner, disposition } => {
                [HEADER, RESOLVE, owner, disposition, 0, 0, 0, 0]
            }
            Self::Query { owner, field } => [HEADER, QUERY, owner, field, 0, 0, 0, 0],
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_preflight_never_enables_live_trusty_or_overrides_owner_denial() {
        for component in [TRUSTY, HYPERVISOR] {
            for mode in [LIVE, ON_REBOOT] {
                let bit = activation_capability(component, mode).unwrap();
                assert_eq!(activation_preflight(component, mode, Ok(bit)), Ok(()));
                assert_eq!(
                    activation_preflight(component, mode, Ok(0)),
                    Err(Status::Unsupported)
                );
                for status in [Status::AccessDenied, Status::InvalidHandle, Status::Busy] {
                    assert_eq!(
                        activation_preflight(component, mode, Err(status)),
                        Err(status)
                    );
                }
                for status in [Status::InvalidArgs, Status::Unsupported] {
                    assert_eq!(
                        activation_preflight(component, mode, Err(status)),
                        if bit == CAP_TRUSTY_LIVE {
                            Err(status)
                        } else {
                            Ok(())
                        }
                    );
                }
            }
        }
        assert_eq!(
            activation_preflight(0, LIVE, Ok(u64::MAX)),
            Err(Status::InvalidArgs)
        );
    }
    #[test]
    fn activation_capabilities_are_independent_and_queries_are_versioned() {
        let cases = [
            (TRUSTY, LIVE, CAP_TRUSTY_LIVE),
            (TRUSTY, ON_REBOOT, CAP_TRUSTY_REBOOT),
            (HYPERVISOR, LIVE, CAP_MONITOR_LIVE),
            (HYPERVISOR, ON_REBOOT, CAP_MONITOR_REBOOT),
        ];
        for (component, mode, bit) in cases {
            assert_eq!(activation_capability(component, mode), Some(bit));
            assert!(bit.is_power_of_two());
        }
        assert_eq!(activation_capability(0, LIVE), None);
        assert_eq!(activation_capability(TRUSTY, 3), None);
        for field in [
            QUERY_CAPABILITIES,
            QUERY_MIGRATION_ABI,
            QUERY_TRANSPORT_GENERATION,
        ] {
            let query = Call::Query {
                owner: PINNED_HANDLE_TAG | 1,
                field,
            };
            assert_eq!(Call::decode(query.encode()), Ok(query));
        }
    }
    #[test]
    fn activation_and_status_have_closed_versioned_fields() {
        let owner = PINNED_HANDLE_TAG | 4;
        for valid in [
            Call::Activate { owner, mode: LIVE },
            Call::Activate {
                owner,
                mode: ON_REBOOT,
            },
            Call::Query {
                owner,
                field: QUERY_SLOT,
            },
            Call::Resolve {
                owner,
                disposition: RESOLVE_COMMIT,
            },
            Call::Resolve {
                owner,
                disposition: RESOLVE_ROLLBACK,
            },
        ] {
            assert_eq!(Call::decode(valid.encode()), Ok(valid));
            for i in 4..8 {
                let mut r = valid.encode();
                r[i] = 1;
                assert!(Call::decode(r).is_err());
            }
        }
        for mode in [0, 3, u64::MAX] {
            assert!(Call::decode(Call::Activate { owner, mode }.encode()).is_err());
            assert!(
                Call::decode(
                    Call::Resolve {
                        owner,
                        disposition: mode,
                    }
                    .encode()
                )
                .is_err()
            );
        }
        assert!(
            Call::decode(
                Call::Query {
                    owner,
                    field: QUERY_TRANSPORT_GENERATION + 1
                }
                .encode()
            )
            .is_err()
        );
    }
    #[test]
    fn firmware_requests_require_pinned_owner_and_bounded_versioned_fields() {
        let valid = Call::Begin {
            owner: PINNED_HANDLE_TAG | 1,
            length: 40 * 1024 * 1024,
            generation: 2,
            component: TRUSTY,
        };
        assert_eq!(Call::decode(valid.encode()), Ok(valid));
        for (index, value) in [
            (0, HEADER ^ 1),
            (2, 0),
            (2, u64::MAX),
            (3, MAX_BUNDLE_BYTES + 1),
            (4, 0),
            (5, 3),
            (6, 1),
            (7, 1),
        ] {
            let mut request = valid.encode();
            request[index] = value;
            assert!(Call::decode(request).is_err());
        }
        assert!(
            Call::decode(
                Call::Write {
                    owner: valid.owner(),
                    offset: u64::MAX,
                    length: 1
                }
                .encode()
            )
            .is_err()
        );
        assert!(
            Call::decode(
                Call::Write {
                    owner: valid.owner(),
                    offset: 0,
                    length: MAX_SHARED_BYTES + 1
                }
                .encode()
            )
            .is_err()
        );
    }
}
