//! Monitor-owned registration table. Only immutable RAM windows assigned by
//! the monitor can be shared. Guest-provided host pointers are never accepted.
use bexos_secure_monitor_abi::{
    PAGE_SIZE, PINNED_HANDLE_TAG, Request, SHARED_READ, SHARED_WRITE, Status,
};
#[path = "shared_state.rs"]
mod state;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DomainId(pub u32);

/// Supplied by monitor setup and the exiting vCPU, never decoded from a request.
#[derive(Clone, Copy)]
pub struct Caller {
    pub domain: DomainId,
    pub may_share: bool,
}

#[derive(Clone, Copy)]
pub struct RamWindow {
    pub owner: DomainId,
    pub guest_start: u64,
    pub host_start: u64,
    pub length: u64,
}

#[derive(Clone, Copy)]
struct Registration {
    handle: u64,
    owner: DomainId,
    guest_start: u64,
    host_start: u64,
    length: u64,
    access: u64,
}

pub struct Registry<const WINDOWS: usize, const SLOTS: usize> {
    windows: [RamWindow; WINDOWS],
    slots: [Option<Registration>; SLOTS],
    next_handle: u64,
}

impl<const W: usize, const S: usize> Registry<W, S> {
    /// Windows must describe assigned RAM, excluding devices and protected
    /// monitor pages. This validates geometry and disjoint host ownership;
    /// installation into CPU and IOMMU page tables is a separate operation.
    pub fn new(windows: [RamWindow; W]) -> Result<Self, Status> {
        for (index, window) in windows.iter().enumerate() {
            if window.length == 0
                || (window.guest_start | window.host_start | window.length) & (PAGE_SIZE - 1) != 0
                || window.guest_start.checked_add(window.length).is_none()
                || window
                    .host_start
                    .checked_add(window.length)
                    .is_none_or(|end| end > 1 << 52)
            {
                return Err(Status::InvalidArgs);
            }
            for other in &windows[..index] {
                if overlaps(
                    window.host_start,
                    window.length,
                    other.host_start,
                    other.length,
                ) || (window.owner == other.owner
                    && overlaps(
                        window.guest_start,
                        window.length,
                        other.guest_start,
                        other.length,
                    ))
                {
                    return Err(Status::AccessDenied);
                }
            }
        }
        Ok(Self {
            windows,
            slots: [None; S],
            next_handle: 1,
        })
    }

    pub fn dispatch(&mut self, caller: Caller, registers: [u64; 8]) -> [u64; 8] {
        let result = self.request(caller, registers);
        match result {
            Ok(handle) => Status::Ok.registers(handle),
            Err(status) => status.registers(0),
        }
    }

    pub(crate) fn request(&mut self, caller: Caller, registers: [u64; 8]) -> Result<u64, Status> {
        if !caller.may_share {
            return Err(Status::AccessDenied);
        }
        let request = Request::decode(registers)?;
        let pinned = match request {
            Request::RegisterPinned { token, .. } => Some(token | PINNED_HANDLE_TAG),
            _ => None,
        };
        match request {
            Request::Register {
                address,
                length,
                access,
            }
            | Request::RegisterPinned {
                address,
                length,
                access,
                ..
            } => {
                let window = self
                    .windows
                    .iter()
                    .find(|window| {
                        window.owner == caller.domain
                            && address >= window.guest_start
                            && address + length <= window.guest_start + window.length
                    })
                    .ok_or(Status::AccessDenied)?;
                if self.slots.iter().flatten().any(|entry| {
                    (entry.owner == caller.domain
                        && overlaps(address, length, entry.guest_start, entry.length))
                        || pinned == Some(entry.handle)
                }) {
                    return Err(Status::Busy);
                }
                let slot = self
                    .slots
                    .iter_mut()
                    .find(|slot| slot.is_none())
                    .ok_or(Status::NoResources)?;
                let handle = pinned.unwrap_or(self.next_handle);
                // Never wrap and revive a revoked handle during this boot.
                if pinned.is_none() {
                    if handle >= PINNED_HANDLE_TAG - 1 {
                        return Err(Status::NoResources);
                    }
                    self.next_handle = handle + 1;
                }
                *slot = Some(Registration {
                    handle,
                    owner: caller.domain,
                    guest_start: address,
                    host_start: window.host_start + (address - window.guest_start),
                    length,
                    access,
                });
                Ok(handle)
            }
            Request::Unregister { handle } => {
                let slot = self
                    .slots
                    .iter_mut()
                    .find(|slot| {
                        slot.is_some_and(|entry| {
                            entry.handle == handle && entry.owner == caller.domain
                        })
                    })
                    .ok_or(Status::InvalidHandle)?;
                *slot = None;
                Ok(0)
            }
            Request::StageTrustyCore { .. } | Request::ActivateTrustyCore { .. } => {
                Err(Status::Unsupported)
            }
        }
    }

