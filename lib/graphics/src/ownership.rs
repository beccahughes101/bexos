use crate::Error;
/// The display service serializes this state with all device submissions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Transfer {
    pub previous: u64,
    pub next: u64,
    pub deadline_us: u64,
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Ownership {
    pub owner: u64,
    pub generation: u64,
    pub pending: Option<Transfer>,
}
impl Ownership {
    pub fn acquire(&mut self, client: u64) -> Result<u64, Error> {
        if client == 0 {
            return Err(Error::Invalid);
        }
        if self.owner == client && self.pending.is_none() {
            return Ok(self.generation);
        }
        if self.owner != 0 {
            return Err(Error::Busy);
        }
        self.generation = self.generation.checked_add(1).ok_or(Error::Bounds)?;
        self.owner = client;
        Ok(self.generation)
    }
    pub fn authorize(&self, client: u64, generation: u64) -> Result<(), Error> {
        if self.owner != client || client == 0 {
            return Err(Error::Denied);
        }
        if self.generation != generation {
            return Err(Error::Stale);
        }
        Ok(())
    }
    pub fn begin(
        &mut self,
        client: u64,
        generation: u64,
        next: u64,
        now: u64,
    ) -> Result<u64, Error> {
        self.authorize(client, generation)?;
        if next == 0 || next == client {
            return Err(Error::Invalid);
        }
        if self.pending.is_some() {
            return Err(Error::Busy);
        }
        let next_generation = self.generation.checked_add(1).ok_or(Error::Bounds)?;
        self.pending = Some(Transfer {
            previous: client,
            next,
            deadline_us: now.saturating_add(2_000_000),
        });
        self.owner = next;
        self.generation = next_generation;
        Ok(next_generation)
    }
    /// Call only after the first new-owner frame has completed on the device.
    pub fn complete(&mut self, client: u64, generation: u64) -> Result<(), Error> {
        self.authorize(client, generation)?;
        if self.pending.is_none() {
            return Err(Error::Stale);
        }
        self.pending = None;
        Ok(())
    }
    pub fn rollback(&mut self) -> Result<u64, Error> {
        let pending = self.pending.ok_or(Error::Stale)?;
        let generation = self.generation.checked_add(1).ok_or(Error::Bounds)?;
        self.owner = pending.previous;
        self.generation = generation;
        self.pending = None;
        Ok(generation)
    }
    pub fn expire(&mut self, now: u64) -> bool {
        if self.pending.is_some_and(|p| now >= p.deadline_us) {
            return self.rollback().is_ok();
        }
        false
    }
    pub fn disconnected(&mut self, client: u64) {
        if self.pending.is_some_and(|p| p.next == client) {
            let _ = self.rollback();
        } else if self.pending.is_some_and(|p| p.previous == client) {
            self.pending = None;
        } else if self.owner == client {
            self.owner = 0;
        }
    }
}
