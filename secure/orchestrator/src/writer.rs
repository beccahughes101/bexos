//! Resident single-writer authority. The execution owner must authorize every
//! persistent mutation here before any frame reaches the backing device.
//! Tickets and watermarks are protected state, never supplied by either guest.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Denied,
    Busy,
    Stale,
    Exhausted,
    Uncertain,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Outcome {
    Committed,
    NotCommitted,
    Unknown,
}

#[derive(Debug, Eq, PartialEq)]
pub struct Ticket {
    epoch: u64,
    sequence: u64,
}

pub struct Writer {
    owner: u64,
    epoch: u64,
    next: u64,
    watermark: u64,
    pending: Option<(u64, u64)>,
    frozen: bool,
    uncertain: bool,
}
impl Writer {
    pub fn new(owner: u64, epoch: u64, watermark: u64) -> Result<Self, Error> {
        if owner == 0 || epoch == 0 {
            return Err(Error::Denied);
        }
        Ok(Self {
            owner,
            epoch,
            next: 1,
            watermark,
            pending: None,
            frozen: false,
            uncertain: false,
        })
    }
    pub fn owner(&self) -> u64 {
        self.owner
    }
    pub fn epoch(&self) -> u64 {
        self.epoch
    }
    pub fn watermark(&self) -> u64 {
        self.watermark
    }
    /// Recover the resident ticket after restoring protected state. This does
    /// not authorize resubmission of the write: obtain its authenticated outcome
    /// and pass that observation to `resolve` instead.
    pub fn pending_ticket(&self) -> Option<Ticket> {
        self.pending
            .map(|(epoch, sequence)| Ticket { epoch, sequence })
    }
    pub fn drained(&self) -> bool {
        self.pending.is_none() && !self.uncertain
    }
    pub fn frozen(&self) -> bool {
        self.frozen
    }

    pub fn begin(&mut self, owner: u64, epoch: u64) -> Result<Ticket, Error> {
        if owner != self.owner || epoch != self.epoch {
            return Err(Error::Denied);
        }
        if self.uncertain {
            return Err(Error::Uncertain);
        }
        if self.frozen || self.pending.is_some() {
            return Err(Error::Busy);
        }
        let next = self.next.checked_add(1).ok_or(Error::Exhausted)?;
        self.watermark.checked_add(1).ok_or(Error::Exhausted)?;
        let ticket = Ticket {
            epoch,
            sequence: self.next,
        };
        self.pending = Some((epoch, self.next));
        self.next = next;
        Ok(ticket)
    }
    /// A lost write acknowledgement does not consume its ticket. Keep the
    /// mutation fenced until an authenticated observation resolves it.
    pub fn finish(&mut self, ticket: &Ticket, outcome: Outcome) -> Result<(), Error> {
        if self.pending != Some((ticket.epoch, ticket.sequence)) {
            return Err(Error::Stale);
        }
        if self.uncertain {
            return Err(Error::Uncertain);
        }
        self.apply(outcome)
    }
    pub fn resolve(&mut self, ticket: &Ticket, outcome: Outcome) -> Result<(), Error> {
        if self.pending != Some((ticket.epoch, ticket.sequence)) {
            return Err(Error::Stale);
        }
        if !self.uncertain {
            return Err(Error::Stale);
        }
        self.apply(outcome)
    }
    fn apply(&mut self, outcome: Outcome) -> Result<(), Error> {
        match outcome {
            Outcome::Unknown => {
                self.uncertain = true;
                self.frozen = true;
                Err(Error::Uncertain)
            }
            Outcome::Committed => {
                self.watermark = self.watermark.checked_add(1).ok_or(Error::Exhausted)?;
                self.pending = None;
                self.uncertain = false;
                Ok(())
            }
            Outcome::NotCommitted => {
                self.pending = None;
                self.uncertain = false;
                Ok(())
            }
        }
    }
    /// Close admission before waiting for the current frame transaction.
    pub fn freeze(&mut self) {
        self.frozen = true;
    }
    pub fn resume(&mut self) -> Result<(), Error> {
        if self.uncertain {
            return Err(Error::Uncertain);
        }
        self.frozen = false;
        Ok(())
    }
    /// Call only after authenticated durable image commitment. Preparation
    /// cannot grant write authority, even when a candidate reports healthy.
    pub(crate) fn transfer(&mut self, candidate: u64) -> Result<(), Error> {
        if candidate == 0 || candidate == self.owner {
            return Err(Error::Denied);
        }
        if !self.frozen || !self.drained() {
            return Err(Error::Busy);
        }
        let epoch = self.epoch.checked_add(1).ok_or(Error::Exhausted)?;
        self.owner = candidate;
        self.epoch = epoch;
        self.frozen = false;
        Ok(())
    }
}

impl Writer {
    pub(crate) fn snapshot(&self) -> [u8; 64] {
        let mut out = [0; 64];
        for (offset, value) in [
            (0, self.owner),
            (8, self.epoch),
            (16, self.next),
            (24, self.watermark),
            (32, self.pending.map_or(0, |p| p.0)),
            (40, self.pending.map_or(0, |p| p.1)),
        ] {
            out[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
        }
        out[48] = u8::from(self.frozen);
        out[49] = u8::from(self.uncertain);
        out
    }
    pub(crate) fn restore(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() != 64 || bytes[48] > 1 || bytes[49] > 1 || bytes[50..] != [0; 14] {
            return Err(Error::Stale);
        }
        let word = |i| u64::from_le_bytes(bytes[i..i + 8].try_into().unwrap());
        let owner = word(0);
        let epoch = word(8);
        let next = word(16);
        let pending = match (word(32), word(40)) {
            (0, 0) => None,
            (e, sequence)
                if e == epoch && sequence != 0 && sequence.checked_add(1) == Some(next) =>
            {
                Some((e, sequence))
            }
            _ => return Err(Error::Stale),
        };
        if owner == 0
            || epoch == 0
            || next == 0
            || (bytes[49] != 0 && (bytes[48] == 0 || pending.is_none()))
        {
            return Err(Error::Stale);
        }
        Ok(Self {
            owner,
            epoch,
            next,
            watermark: word(24),
            pending,
            frozen: bytes[48] != 0,
            uncertain: bytes[49] != 0,
        })
    }
}
