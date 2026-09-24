#![no_std]
extern crate alloc;

use alloc::vec::Vec;
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};

pub const JOURNAL_VERSION: u64 = 1;
pub const MAX_BUFFERS: usize = 256;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlaneKind {
    Ethernet,
    Block,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RetainedBuffer {
    pub buffer_id: u32,
    pub vmo: u64,
    pub size_bytes: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DataPlaneJournal {
    pub node_id: u64,
    pub generation: u64,
    pub kind: PlaneKind,
    pub request_fifo_provider: u64,
    pub completion_fifo_provider: Option<u64>,
    pub started: bool,
    buffers: Vec<RetainedBuffer>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JournalError {
    InvalidHandle,
    DuplicateBuffer,
    UnknownBuffer,
    Capacity,
    WrongKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReplayState {
    pub node_id: u64,
    pub generation: u64,
    pub request_fifo_provider: u64,
    pub completion_fifo_provider: Option<u64>,
    pub started: bool,
    pub buffers: Vec<RetainedBuffer>,
}

impl DataPlaneJournal {
    pub fn ethernet(node_id: u64, fifo_provider: u64) -> Result<Self, JournalError> {
        Self::new(PlaneKind::Ethernet, node_id, fifo_provider, None)
    }

    pub fn block(
        node_id: u64,
        request_fifo_provider: u64,
        completion_fifo_provider: u64,
    ) -> Result<Self, JournalError> {
        Self::new(
            PlaneKind::Block,
            node_id,
            request_fifo_provider,
            Some(completion_fifo_provider),
        )
    }

    fn new(
        kind: PlaneKind,
        node_id: u64,
        request_fifo_provider: u64,
        completion_fifo_provider: Option<u64>,
    ) -> Result<Self, JournalError> {
        if node_id == 0
            || request_fifo_provider == 0
            || completion_fifo_provider == Some(0)
            || (kind == PlaneKind::Block) != completion_fifo_provider.is_some()
        {
            return Err(JournalError::InvalidHandle);
        }
        Ok(Self {
            node_id,
            generation: 1,
            kind,
            request_fifo_provider,
            completion_fifo_provider,
            started: false,
            buffers: Vec::new(),
        })
    }

    pub fn buffers(&self) -> &[RetainedBuffer] {
        &self.buffers
    }

    pub fn register_buffer(&mut self, buffer: RetainedBuffer) -> Result<(), JournalError> {
        if buffer.vmo == 0 || buffer.size_bytes == 0 {
            return Err(JournalError::InvalidHandle);
        }
        if self
            .buffers
            .iter()
            .any(|entry| entry.buffer_id == buffer.buffer_id)
        {
            return Err(JournalError::DuplicateBuffer);
        }
        if self.buffers.len() >= MAX_BUFFERS {
            return Err(JournalError::Capacity);
        }
        self.buffers.push(buffer);
        self.buffers.sort_by_key(|entry| entry.buffer_id);
        self.generation = self.generation.saturating_add(1);
        Ok(())
    }

    pub fn unregister_buffer(&mut self, buffer_id: u32) -> Result<RetainedBuffer, JournalError> {
        let index = self
            .buffers
            .iter()
            .position(|entry| entry.buffer_id == buffer_id)
            .ok_or(JournalError::UnknownBuffer)?;
        self.generation = self.generation.saturating_add(1);
        Ok(self.buffers.remove(index))
    }

    pub fn set_started(&mut self, started: bool) {
        if self.started != started {
            self.started = started;
            self.generation = self.generation.saturating_add(1);
        }
    }

    /// The replay descriptor deliberately contains the same provider endpoint
    /// and VMO handles. A recovering driver adopts them rather than creating
    /// client-visible replacements.
    pub fn replay(&self) -> ReplayState {
        ReplayState {
            node_id: self.node_id,
            generation: self.generation,
            request_fifo_provider: self.request_fifo_provider,
            completion_fifo_provider: self.completion_fifo_provider,
            started: self.started,
            buffers: self.buffers.clone(),
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut writer = Encoder::new();
        writer.word(JOURNAL_VERSION);
        writer.word(match self.kind {
            PlaneKind::Ethernet => 1,
            PlaneKind::Block => 2,
        });
        writer.word(self.node_id);
        writer.word(self.generation);
        writer.word(self.request_fifo_provider);
        writer.word(self.completion_fifo_provider.is_some() as u64);
        writer.word(self.completion_fifo_provider.unwrap_or(0));
        writer.word(self.started as u64);
        writer.word(self.buffers.len() as u64);
        for buffer in &self.buffers {
            writer.word(buffer.buffer_id as u64);
            writer.word(buffer.vmo);
            writer.word(buffer.size_bytes);
        }
        writer.finish()
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut reader = Decoder::new(bytes);
        if reader.word()? != JOURNAL_VERSION {
            return Err(Error::UnsupportedVersion);
        }
        let kind = match reader.word()? {
            1 => PlaneKind::Ethernet,
            2 => PlaneKind::Block,
            _ => return Err(Error::InvalidData),
        };
        let node_id = reader.word()?;
        let generation = reader.word()?;
        let request_fifo_provider = reader.word()?;
        let has_completion = reader.flag()?;
        let completion = reader.word()?;
        let completion_fifo_provider = has_completion.then_some(completion);
        let started = reader.flag()?;
        let mut journal = Self::new(
            kind,
            node_id,
            request_fifo_provider,
            completion_fifo_provider,
        )
        .map_err(|_| Error::InvalidData)?;
        journal.generation = generation;
        journal.started = started;
        for _ in 0..reader.count(MAX_BUFFERS)? {
            journal
                .register_buffer(RetainedBuffer {
                    buffer_id: u32::try_from(reader.word()?).map_err(|_| Error::InvalidData)?,
                    vmo: reader.word()?,
                    size_bytes: reader.word()?,
                })
                .map_err(|_| Error::InvalidData)?;
        }
        journal.generation = generation;
        reader.finish()?;
        Ok(journal)
    }
}
