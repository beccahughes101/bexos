//! Packet requests retained while hardware queues are in flight.
use alloc::collections::VecDeque;
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
use bexos_userspace::Channel;
use ethernet_fidl::{FidlDecode, FidlEncode, FrameEntry, FrameOpcode};

pub const MAX_PENDING: usize = 128;
pub struct PendingFrames {
    pub rx: VecDeque<(Channel, FrameEntry)>,
    pub tx: VecDeque<(Channel, FrameEntry)>,
    pub transmitting: Option<(Channel, FrameEntry)>,
}

impl Default for PendingFrames {
    fn default() -> Self {
        Self {
            rx: VecDeque::new(),
            tx: VecDeque::new(),
            transmitting: None,
        }
    }
}

impl PendingFrames {
    pub fn encode(&self, w: &mut Encoder) -> Result<(), Error> {
        w.word(self.rx.len() as u64);
        for frame in &self.rx {
            encode_frame(w, *frame)?;
        }
        w.word(self.tx.len() as u64);
        for frame in &self.tx {
            encode_frame(w, *frame)?;
        }
        w.word(self.transmitting.is_some() as u64);
        if let Some(frame) = self.transmitting {
            encode_frame(w, frame)?;
        }
        Ok(())
    }

    pub fn decode(r: &mut Decoder<'_>) -> Result<Self, Error> {
        let mut this = Self::default();
        for _ in 0..r.count(MAX_PENDING)? {
            this.rx.push_back(decode_frame(r, FrameOpcode::RxSupply)?);
        }
        for _ in 0..r.count(MAX_PENDING)? {
            this.tx.push_back(decode_frame(r, FrameOpcode::TxSend)?);
        }
        if r.flag()? {
            this.transmitting = Some(decode_frame(r, FrameOpcode::TxSend)?);
        }
        Ok(this)
    }
}

fn encode_frame(w: &mut Encoder, (channel, entry): (Channel, FrameEntry)) -> Result<(), Error> {
    w.word(channel.0);
    let mut bytes = [0; 64];
    let encoded = entry
        .encode(&mut bytes, &mut [])
        .map_err(|_| Error::InvalidData)?;
    w.bytes(&bytes[..encoded.bytes]);
    Ok(())
}

fn decode_frame(r: &mut Decoder<'_>, opcode: FrameOpcode) -> Result<(Channel, FrameEntry), Error> {
    let channel = Channel(r.word()?);
    let entry = FrameEntry::decode(r.bytes(64)?, &[]).map_err(|_| Error::InvalidData)?;
    if channel.0 == 0 || entry.opcode != opcode || entry.length == 0 {
        return Err(Error::InvalidData);
    }
    Ok((channel, entry))
}
