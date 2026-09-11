//! Canonical pending-transport state for a protected monitor handoff. A running
//! message must travel with its secure vCPU and registered-memory checkpoints;
//! it must never be resubmitted to a newly booted secure runtime.
use super::{Mailbox, Message, Phase};
use bexos_secure_monitor_abi::{MAX_SHARED_BYTES, Status};
use sha2::{Digest, Sha256};

const HEADER: usize = 80;
const END: usize = HEADER + MAX_SHARED_BYTES as usize;
pub const STATE_BYTES: usize = END + 32;

impl Mailbox {
    pub fn snapshot(&self, domain: u64, output: &mut [u8]) -> Result<(), Status> {
        if domain == 0 || output.len() != STATE_BYTES {
            return Err(Status::InvalidArgs);
        }
        output.fill(0);
        output[..8].copy_from_slice(b"BEXMB001");
        output[8..16].copy_from_slice(&domain.to_le_bytes());
        output[16..24].copy_from_slice(&self.next_ticket.to_le_bytes());
        output[24] = match self.phase {
            Phase::Empty => 0,
            Phase::Submitted => 1,
            Phase::Running => 2,
            Phase::Completed => 3,
        };
        if self.phase != Phase::Empty {
            output[32..40].copy_from_slice(&self.message.ticket.to_le_bytes());
            output[40..48].copy_from_slice(&self.message.handle.to_le_bytes());
            output[48..52].copy_from_slice(&self.message.operation.to_le_bytes());
            output[52..56].copy_from_slice(&(self.message.length as u32).to_le_bytes());
            output[56..60].copy_from_slice(&(self.message.capacity as u32).to_le_bytes());
            output[64..72].copy_from_slice(&self.result.to_le_bytes());
            output[HEADER..END].copy_from_slice(&self.bytes);
        }
        let digest = Sha256::digest(&output[..END]);
        output[END..].copy_from_slice(&digest);
        Ok(())
    }

    /// The caller must establish protected resident-memory provenance. The
    /// digest detects damage, not forgery. `registered` checks the restored
    /// owner's exact live mapping capacity before any pending reply is adopted.
    /// Destination state remains unchanged on failure.
    pub fn restore_protected(
        &mut self,
        domain: u64,
        input: &[u8],
        registered: impl FnOnce(u64, usize) -> bool,
    ) -> Result<(), Status> {
        let (phase, message, result, next_ticket) = Self::validate(domain, input, registered)?;
        self.next_ticket = next_ticket;
        self.message = message;
        self.result = result;
        self.bytes.copy_from_slice(&input[HEADER..END]);
        self.phase = phase;
        Ok(())
    }

    /// Validate as part of a larger atomic handoff without allocating another
    /// 64 KiB mailbox on the monitor's stack. Returns whether secure code owns
    /// a fetched request, which requires retaining its completion buffer.
    pub fn validate_protected(
        domain: u64,
        input: &[u8],
        registered: impl FnOnce(u64, usize) -> bool,
    ) -> Result<bool, Status> {
        Self::validate(domain, input, registered).map(|state| state.0 == Phase::Running)
    }

