//! Versioned lifecycle records for protected resident handoff memory. The hash
//! detects corruption; it is not caller authentication. Only a trusted execution
//! owner may supply these bytes. No normal-world import interface is provided.
use super::*;
use sha2::{Digest, Sha256};

pub const STATE_BYTES: usize = 160;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StateIdentity {
    ArmTrusty,
    X86Trusty,
    X86Hypervisor,
}
impl StateIdentity {
    fn wire(self) -> [u8; 4] {
        match self {
            Self::ArmTrusty => [1, 0, 1, 0],
            Self::X86Trusty => [2, 0, 1, 0],
            Self::X86Hypervisor => [2, 0, 2, 0],
        }
    }
}

impl TeeSlotState {
    pub fn snapshot(
        &self,
        identity: StateIdentity,
        out: &mut [u8; STATE_BYTES],
    ) -> Result<(), OrchestratorError> {
        self.validate_record()?;
        out.fill(0);
        out[..8].copy_from_slice(b"BEXSW001");
        out[8..12].copy_from_slice(&identity.wire());
        out[12..16].copy_from_slice(&(STATE_BYTES as u32).to_le_bytes());
        out[16] = phase(self.phase);
        out[17] = slot(Some(self.active_slot));
        out[18] = slot(self.pending_slot);
        out[19] = slot(self.rollback_slot);
        out[20] = u8::from(self.reboot_required)
            | (u8::from(self.preparation_started.is_some()) << 1)
            | (u8::from(self.cutover_started.is_some()) << 2);
        for (offset, value) in [
            (24, self.generation),
            (32, self.pending_generation.unwrap_or(0)),
            (40, self.preparation_started.unwrap_or(0)),
            (48, self.cutover_started.unwrap_or(0)),
            (56, self.last_tick),
        ] {
            out[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
        }
        out[64..96].copy_from_slice(&self.slot_hashes[0]);
        out[96..128].copy_from_slice(&self.slot_hashes[1]);
        let digest = Sha256::digest(&out[..128]);
        out[128..].copy_from_slice(&digest);
        Ok(())
    }

    /// Decode a record whose provenance the resident execution owner has already
    /// established. This does not approve an image or resolve a durable commit.
    pub fn restore_protected(
        bytes: &[u8],
        identity: StateIdentity,
        now_ns: u64,
    ) -> Result<Self, OrchestratorError> {
        let invalid = OrchestratorError::InvalidState;
        if bytes.len() != STATE_BYTES
            || &bytes[..8] != b"BEXSW001"
            || bytes[8..12] != identity.wire()
            || bytes[12..16] != (STATE_BYTES as u32).to_le_bytes()
            || bytes[20] & !7 != 0
            || bytes[21..24] != [0; 3]
            || bytes[128..] != Sha256::digest(&bytes[..128])[..]
        {
            return Err(invalid);
        }
        let word = |offset| u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap());
        let pending_slot = read_slot(bytes[18])?;
        let mut state = Self {
            active_slot: read_slot(bytes[17])?.ok_or(invalid)?,
            pending_slot,
            rollback_slot: read_slot(bytes[19])?,
            generation: word(24),
            phase: read_phase(bytes[16])?,
            reboot_required: bytes[20] & 1 != 0,
            pending_generation: pending_slot.map(|_| word(32)),
            preparation_started: (bytes[20] & 2 != 0).then(|| word(40)),
            cutover_started: (bytes[20] & 4 != 0).then(|| word(48)),
            last_tick: word(56),
            slot_hashes: [
                bytes[64..96].try_into().unwrap(),
                bytes[96..128].try_into().unwrap(),
            ],
        };
        // Require the unique encoding, including zeroed absent optional fields.
        let mut canonical = [0; STATE_BYTES];
        state.snapshot(identity, &mut canonical)?;
        if canonical != bytes {
            return Err(invalid);
        }
        if now_ns < state.last_tick {
            return Err(OrchestratorError::DeadlineExceeded);
        }
        state.poll_deadline(now_ns)?;
        Ok(state)
    }

    fn validate_record(&self) -> Result<(), OrchestratorError> {
        use TeeUpdatePhase::*;
        let invalid = OrchestratorError::InvalidState;
        let pending = matches!(
            self.phase,
            Staged
                | Verifying
                | Prepared
                | LiveSwitch
                | RebootPending
                | HealthWindow
                | CommitPending
                | CommitUncertain
        );
        if self.pending_slot.is_some() != pending
            || self.pending_generation.is_some() != pending
            || self.reboot_required != (self.phase == RebootPending)
        {
            return Err(invalid);
        }
        if pending {
            let slot = self.pending_slot.ok_or(invalid)?;
            let previous = self.rollback_slot.ok_or(invalid)?;
            if slot == previous || self.pending_generation.ok_or(invalid)? <= self.generation {
                return Err(invalid);
            }
            let switched = matches!(self.phase, HealthWindow | CommitPending | CommitUncertain);
            if self.active_slot != if switched { slot } else { previous } {
                return Err(invalid);
            }
        }
        let prepared = pending && self.phase != Staged;
        let cutover = matches!(
            self.phase,
            LiveSwitch | HealthWindow | CommitPending | CommitUncertain
        );
        if self.preparation_started.is_some() != prepared
            || self.cutover_started.is_some() != cutover
        {
            return Err(invalid);
        }
        if self
            .preparation_started
            .is_some_and(|time| time > self.last_tick)
            || self
                .cutover_started
                .is_some_and(|time| time > self.last_tick || Some(time) < self.preparation_started)
        {
            return Err(invalid);
        }
        Ok(())
    }
}
fn slot(value: Option<SlotId>) -> u8 {
    match value {
        None => 0,
        Some(SlotId::A) => 1,
        Some(SlotId::B) => 2,
    }
}
fn read_slot(value: u8) -> Result<Option<SlotId>, OrchestratorError> {
    match value {
        0 => Ok(None),
        1 => Ok(Some(SlotId::A)),
        2 => Ok(Some(SlotId::B)),
        _ => Err(OrchestratorError::InvalidState),
    }
}
fn phase(value: TeeUpdatePhase) -> u8 {
    use TeeUpdatePhase::*;
    match value {
        Idle => 0,
        Verifying => 1,
        Staged => 2,
        Prepared => 3,
        LiveSwitch => 4,
        RebootPending => 5,
        HealthWindow => 6,
        CommitPending => 7,
        CommitUncertain => 8,
        Completed => 9,
        RolledBack => 10,
        Failed => 11,
    }
}
fn read_phase(value: u8) -> Result<TeeUpdatePhase, OrchestratorError> {
    use TeeUpdatePhase::*;
    Ok(match value {
        0 => Idle,
        1 => Verifying,
        2 => Staged,
        3 => Prepared,
        4 => LiveSwitch,
        5 => RebootPending,
        6 => HealthWindow,
        7 => CommitPending,
        8 => CommitUncertain,
        9 => Completed,
        10 => RolledBack,
        11 => Failed,
        _ => return Err(OrchestratorError::InvalidState),
    })
}
