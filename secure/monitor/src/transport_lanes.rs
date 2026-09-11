//! Independent root and normal completion slots, served by one secure worker.
//! A completed normal reply must not block authenticated root journal reads.
//! Revocation retains the worker's route until its exact late completion.
use crate::mailbox::{Mailbox, Message};
use bexos_secure_monitor_abi::{MAX_SHARED_BYTES, Status, transport::BOOT_OWNER_HANDLE};
use sha2::{Digest, Sha256};
const HEADER: usize = 32;
const MIDDLE: usize = HEADER + crate::mailbox::STATE_BYTES;
const END: usize = MIDDLE + crate::mailbox::STATE_BYTES;
pub const STATE_BYTES: usize = END + 32;

pub struct Lanes {
    normal: Mailbox,
    root: Mailbox,
    active: Option<(bool, u64)>,
}
impl Lanes {
    pub const fn new() -> Self {
        Self {
            normal: Mailbox::new(),
            root: Mailbox::new(),
            active: None,
        }
    }
    fn lane(&self, handle: u64) -> &Mailbox {
        if handle == BOOT_OWNER_HANDLE {
            &self.root
        } else {
            &self.normal
        }
    }
    fn lane_mut(&mut self, handle: u64) -> &mut Mailbox {
        if handle == BOOT_OWNER_HANDLE {
            &mut self.root
        } else {
            &mut self.normal
        }
    }
    pub fn submit(
        &mut self,
        handle: u64,
        operation: u32,
        length: usize,
        bytes: &[u8],
    ) -> Result<u64, Status> {
        self.lane_mut(handle)
            .submit(handle, operation, length, bytes)
    }
    pub fn capacity(&self, handle: u64, ticket: u64) -> Result<usize, Status> {
        self.lane(handle).capacity(handle, ticket)
    }
    pub fn poll(&mut self, handle: u64, ticket: u64, output: &mut [u8]) -> Result<i64, Status> {
        self.lane_mut(handle).poll(handle, ticket, output)
    }
    pub fn revoke(&mut self, handle: u64) {
        self.lane_mut(handle).revoke(handle);
    }
    pub fn needs_secure_progress(&self) -> bool {
        self.active.is_some()
            || self.root.needs_secure_progress()
            || self.normal.needs_secure_progress()
    }
    pub fn has_running_request(&self) -> bool {
        self.active.is_some()
    }
    pub fn fetch(&mut self, output: &mut [u8]) -> Result<Message, Status> {
        if self.active.is_some() {
            return Err(Status::Busy);
        }
        let (root, message) = match self.root.fetch(output) {
            Ok(message) => (true, message),
            Err(Status::Busy) => (false, self.normal.fetch(output)?),
            Err(error) => return Err(error),
        };
        self.active = Some((root, message.ticket));
        Ok(message)
    }
    pub fn complete(&mut self, ticket: u64, result: i64, response: &[u8]) -> Result<(), Status> {
        let (root, expected) = self.active.ok_or(Status::InvalidHandle)?;
        if ticket != expected {
            return Err(Status::InvalidHandle);
        }
        if response.len() != MAX_SHARED_BYTES as usize {
            return Err(Status::InvalidArgs);
        }
        let result = if root {
            &mut self.root
        } else {
            &mut self.normal
        }
        .complete(ticket, result, response);
        // A revoked request is still finished by this exact worker reply. Its
        // bytes cannot enter the other lane, even when ticket numbers coincide.
        self.active = None;
        result
    }
    pub fn snapshot(&self, domain: u64, output: &mut [u8]) -> Result<(), Status> {
        if output.len() != STATE_BYTES {
            return Err(Status::InvalidArgs);
        }
        output.fill(0);
        output[..8].copy_from_slice(b"BEXLM001");
        if let Some((root, ticket)) = self.active {
            output[8] = if root { 2 } else { 1 };
            output[16..24].copy_from_slice(&ticket.to_le_bytes());
        }
        self.normal.snapshot(domain, &mut output[HEADER..MIDDLE])?;
        self.root
            .snapshot(BOOT_OWNER_HANDLE, &mut output[MIDDLE..END])?;
        let digest = Sha256::digest(&output[..END]);
        output[END..].copy_from_slice(&digest);
        Ok(())
    }
    pub fn restore_protected(
        &mut self,
        domain: u64,
        input: &[u8],
        mut registered: impl FnMut(u64, usize) -> bool,
    ) -> Result<(), Status> {
        if input.len() != STATE_BYTES {
            return Err(Status::InvalidArgs);
        }
        if &input[..8] != b"BEXLM001"
            || input[9..16] != [0; 7]
            || input[24..32] != [0; 8]
            || input[END..] != Sha256::digest(&input[..END])[..]
        {
            return Err(Status::AccessDenied);
        }
        let ticket = u64::from_le_bytes(input[16..24].try_into().unwrap());
        let active = match (input[8], ticket) {
            (0, 0) => None,
            (1, 1..) => Some((false, ticket)),
            (2, 1..) => Some((true, ticket)),
            _ => return Err(Status::InvalidArgs),
        };
        let mut next = Self::new();
        next.normal
            .restore_protected(domain, &input[HEADER..MIDDLE], |h, n| {
                h != BOOT_OWNER_HANDLE && registered(h, n)
            })?;
        next.root
            .restore_protected(BOOT_OWNER_HANDLE, &input[MIDDLE..END], |h, n| {
                h == BOOT_OWNER_HANDLE && registered(h, n)
            })?;
        for (root, lane) in [(false, &next.normal), (true, &next.root)] {
            if let Some(running) = lane.running_ticket() {
                if active != Some((root, running)) {
                    return Err(Status::InvalidArgs);
                }
            }
        }
        if let Some((root, ticket)) = active {
            let lane = if root { &next.root } else { &next.normal };
            if !lane.retains_worker(ticket) {
                return Err(Status::InvalidArgs);
            }
        }
        next.active = active;
        *self = next;
        Ok(())
    }
    pub fn validate_protected(
        domain: u64,
        input: &[u8],
        registered: impl FnMut(u64, usize) -> bool,
    ) -> Result<bool, Status> {
        let mut lanes = Self::new();
        lanes.restore_protected(domain, input, registered)?;
        Ok(lanes.active.is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn root_progress_preserves_unpolled_normal_reply_and_late_revocation_route() {
        let mut lanes = Lanes::new();
        let mut worker = [0; MAX_SHARED_BYTES as usize];
        let normal = lanes.submit(7, 9, 1, &[11]).unwrap();
        assert_eq!(lanes.fetch(&mut worker).unwrap().handle, 7);
        let root = lanes
            .submit(BOOT_OWNER_HANDLE, 0x80000002, 1, &[22])
            .unwrap();
        assert_eq!(root, normal); // Independent ticket spaces deliberately collide.
        assert_eq!(lanes.fetch(&mut worker), Err(Status::Busy));
        worker[0] = 33;
        lanes.complete(normal, 1, &worker).unwrap();
        assert_eq!(lanes.fetch(&mut worker).unwrap().handle, BOOT_OWNER_HANDLE);
        assert_eq!(worker[0], 22);
        lanes.revoke(BOOT_OWNER_HANDLE);
        let mut wire = [0; STATE_BYTES];
        lanes.snapshot(2, &mut wire).unwrap();
        let mut restored = Lanes::new();
        restored
            .restore_protected(2, &wire, |h, n| h == 7 && n == 1)
            .unwrap();
        assert_eq!(
            restored.complete(root, 1, &worker),
            Err(Status::InvalidHandle)
        );
        let mut reply = [0];
        assert_eq!(restored.poll(7, normal, &mut reply), Ok(1));
        assert_eq!(reply, [33]);
        let root = restored
            .submit(BOOT_OWNER_HANDLE, 0x80000002, 1, &[44])
            .unwrap();
        assert_eq!(restored.fetch(&mut worker).unwrap().ticket, root);
        restored.complete(root, 1, &worker).unwrap();
        assert_eq!(restored.poll(BOOT_OWNER_HANDLE, root, &mut reply), Ok(1));
        assert_eq!(reply, [44]);
    }
}
