//! Generations of the boot components authenticated by the signed EFI image.
//! These constants describe the current image, not a writable generation floor.
use bexos_secure_firmware::Component;
use bexos_secure_monitor_abi::Status;
use bexos_trusty_boot::approval::{Approval, Generation};
use bexos_trusty_boot::{approval, journal, ql};

pub const TRUSTY: Generation = Generation {
    generation: 1,
    location: 30,
};
pub const MONITOR: Generation = Generation {
    generation: 1,
    location: 31,
};
#[cfg(feature = "resident_nucleus")]
static mut SELECTED: [bexos_secure_firmware::selection::Identity; 2] =
    [bexos_secure_firmware::selection::Identity::INITIAL; 2];
#[cfg(feature = "resident_nucleus")]
pub fn select(identities: [bexos_secure_firmware::selection::Identity; 2]) {
    assert!(identities.iter().all(|id| id.valid(true)));
    unsafe {
        SELECTED = identities;
    }
}
fn current() -> [Generation; 2] {
    #[cfg(feature = "resident_nucleus")]
    {
        let selected = unsafe { SELECTED };
        return [
            Generation {
                generation: selected[0].generation,
                ..TRUSTY
            },
            Generation {
                generation: selected[1].generation,
                ..MONITOR
            },
        ];
    }
    #[cfg(not(feature = "resident_nucleus"))]
    {
        [TRUSTY, MONITOR]
    }
}
#[cfg(feature = "resident_nucleus")]
pub fn active_generations() -> [u64; 2] {
    current().map(|g| g.generation)
}
pub fn trusty_digest() -> [u8; 32] {
    #[cfg(feature = "resident_nucleus")]
    if unsafe { SELECTED[0] } != bexos_secure_firmware::selection::Identity::INITIAL {
        return unsafe { SELECTED[0].digest };
    }
    use sha2::{Digest, Sha256};
    Sha256::digest(crate::TRUSTY).into()
}
#[unsafe(link_section = ".resident.approvals")]
static mut APPROVED: [u8; 32 + 2 * approval::STATE_BYTES] = [0; 32 + 2 * approval::STATE_BYTES];

pub fn committed(
    transport: &mut impl ql::Transport,
) -> Result<[journal::Record; 2], journal::Error> {
    let records = [
        journal::query(transport, 2, 1)?,
        journal::query(transport, 2, 2)?,
    ];
    // Boot selection must load the durably committed image before execution.
    // The current signed base image has generation one; it cannot substitute
    // for a later committed slot whose recovery image is unavailable.
    let selected = current();
    if records[0].generation > selected[0].generation
        || records[1].generation > selected[1].generation
    {
        return Err(journal::Error::Rollback);
    }
    Ok(records)
}

pub fn approve_chain(
    state: &mut impl approval::State,
    payload: Generation,
    records: [journal::Record; 2],
) -> Result<[Approval; 3], approval::Error> {
    struct Floors<'a, S> {
        state: &'a mut S,
        generations: [u64; 2],
    }
    impl<S: approval::State> approval::State for Floors<'_, S> {
        fn locked(&mut self) -> Result<bool, ql::Error> {
            self.state.locked()
        }
        fn floor(&mut self, location: u32) -> Result<u64, ql::Error> {
            let avb = self.state.floor(location)?;
            Ok(match location {
                30 => avb.max(self.generations[0]),
                31 => avb.max(self.generations[1]),
                _ => avb,
            })
        }
        fn seal(&mut self) -> Result<(), ql::Error> {
            self.state.seal()
        }
    }
    approval::approve_chain(
        &mut Floors {
            state,
            generations: [records[0].generation, records[1].generation],
        },
        [payload, current()[0], current()[1]],
    )
}

pub fn install(trusty: Approval, monitor: Approval) {
    for (approved, expected) in [(trusty, current()[0]), (monitor, current()[1])] {
        assert_eq!(approved.location(), expected.location);
        assert_eq!(approved.generation(), expected.generation);
    }
    // Only the single root owner executes, with both guest domains stopped.
    unsafe {
        let slot = &mut *core::ptr::addr_of_mut!(APPROVED);
        assert!(slot.iter().all(|byte| *byte == 0));
        for (target, approved) in slot[32..]
            .chunks_exact_mut(approval::STATE_BYTES)
            .zip([trusty, monitor])
        {
            target.copy_from_slice(&approved.snapshot());
        }
        slot[8..16].copy_from_slice(&2u64.to_le_bytes());
        slot[16..24].copy_from_slice(&2u64.to_le_bytes());
        slot[..8].copy_from_slice(b"BEXFG001");
    }
}
pub fn approved(component: Component) -> Result<Approval, Status> {
    let index = match component {
        Component::Trusty => 0,
        Component::Hypervisor => 1,
    };
    let bytes = unsafe { &*core::ptr::addr_of!(APPROVED) };
    if &bytes[..8] != b"BEXFG001"
        || bytes[8..16] != 2u64.to_le_bytes()
        || bytes[16..24] != 2u64.to_le_bytes()
        || bytes[24..32] != [0; 8]
    {
        return Err(Status::Unsupported);
    }
    let start = 32 + index * approval::STATE_BYTES;
    let approved =
        unsafe { Approval::restore_protected(&bytes[start..start + approval::STATE_BYTES]) }
            .map_err(|_| Status::AccessDenied)?;
    if approved.location() != component.rollback_location() {
        return Err(Status::AccessDenied);
    }
    Ok(approved)
}
