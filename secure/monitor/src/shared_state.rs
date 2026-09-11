//! Registration handoff retains identifiers, but derives host addresses again
//! from the resident owner's RAM policy. No record can install a host pointer.
use super::*;
use sha2::{Digest, Sha256};

const HEADER: usize = 64;
const SLOT: usize = 40;
impl<const W: usize, const S: usize> Registry<W, S> {
    pub const fn state_bytes() -> usize {
        HEADER + S * SLOT + 32
    }

    fn window_digest(&self) -> [u8; 32] {
        let mut hash = Sha256::new();
        hash.update((W as u64).to_le_bytes());
        for window in &self.windows {
            hash.update(window.owner.0.to_le_bytes());
            hash.update(window.guest_start.to_le_bytes());
            hash.update(window.host_start.to_le_bytes());
            hash.update(window.length.to_le_bytes());
        }
        hash.finalize().into()
    }

    pub fn snapshot(&self, output: &mut [u8]) -> Result<(), Status> {
        if output.len() != Self::state_bytes() {
            return Err(Status::InvalidArgs);
        }
        output.fill(0);
        output[..8].copy_from_slice(b"BEXRG001");
        output[8..40].copy_from_slice(&self.window_digest());
        output[40..48].copy_from_slice(&self.next_handle.to_le_bytes());
        output[48..56].copy_from_slice(&(S as u64).to_le_bytes());
        for (index, entry) in self.slots.iter().enumerate() {
            let Some(entry) = entry else {
                continue;
            };
            let out = &mut output[HEADER + index * SLOT..HEADER + (index + 1) * SLOT];
            out[..8].copy_from_slice(&entry.handle.to_le_bytes());
            out[8..12].copy_from_slice(&entry.owner.0.to_le_bytes());
            out[16..24].copy_from_slice(&entry.guest_start.to_le_bytes());
            out[24..32].copy_from_slice(&entry.length.to_le_bytes());
            out[32..40].copy_from_slice(&entry.access.to_le_bytes());
        }
        let end = output.len() - 32;
        let digest = Sha256::digest(&output[..end]);
        output[end..].copy_from_slice(&digest);
        Ok(())
    }

