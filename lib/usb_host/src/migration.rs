use alloc::vec::Vec;
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ChannelState {
    pub handle: u64,
    pub generation: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct QueueState {
    pub producer: u64,
    pub consumer: u64,
    pub cycle: bool,
    pub paused: bool,
}

impl QueueState {
    pub fn encode(self, w: &mut Encoder) {
        w.word(self.producer);
        w.word(self.consumer);
        w.word(self.cycle as u64);
        w.word(self.paused as u64);
    }

    pub fn decode(r: &mut Decoder<'_>) -> Result<Self, Error> {
        Ok(Self {
            producer: r.word()?,
            consumer: r.word()?,
            cycle: r.flag()?,
            paused: r.flag()?,
        })
    }
}

pub fn encode_handles(handles: &[u64]) -> Vec<u8> {
    let mut w = Encoder::new();
    w.word(1);
    w.word(handles.len() as u64);
    for handle in handles {
        w.word(*handle);
    }
    w.finish()
}

pub fn decode_handles(bytes: &[u8], max: usize) -> Result<Vec<u64>, Error> {
    let mut r = Decoder::new(bytes);
    if r.word()? != 1 {
        return Err(Error::UnsupportedVersion);
    }
    let mut out = Vec::new();
    for _ in 0..r.count(max)? {
        let handle = r.word()?;
        if handle == 0 || out.contains(&handle) {
            return Err(Error::InvalidData);
        }
        out.push(handle);
    }
    r.finish()?;
    Ok(out)
}
