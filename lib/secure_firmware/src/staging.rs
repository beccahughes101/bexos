//! Bounded sequential transfer into owner-private memory. No guest pointer or
//! mutable reference survives a write. Successful sealing freezes the snapshot.
use crate::{Error, MAX_BUNDLE_BYTES};

pub const PREPARATION_NS: u64 = 30_000_000_000;
pub struct Staging<'a> {
    memory: &'a mut [u8],
    owner: u64,
    length: usize,
    received: usize,
    started: u64,
    sealed: bool,
}
impl<'a> Staging<'a> {
    pub fn started_ns(&self) -> u64 {
        self.started
    }
    pub fn expired(&self, now: u64) -> bool {
        now < self.started || now - self.started >= PREPARATION_NS
    }
    pub fn begin(memory: &'a mut [u8], owner: u64, length: usize, now: u64) -> Result<Self, Error> {
        if owner == 0 {
            return Err(Error::Owner);
        }
        if length == 0 || length > MAX_BUNDLE_BYTES || length > memory.len() {
            return Err(Error::Bounds);
        }
        Ok(Self {
            memory,
            owner,
            length,
            received: 0,
            started: now,
            sealed: false,
        })
    }
    fn check(&self, owner: u64, now: u64) -> Result<(), Error> {
        if owner != self.owner {
            return Err(Error::Owner);
        }
        if now < self.started || now - self.started >= PREPARATION_NS {
            return Err(Error::Deadline);
        }
        Ok(())
    }
    pub fn write(
        &mut self,
        owner: u64,
        offset: usize,
        bytes: &[u8],
        now: u64,
    ) -> Result<(), Error> {
        self.check(owner, now)?;
        if self.sealed {
            return Err(Error::Busy);
        }
        if offset != self.received {
            return Err(Error::Order);
        }
        if bytes.is_empty() || bytes.len() > 65536 {
            return Err(Error::Bounds);
        }
        let end = offset
            .checked_add(bytes.len())
            .filter(|n| *n <= self.length)
            .ok_or(Error::Bounds)?;
        self.memory[offset..end].copy_from_slice(bytes);
        self.received = end;
        Ok(())
    }
    pub fn seal(&mut self, owner: u64, now: u64) -> Result<&[u8], Error> {
        self.check(owner, now)?;
        if self.received != self.length {
            return Err(Error::Order);
        }
        self.sealed = true;
        Ok(&self.memory[..self.length])
    }
    /// Consume an already sealed upload into the resident storage pipeline.
    /// The returned bytes remain root-owned and must be authenticated again
    /// after a disk reread; the receiver assumes responsibility for erasure.
    pub fn release(mut self, owner: u64, now: u64) -> Result<&'a mut [u8], Error> {
        self.check(owner, now)?;
        if !self.sealed || self.received != self.length {
            return Err(Error::Order);
        }
        let memory = core::mem::take(&mut self.memory);
        self.received = 0;
        Ok(&mut memory[..self.length])
    }
}
impl Drop for Staging<'_> {
    fn drop(&mut self) {
        self.memory[..self.received].fill(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn transfer_owns_bytes_fences_callers_and_reclaims_on_abort() {
        let mut memory = [0u8; 12];
        {
            let mut stage = Staging::begin(&mut memory, 7, 8, 100).unwrap();
            let mut source = [1u8; 4];
            assert_eq!(stage.write(8, 0, &source, 101), Err(Error::Owner));
            assert_eq!(stage.write(7, 4, &source, 101), Err(Error::Order));
            stage.write(7, 0, &source, 101).unwrap();
            source.fill(9);
            assert_eq!(stage.seal(7, 102), Err(Error::Order));
            stage.write(7, 4, &source, 102).unwrap();
            assert_eq!(stage.seal(7, 103).unwrap(), &[1, 1, 1, 1, 9, 9, 9, 9]);
            assert_eq!(stage.write(7, 8, &source, 104), Err(Error::Busy));
        }
        assert_eq!(memory, [0; 12]);
    }
    #[test]
    fn preparation_budget_cannot_be_restarted_by_chunks_or_clock_regression() {
        let mut memory = [0u8; 8];
        let mut stage = Staging::begin(&mut memory, 1, 8, 100).unwrap();
        assert_eq!(stage.write(1, 0, &[1; 4], 99), Err(Error::Deadline));
        stage
            .write(1, 0, &[1; 4], 100 + PREPARATION_NS - 1)
            .unwrap();
        assert_eq!(
            stage.write(1, 4, &[2; 4], 100 + PREPARATION_NS),
            Err(Error::Deadline)
        );
        assert_eq!(stage.seal(1, 100 + PREPARATION_NS), Err(Error::Deadline));
    }
    #[test]
    fn release_requires_sealing_and_transfers_erasure_responsibility() {
        let mut memory = [0; 8];
        let mut staging = Staging::begin(&mut memory, 1, 4, 0).unwrap();
        staging.write(1, 0, &[1; 4], 1).unwrap();
        assert_eq!(staging.release(1, 2), Err(Error::Order));
        assert_eq!(memory, [0; 8]);
        let mut staging = Staging::begin(&mut memory, 1, 4, 0).unwrap();
        staging.write(1, 0, &[2; 4], 1).unwrap();
        staging.seal(1, 2).unwrap();
        let owned = staging.release(1, 3).unwrap();
        assert_eq!(owned, [2; 4]);
        owned.fill(0);
        assert_eq!(memory, [0; 8]);
    }
}
