use super::*;
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
impl BlockDeviceServer {
    pub fn checkpoint(&self) -> Vec<u8> {
        let mut w = Encoder::new();
        w.word(1);
        w.word(self.next_vmo_id as u64);
        w.word(self.buffers.len() as u64);
        for (id, b) in &self.buffers {
            w.word(*id as u64);
            w.word(b.handle);
            w.word(b.blocks);
        }
        w.finish()
    }
    pub fn adopt(bytes: &[u8], info: NamespaceInfo) -> Result<Self, Error> {
        let mut r = Decoder::new(bytes);
        if r.word()? != 1 {
            return Err(Error::UnsupportedVersion);
        }
        let mut s = Self::new(info);
        s.next_vmo_id = u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
        for _ in 0..r.count(1024)? {
            let id = u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
            let b = RegisteredBuffer {
                handle: r.word()?,
                blocks: r.word()?,
            };
            if id == 0
                || id >= s.next_vmo_id
                || b.handle == 0
                || b.blocks == 0
                || s.buffers.insert(id, b).is_some()
            {
                return Err(Error::InvalidData);
            }
        }
        r.finish()?;
        Ok(s)
    }
    pub fn complete_dispatched(&mut self) {
        self.dispatched.clear();
    }
    pub fn registrations_match(&self, buffers: &BTreeMap<u32, (u64, u64)>) -> bool {
        self.buffers.len() == buffers.len()
            && self
                .buffers
                .iter()
                .all(|(id, b)| buffers.get(id).is_some_and(|v| v.0 == b.handle))
    }
}