    /// Borrow a bounded registered buffer for a synchronous monitor operation.
    /// The mutable table borrow prevents unregister or reuse until completion.
    /// This conveys no right to execute the buffer or to submit raw DMA.
    pub fn acquire(
        &mut self,
        caller: Caller,
        handle: u64,
        offset: u64,
        length: u64,
        access: u64,
    ) -> Result<BufferLease<'_>, Status> {
        if !caller.may_share {
            return Err(Status::AccessDenied);
        }
        let entry = self
            .slots
            .iter_mut()
            .flatten()
            .find(|entry| entry.handle == handle && entry.owner == caller.domain)
            .ok_or(Status::InvalidHandle)?;
        if access == 0 || access & !(SHARED_READ | SHARED_WRITE) != 0 || access & !entry.access != 0
        {
            return Err(Status::AccessDenied);
        }
        if length == 0
            || offset
                .checked_add(length)
                .is_none_or(|end| end > entry.length)
        {
            return Err(Status::InvalidArgs);
        }
        Ok(BufferLease {
            entry,
            offset,
            length,
        })
    }

    pub fn registered_length(
        &mut self,
        caller: Caller,
        handle: u64,
        access: u64,
    ) -> Result<u64, Status> {
        Ok(self.acquire(caller, handle, 0, 1, access)?.entry.length)
    }
}

pub struct BufferLease<'a> {
    entry: &'a mut Registration,
    offset: u64,
    length: u64,
}

impl BufferLease<'_> {
    pub fn host_range(&self) -> (u64, u64) {
        (self.entry.host_start + self.offset, self.length)
    }
}

fn overlaps(a: u64, a_len: u64, b: u64, b_len: u64) -> bool {
    a < b + b_len && b < a + a_len
}

#[cfg(test)]
mod tests {
    use super::*;
    const OWNER: Caller = Caller {
        domain: DomainId(1),
        may_share: true,
    };
    const OTHER: Caller = Caller {
        domain: DomainId(2),
        may_share: true,
    };
    fn registry() -> Registry<1, 2> {
        Registry::new([RamWindow {
            owner: OWNER.domain,
            guest_start: 0x1000,
            host_start: 0x100000,
            length: 0x10000,
        }])
        .unwrap()
    }
    fn register(address: u64) -> [u64; 8] {
        Request::Register {
            address,
            length: 4096,
            access: SHARED_READ,
        }
        .encode()
    }