    /// Requires provenance from protected resident transition memory and a
    /// matching kernel pin checkpoint. Checksums alone grant no authority.
    /// Validation finishes before changing any live registration.
    pub fn restore_protected(&mut self, input: &[u8]) -> Result<(), Status> {
        if input.len() != Self::state_bytes() || &input[..8] != b"BEXRG001" {
            return Err(Status::InvalidArgs);
        }
        let word = |offset| u64::from_le_bytes(input[offset..offset + 8].try_into().unwrap());
        let next = word(40);
        let end = input.len() - 32;
        if input[8..40] != self.window_digest()
            || next == 0
            || next >= PINNED_HANDLE_TAG
            || word(48) != S as u64
            || word(56) != 0
            || input[end..] != Sha256::digest(&input[..end])[..]
        {
            return Err(Status::AccessDenied);
        }
        let mut restored = Self::new(self.windows)?;
        for index in 0..S {
            let offset = HEADER + index * SLOT;
            let handle = word(offset);
            if handle == 0 {
                if input[offset..offset + SLOT].iter().any(|byte| *byte != 0) {
                    return Err(Status::InvalidArgs);
                }
                continue;
            }
            if input[offset + 12..offset + 16] != [0; 4]
                || restored
                    .slots
                    .iter()
                    .flatten()
                    .any(|entry| entry.handle == handle)
            {
                return Err(Status::InvalidArgs);
            }
            let address = word(offset + 16);
            let length = word(offset + 24);
            let access = word(offset + 32);
            let request = if handle & PINNED_HANDLE_TAG != 0 {
                Request::RegisterPinned {
                    token: handle & !PINNED_HANDLE_TAG,
                    address,
                    length,
                    access,
                }
            } else {
                if handle >= next {
                    return Err(Status::InvalidArgs);
                }
                // Allocate the exact retained id, not a temporary id that
                // could collide with an earlier restored registration.
                restored.next_handle = handle;
                Request::Register {
                    address,
                    length,
                    access,
                }
            };
            let owner = DomainId(u32::from_le_bytes(
                input[offset + 8..offset + 12].try_into().unwrap(),
            ));
            // Reuse live registration geometry, permission, overlap and pinned
            // identifier checks. Host addresses are recomputed from windows.
            let allocated = restored.request(
                Caller {
                    domain: owner,
                    may_share: true,
                },
                request.encode(),
            )?;
            let slot = restored
                .slots
                .iter()
                .position(|entry| entry.is_some_and(|entry| entry.handle == allocated))
                .ok_or(Status::InvalidHandle)?;
            let mut entry = restored.slots[slot].take().ok_or(Status::InvalidHandle)?;
            entry.handle = handle;
            restored.slots[index] = Some(entry);
        }
        restored.next_handle = next;
        *self = restored;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const OWNER: Caller = Caller {
        domain: DomainId(3),
        may_share: true,
    };
    fn registry() -> Registry<1, 3> {
        Registry::new([RamWindow {
            owner: OWNER.domain,
            guest_start: 0x1000,
            host_start: 0x200000,
            length: 0x10000,
        }])
        .unwrap()
    }
    fn add(registry: &mut Registry<1, 3>, address: u64) -> u64 {
        registry
            .request(
                OWNER,
                Request::Register {
                    address,
                    length: 4096,
                    access: SHARED_READ | SHARED_WRITE,
                }
                .encode(),
            )
            .unwrap()
    }
    #[test]
    fn preserves_holes_pin_identity_and_revoked_handle_floor() {
        let mut old = registry();
        let revoked = add(&mut old, 0x1000);
        let retained = add(&mut old, 0x2000);
        old.request(OWNER, Request::Unregister { handle: revoked }.encode())
            .unwrap();
        let pin = old
            .request(
                OWNER,
                Request::RegisterPinned {
                    token: 19,
                    address: 0x3000,
                    length: 4096,
                    access: SHARED_READ,
                }
                .encode(),
            )
            .unwrap();
        let mut bytes = [0; Registry::<1, 3>::state_bytes()];
        old.snapshot(&mut bytes).unwrap();
        let mut new = registry();
        new.restore_protected(&bytes).unwrap();
        assert_eq!(
            new.registered_length(OWNER, retained, SHARED_WRITE),
            Ok(4096)
        );
        assert_eq!(
            new.acquire(OWNER, pin, 0, 4096, SHARED_READ)
                .unwrap()
                .host_range(),
            (0x202000, 4096)
        );
        assert!(new.registered_length(OWNER, pin, SHARED_WRITE).is_err());
        assert!(add(&mut new, 0x1000) > retained);
        assert!(new.registered_length(OWNER, revoked, SHARED_READ).is_err());
    }
    #[test]
    fn restores_generic_ids_after_earlier_allocations_were_revoked() {
        let mut old = registry();
        let revoked = add(&mut old, 0x1000);
        old.request(OWNER, Request::Unregister { handle: revoked }.encode())
            .unwrap();
        let first = add(&mut old, 0x2000);
        let second = add(&mut old, 0x3000);
        let mut bytes = [0; Registry::<1, 3>::state_bytes()];
        old.snapshot(&mut bytes).unwrap();
        let mut new = registry();
        new.restore_protected(&bytes).unwrap();
        assert_eq!(
            new.acquire(OWNER, first, 0, 4096, SHARED_READ)
                .unwrap()
                .host_range(),
            (0x201000, 4096)
        );
        assert_eq!(
            new.acquire(OWNER, second, 0, 4096, SHARED_READ)
                .unwrap()
                .host_range(),
            (0x202000, 4096)
        );
        let mut encoded = [0; Registry::<1, 3>::state_bytes()];
        new.snapshot(&mut encoded).unwrap();
        assert_eq!(bytes, encoded);
    }
    #[test]
    fn rejects_forged_ranges_permissions_policy_and_reused_handles_atomically() {
        let mut old = registry();
        add(&mut old, 0x1000);
        let mut valid = [0; Registry::<1, 3>::state_bytes()];
        old.snapshot(&mut valid).unwrap();
        let mut new = registry();
        let mut before = [0; Registry::<1, 3>::state_bytes()];
        new.snapshot(&mut before).unwrap();
        for (offset, value) in [
            (8, 1),
            (40, 1),
            (48, 4),
            (56, 1),
            (HEADER + 8, 4),
            (HEADER + 12, 1),
            (HEADER + 16, 1),
            (HEADER + 24, 1),
            (HEADER + 32, 255),
        ] {
            let mut bad = valid;
            bad[offset] = value;
            let end = bad.len() - 32;
            let digest = Sha256::digest(&bad[..end]);
            bad[end..].copy_from_slice(&digest);
            assert!(new.restore_protected(&bad).is_err(), "offset {offset}");
        }
        valid[HEADER] ^= 1;
        assert!(new.restore_protected(&valid).is_err());
        let mut after = [0; Registry::<1, 3>::state_bytes()];
        new.snapshot(&mut after).unwrap();
        assert_eq!(before, after);
    }
}
