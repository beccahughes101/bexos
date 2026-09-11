use crate::spec::NvmeStatus;
use alloc::vec;
use alloc::vec::Vec;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Submission {
    pub command_id: u16,
    pub opcode: u8,
    pub namespace_id: u32,
    pub prp1: u64,
    pub prp2: u64,
    pub cdw10: u32,
    pub cdw11: u32,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Completion {
    pub command_id: u16,
    pub status: u16,
    pub phase: bool,
}

impl Completion {
    pub const fn decoded_status(self) -> NvmeStatus {
        NvmeStatus::from_completion(self.status)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum QueueError {
    Full,
    Empty,
    CommandIdMismatch { expected: u16, actual: u16 },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NvmeQueue {
    submissions: Vec<Option<Submission>>,
    completions: Vec<Option<Completion>>,
    submission_tail: u16,
    completion_head: u16,
    completion_phase: bool,
}

impl NvmeQueue {
    pub fn new(depth: u16) -> Self {
        assert!(depth > 1, "NVMe queue depth must leave an empty slot");
        Self {
            submissions: vec![None; depth as usize],
            completions: vec![None; depth as usize],
            submission_tail: 0,
            completion_head: 0,
            completion_phase: true,
        }
    }

    pub fn submit(&mut self, submission: Submission) -> Result<u16, QueueError> {
        let slot = self.submission_tail as usize;
        if self.submissions[slot].is_some() {
            return Err(QueueError::Full);
        }
        self.submissions[slot] = Some(submission);
        self.submission_tail = self.next(self.submission_tail);
        Ok(slot as u16)
    }

    pub fn inject_completion(&mut self, index: u16, completion: Completion) {
        self.completions[index as usize] = Some(completion);
    }

    pub fn complete_next(&mut self, expected_command_id: u16) -> Result<Completion, QueueError> {
        let index = self.completion_head as usize;
        let completion = self.completions[index].take().ok_or(QueueError::Empty)?;
        if completion.phase != self.completion_phase {
            self.completions[index] = Some(completion);
            return Err(QueueError::Empty);
        }
        if completion.command_id != expected_command_id {
            return Err(QueueError::CommandIdMismatch {
                expected: expected_command_id,
                actual: completion.command_id,
            });
        }

        self.submissions[index] = None;
        self.completion_head = self.next(self.completion_head);
        if self.completion_head == 0 {
            self.completion_phase = !self.completion_phase;
        }
        Ok(completion)
    }

    pub const fn submission_tail(&self) -> u16 {
        self.submission_tail
    }

    pub const fn completion_head(&self) -> u16 {
        self.completion_head
    }

    fn next(&self, value: u16) -> u16 {
        (value + 1) % self.submissions.len() as u16
    }
}