    #[test]
    fn ownership_bounds_permissions_and_revocation_are_enforced() {
        let mut registry = registry();
        assert_eq!(
            registry.dispatch(OTHER, register(0x1000)),
            Status::AccessDenied.registers(0)
        );
        assert_eq!(
            registry.dispatch(
                Caller {
                    may_share: false,
                    ..OWNER
                },
                register(0x1000)
            ),
            Status::AccessDenied.registers(0)
        );
        for address in [0, 0x11000, 0x100000] {
            assert_eq!(
                registry.dispatch(OWNER, register(address)),
                Status::AccessDenied.registers(0)
            );
        }
        let result = registry.dispatch(OWNER, register(0x1000));
        assert_eq!(result[0], 0);
        let handle = result[1];
        assert_eq!(
            registry
                .acquire(OWNER, handle, 12, 24, SHARED_READ)
                .unwrap()
                .host_range(),
            (0x10000c, 24)
        );
        assert!(matches!(
            registry.acquire(OTHER, handle, 0, 1, SHARED_READ),
            Err(Status::InvalidHandle)
        ));
        assert!(matches!(
            registry.acquire(OWNER, handle, 0, 1, SHARED_WRITE),
            Err(Status::AccessDenied)
        ));
        for (offset, length) in [(0, 0), (4096, 1), (u64::MAX, 2)] {
            assert!(matches!(
                registry.acquire(OWNER, handle, offset, length, SHARED_READ),
                Err(Status::InvalidArgs)
            ));
        }
        assert_eq!(
            registry.dispatch(OWNER, register(0x1000)),
            Status::Busy.registers(0)
        );
        let remove = Request::Unregister { handle }.encode();
        assert_eq!(
            registry.dispatch(OTHER, remove),
            Status::InvalidHandle.registers(0)
        );
        assert_eq!(registry.dispatch(OWNER, remove), Status::Ok.registers(0));
        let replacement = registry.dispatch(OWNER, register(0x1000))[1];
        assert_ne!(replacement, handle);
        assert_eq!(
            registry.dispatch(OWNER, remove),
            Status::InvalidHandle.registers(0)
        );
    }

    #[test]
    fn capacity_and_handle_exhaustion_do_not_change_existing_registrations() {
        let mut r = registry();
        assert_eq!(r.dispatch(OWNER, register(0x1000))[0], 0);
        assert_eq!(r.dispatch(OWNER, register(0x2000))[0], 0);
        assert_eq!(
            r.dispatch(OWNER, register(0x3000)),
            Status::NoResources.registers(0)
        );
        r.dispatch(OWNER, Request::Unregister { handle: 2 }.encode());
        r.next_handle = u64::MAX;
        assert_eq!(
            r.dispatch(OWNER, register(0x3000)),
            Status::NoResources.registers(0)
        );
        assert!(r.acquire(OWNER, 1, 0, 4096, SHARED_READ).is_ok());
    }

    #[test]
    fn overlapping_host_ownership_cannot_be_configured() {
        let a = RamWindow {
            owner: OWNER.domain,
            guest_start: 0,
            host_start: 0x100000,
            length: 0x10000,
        };
        let b = RamWindow {
            owner: OTHER.domain,
            ..a
        };
        assert!(matches!(
            Registry::<2, 1>::new([a, b]),
            Err(Status::AccessDenied)
        ));
        let b = RamWindow {
            host_start: 0x110000,
            ..b
        };
        assert!(Registry::<2, 1>::new([a, b]).is_ok());
    }

    #[test]
    fn kernel_pin_identity_is_disjoint_and_cannot_alias_another_registration() {
        let mut r = registry();
        let pinned = Request::RegisterPinned {
            token: 1,
            address: 0x1000,
            length: 4096,
            access: SHARED_READ,
        };
        assert_eq!(
            r.dispatch(OWNER, pinned.encode()),
            Status::Ok.registers(PINNED_HANDLE_TAG | 1)
        );
        assert_eq!(r.dispatch(OWNER, register(0x2000)), Status::Ok.registers(1));
        assert!(
            r.acquire(OWNER, PINNED_HANDLE_TAG | 1, 0, 4096, SHARED_READ)
                .is_ok()
        );
        assert_eq!(
            r.dispatch(
                OTHER,
                Request::Unregister {
                    handle: PINNED_HANDLE_TAG | 1
                }
                .encode()
            ),
            Status::InvalidHandle.registers(0)
        );
        for token in [0, PINNED_HANDLE_TAG, u64::MAX] {
            assert_eq!(
                r.dispatch(
                    OWNER,
                    Request::RegisterPinned {
                        token,
                        address: 0x3000,
                        length: 4096,
                        access: SHARED_READ
                    }
                    .encode()
                ),
                Status::InvalidArgs.registers(0)
            );
        }
        r.dispatch(OWNER, Request::Unregister { handle: 1 }.encode());
        assert_eq!(
            r.dispatch(
                OWNER,
                Request::RegisterPinned {
                    token: 1,
                    address: 0x3000,
                    length: 4096,
                    access: SHARED_READ
                }
                .encode()
            ),
            Status::Busy.registers(0)
        );
    }
}
