//! One bounded transaction, with immutable submission bytes and no retained
//! pointers into either guest. The caller can release its registration while
//! the secure guest runs; revocation discards any eventual completion.
use bexos_secure_monitor_abi::{MAX_SHARED_BYTES, Status};
#[path = "mailbox_state.rs"]
mod state;
pub use state::STATE_BYTES;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Message {
    pub ticket: u64,
    pub handle: u64,
    pub operation: u32,
    pub length: usize,
    pub capacity: usize,
}
#[derive(Clone, Copy, Eq, PartialEq)]
enum Phase {
    Empty,
    Submitted,
    Running,
    Completed,
}
pub struct Mailbox {
    message: Message,
    phase: Phase,
    next_ticket: u64,
    result: i64,
    bytes: [u8; MAX_SHARED_BYTES as usize],
}
impl Mailbox {
    pub(crate) fn running_ticket(&self) -> Option<u64> {
        (self.phase == Phase::Running).then_some(self.message.ticket)
    }
    pub(crate) fn retains_worker(&self, ticket: u64) -> bool {
        ticket != 0
            && ticket < self.next_ticket
            && match self.phase {
                Phase::Empty => true,
                Phase::Submitted => self.message.ticket > ticket,
                Phase::Running => self.message.ticket == ticket,
                Phase::Completed => false,
            }
    }
    pub fn needs_secure_progress(&self) -> bool {
        matches!(self.phase, Phase::Submitted | Phase::Running)
    }
    pub fn has_running_request(&self) -> bool {
        self.phase == Phase::Running
    }
    pub const fn new() -> Self {
        Self {
            message: Message {
                ticket: 0,
                handle: 0,
                operation: 0,
                length: 0,
                capacity: 0,
            },
            phase: Phase::Empty,
            next_ticket: 1,
            result: 0,
            bytes: [0; MAX_SHARED_BYTES as usize],
        }
    }
    pub fn submit(
        &mut self,
        handle: u64,
        operation: u32,
        length: usize,
        snapshot: &[u8],
    ) -> Result<u64, Status> {
        if self.phase != Phase::Empty {
            return Err(Status::Busy);
        }
        if handle == 0
            || operation == 0
            || length == 0
            || length > snapshot.len()
            || snapshot.len() > self.bytes.len()
        {
            return Err(Status::InvalidArgs);
        }
        let ticket = self.next_ticket;
        self.next_ticket = ticket.checked_add(1).ok_or(Status::NoResources)?;
        self.bytes.fill(0);
        self.bytes[..snapshot.len()].copy_from_slice(snapshot);
        self.message = Message {
            ticket,
            handle,
            operation,
            length,
            capacity: snapshot.len(),
        };
        self.phase = Phase::Submitted;
        Ok(ticket)
    }
    pub fn fetch(&mut self, destination: &mut [u8]) -> Result<Message, Status> {
        if self.phase != Phase::Submitted {
            return Err(Status::Busy);
        }
        if destination.len() != self.bytes.len() {
            return Err(Status::InvalidArgs);
        }
        destination.copy_from_slice(&self.bytes);
        self.phase = Phase::Running;
        Ok(self.message)
    }
    pub fn complete(&mut self, ticket: u64, result: i64, response: &[u8]) -> Result<(), Status> {
        if self.phase != Phase::Running || ticket != self.message.ticket {
            return Err(Status::InvalidHandle);
        }
        if response.len() != self.bytes.len() {
            return Err(Status::InvalidArgs);
        }
        self.bytes[..self.message.capacity].copy_from_slice(&response[..self.message.capacity]);
        self.result = result;
        self.phase = Phase::Completed;
        Ok(())
    }
    pub fn capacity(&self, handle: u64, ticket: u64) -> Result<usize, Status> {
        if self.phase == Phase::Empty
            || self.message.handle != handle
            || self.message.ticket != ticket
        {
            return Err(Status::InvalidHandle);
        }
        Ok(self.message.capacity)
    }
    pub fn poll(
        &mut self,
        handle: u64,
        ticket: u64,
        destination: &mut [u8],
    ) -> Result<i64, Status> {
        let capacity = self.capacity(handle, ticket)?;
        if destination.len() != capacity {
            return Err(Status::InvalidArgs);
        }
        if self.phase != Phase::Completed {
            return Err(Status::Busy);
        }
        destination.copy_from_slice(&self.bytes[..capacity]);
        let result = self.result;
        self.clear();
        Ok(result)
    }
    pub fn revoke(&mut self, handle: u64) {
        if self.phase != Phase::Empty && self.message.handle == handle {
            self.clear();
        }
    }
    fn clear(&mut self) {
        self.bytes.fill(0);
        self.phase = Phase::Empty;
        self.result = 0;
        self.message = Message {
            ticket: 0,
            handle: 0,
            operation: 0,
            length: 0,
            capacity: 0,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn revocation_fences_late_writes_and_stale_tickets() {
        let mut m = Mailbox::new();
        let mut secure = [0; MAX_SHARED_BYTES as usize];
        let mut normal = [9; 4096];
        let first = m.submit(1, 7, 16, &normal).unwrap();
        normal.fill(3);
        assert!(m.needs_secure_progress());
        assert_eq!(m.fetch(&mut secure).unwrap().ticket, first);
        assert!(m.needs_secure_progress());
        assert_eq!(secure[0], 9);
        assert_eq!(secure[4096], 0);
        m.revoke(1);
        assert!(!m.needs_secure_progress());
        assert_eq!(m.complete(first, 0, &secure), Err(Status::InvalidHandle));
        assert_eq!(m.poll(1, first, &mut normal), Err(Status::InvalidHandle));
        assert_eq!(normal, [3; 4096]);
        let second = m.submit(1, 7, 16, &normal).unwrap();
        assert_ne!(first, second);
        assert_eq!(m.poll(1, second, &mut normal), Err(Status::Busy));
        m.fetch(&mut secure).unwrap();
        secure[0] = 42;
        m.complete(second, -17, &secure).unwrap();
        assert!(!m.needs_secure_progress());
        assert_eq!(m.poll(2, second, &mut normal), Err(Status::InvalidHandle));
        assert_eq!(m.poll(1, first, &mut normal), Err(Status::InvalidHandle));
        assert_eq!(m.poll(1, second, &mut normal), Ok(-17));
        assert_eq!(normal[0], 42);
        assert_eq!(m.poll(1, second, &mut normal), Err(Status::InvalidHandle));
    }
}
