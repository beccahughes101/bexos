//! Canonical records kept exclusively in resident protected memory. Checksums
//! detect damage, not provenance; no guest may submit these records for import.
use super::*;
use crate::StateIdentity;
use sha2::{Digest, Sha256};

pub const STATE_BYTES: usize = 64 + crate::STATE_BYTES + 64 + 32;
const WRITER: usize = 64 + crate::STATE_BYTES;
const CHECKSUM: usize = WRITER + 64;

impl Live {
    pub fn snapshot(
        &self,
        identity: StateIdentity,
        out: &mut [u8; STATE_BYTES],
    ) -> Result<(), Error> {
        if identity == StateIdentity::X86Hypervisor {
            return Err(Error::Invalid);
        }
        out.fill(0);
        out[..8].copy_from_slice(b"BEXLW002");
        for (offset, value) in [
            (8, self.candidate),
            (16, self.required_services),
            (24, self.ready_services),
            (32, self.copied_through.unwrap_or(0)),
            (40, self.transport_epoch),
            (56, self.migration_abi),
        ] {
            out[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
        }
        out[48] = u8::from(self.copied_through.is_some());
        out[49] = u8::from(self.recovery_required);
        self.slots
            .snapshot(identity, (&mut out[64..WRITER]).try_into().unwrap())?;
        out[WRITER..CHECKSUM].copy_from_slice(&self.writer.snapshot());
        let digest = Sha256::digest(&out[..CHECKSUM]);
        out[CHECKSUM..].copy_from_slice(&digest);
        Ok(())
    }
    /// Validate everything into a new owner before touching a live destination.
    /// Original deadlines and any unresolved write/commit remain in force.
    pub fn restore_protected(
        bytes: &[u8],
        identity: StateIdentity,
        now: u64,
    ) -> Result<Self, Error> {
        if identity == StateIdentity::X86Hypervisor
            || bytes.len() != STATE_BYTES
            || &bytes[..8] != b"BEXLW002"
            || bytes[48] > 1
            || bytes[49] > 1
            || bytes[50..56] != [0; 6]
            || bytes[CHECKSUM..] != Sha256::digest(&bytes[..CHECKSUM])[..]
        {
            return Err(Error::Invalid);
        }
        let word = |i| u64::from_le_bytes(bytes[i..i + 8].try_into().unwrap());
        let writer = Writer::restore(&bytes[WRITER..CHECKSUM])?;
        let slots = TeeSlotState::restore_protected(&bytes[64..WRITER], identity, now)?;
        let value = Self {
            candidate: word(8),
            required_services: word(16),
            ready_services: word(24),
            copied_through: (bytes[48] != 0).then(|| word(32)),
            transport_epoch: word(40),
            migration_abi: word(56),
            recovery_required: bytes[49] != 0,
            writer,
            slots,
        };
        if value.required_services == 0
            || value.ready_services & !value.required_services != 0
            || value.transport_epoch < value.writer.epoch()
            || value.migration_abi == 0
            || (value.copied_through.is_none() && (word(32) != 0 || value.ready_services != 0))
            || value
                .copied_through
                .is_some_and(|w| w > value.writer.watermark())
        {
            return Err(Error::Invalid);
        }
        use TeeUpdatePhase::*;
        match value.slots.phase {
            Verifying | Prepared | LiveSwitch | HealthWindow | CommitPending | CommitUncertain => {
                let remaining = if matches!(value.slots.phase, Verifying | Prepared | LiveSwitch) {
                    2
                } else {
                    1
                };
                if value.candidate == 0
                    || value.candidate == value.writer.owner()
                    || value.transport_epoch.checked_add(remaining).is_none()
                    || value.writer.epoch().checked_add(1).is_none()
                {
                    return Err(Error::Invalid);
                }
            }
            Completed => {
                if value.candidate != value.writer.owner() {
                    return Err(Error::Invalid);
                }
            }
            Idle | RolledBack => {
                if !value.recovery_required
                    && (value.candidate != 0 || value.copied_through.is_some())
                {
                    return Err(Error::Invalid);
                }
            }
            _ => return Err(Error::Invalid),
        }
        if matches!(
            value.slots.phase,
            LiveSwitch | HealthWindow | CommitPending | CommitUncertain
        ) && !value.writer.frozen()
        {
            return Err(Error::Invalid);
        }
        if matches!(
            value.slots.phase,
            HealthWindow | CommitPending | CommitUncertain
        ) && (!value.writer.drained() || value.copied_through != Some(value.writer.watermark()))
        {
            return Err(Error::Invalid);
        }
        if matches!(value.slots.phase, CommitPending | CommitUncertain)
            && value.ready_services != value.required_services
        {
            return Err(Error::Invalid);
        }
        Ok(value)
    }
}