    fn validate(
        domain: u64,
        input: &[u8],
        registered: impl FnOnce(u64, usize) -> bool,
    ) -> Result<(Phase, Message, i64, u64), Status> {
        if input.len() != STATE_BYTES || domain == 0 || &input[..8] != b"BEXMB001" {
            return Err(Status::InvalidArgs);
        }
        let word = |offset| u64::from_le_bytes(input[offset..offset + 8].try_into().unwrap());
        let small = |offset| u32::from_le_bytes(input[offset..offset + 4].try_into().unwrap());
        if word(8) != domain
            || word(16) == 0
            || input[25..32] != [0; 7]
            || input[60..64] != [0; 4]
            || input[72..80] != [0; 8]
            || input[END..] != Sha256::digest(&input[..END])[..]
        {
            return Err(Status::AccessDenied);
        }
        let phase = match input[24] {
            0 => Phase::Empty,
            1 => Phase::Submitted,
            2 => Phase::Running,
            3 => Phase::Completed,
            _ => return Err(Status::InvalidArgs),
        };
        let message = Message {
            ticket: word(32),
            handle: word(40),
            operation: small(48),
            length: small(52) as usize,
            capacity: small(56) as usize,
        };
        let result = word(64) as i64;
        if phase == Phase::Empty {
            if input[32..END].iter().any(|byte| *byte != 0) {
                return Err(Status::InvalidArgs);
            }
        } else if message.ticket == 0
            || message.ticket >= word(16)
            || message.handle == 0
            || message.operation == 0
            || message.length == 0
            || message.length > message.capacity
            || message.capacity > MAX_SHARED_BYTES as usize
            || (phase != Phase::Completed && result != 0)
            || input[HEADER + message.capacity..END]
                .iter()
                .any(|byte| *byte != 0)
            || !registered(message.handle, message.capacity)
        {
            return Err(Status::InvalidArgs);
        }
        Ok((phase, message, result, word(16)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_pending_request_completion_and_ticket_identity() {
        for phase in 0..4 {
            let mut old = Mailbox::new();
            let mut secure = [0; MAX_SHARED_BYTES as usize];
            let ticket = old.submit(9, 7, 4, &[3; 4096]).unwrap();
            if phase >= 2 {
                old.fetch(&mut secure).unwrap();
            }
            if phase == 3 {
                secure[0] = 42;
                old.complete(ticket, -17, &secure).unwrap();
            }
            if phase == 0 {
                old.revoke(9);
            }
            let mut record = [0; STATE_BYTES];
            old.snapshot(5, &mut record).unwrap();
            let mut new = Mailbox::new();
            new.restore_protected(5, &record, |owner, size| owner == 9 && size == 4096)
                .unwrap();
            if phase == 0 {
                assert!(new.submit(9, 7, 4, &[3; 4096]).unwrap() > ticket);
                continue;
            }
            if phase == 1 {
                assert_eq!(new.fetch(&mut secure).unwrap().ticket, ticket);
            }
            if phase <= 2 {
                assert_eq!(secure[0], 3);
                assert_eq!(new.fetch(&mut secure), Err(Status::Busy));
                new.complete(ticket, -17, &secure).unwrap();
            }
            let mut response = [0; 4096];
            assert_eq!(new.poll(9, ticket, &mut response), Ok(-17));
            assert_eq!(response[0], if phase == 3 { 42 } else { 3 });
            assert!(new.submit(9, 7, 4, &response).unwrap() > ticket);
        }
    }
    #[test]
    fn rejects_wrong_owner_mapping_corruption_and_invalid_phase_atomically() {
        let mut source = Mailbox::new();
        source.submit(9, 7, 4, &[3; 4096]).unwrap();
        let mut valid = [0; STATE_BYTES];
        source.snapshot(5, &mut valid).unwrap();
        let mut target = Mailbox::new();
        let mut before = [0; STATE_BYTES];
        target.snapshot(5, &mut before).unwrap();
        assert!(target.restore_protected(6, &valid, |_, _| true).is_err());
        assert!(target.restore_protected(5, &valid, |_, _| false).is_err());
        for (offset, value) in [
            (16, 0),
            (24, 4),
            (25, 1),
            (32, 0),
            (40, 0),
            (48, 0),
            (52, 0),
            (60, 1),
            (64, 1),
            (72, 1),
            (HEADER + 4096, 1),
        ] {
            let mut damaged = valid;
            damaged[offset] = value;
            // Even internally checksummed but incompatible records fail.
            let digest = Sha256::digest(&damaged[..END]);
            damaged[END..].copy_from_slice(&digest);
            assert!(
                target.restore_protected(5, &damaged, |_, _| true).is_err(),
                "offset {offset}"
            );
        }
        valid[HEADER] ^= 1;
        assert!(target.restore_protected(5, &valid, |_, _| true).is_err());
        let mut after = [0; STATE_BYTES];
        target.snapshot(5, &mut after).unwrap();
        assert_eq!(before, after);
    }
}
