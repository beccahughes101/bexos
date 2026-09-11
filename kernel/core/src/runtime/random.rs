//! ChaCha20 output with a unique stream per request, seeded only by boot entropy.
use super::*;
use rand_chacha::{
    ChaCha20Rng,
    rand_core::{RngCore, SeedableRng},
};
use sha2::{Digest, Sha256};
#[derive(Clone)]
pub(super) struct Entropy {
    pub seed: [u8; 32],
    pub stream: u64,
}
impl Entropy {
    pub fn from_boot(seed: [u64; 4]) -> Self {
        let mut hash = Sha256::new();
        hash.update(b"BexOS kernel random v1");
        for word in seed {
            hash.update(word.to_le_bytes());
        }
        Self {
            seed: hash.finalize().into(),
            stream: 0,
        }
    }
    pub fn write(
        &self,
        w: &mut crate::transplant::codec::Writer<'_>,
    ) -> crate::transplant::codec::Result<()> {
        w.bytes(&self.seed)?;
        w.word(self.stream)
    }
    pub fn read(
        r: &mut crate::transplant::codec::Reader<'_>,
    ) -> crate::transplant::codec::Result<Self> {
        let seed = r.bytes(32)?.try_into().unwrap();
        let stream = r.word()?;
        Ok(Self { seed, stream })
    }
}
impl<B: Backend> Runtime<B> {
    pub fn random_bytes(&mut self, length: usize) -> Result<Vec<u8>> {
        if length > 32768 {
            return Err(Status::ErrInvalidArgs);
        }
        let entropy = self.entropy.as_mut().ok_or(Status::ErrAccessDenied)?;
        let next = entropy
            .stream
            .checked_add(1)
            .ok_or(Status::ErrResourceExhausted)?;
        let mut rng = ChaCha20Rng::from_seed(entropy.seed);
        rng.set_stream(entropy.stream);
        entropy.stream = next;
        let mut out = alloc::vec![0;length];
        rng.fill_bytes(&mut out);
        self.changed(META, 0);
        Ok(out)
    }
    pub(super) fn write_entropy(
        &self,
        w: &mut crate::transplant::codec::Writer<'_>,
    ) -> crate::transplant::codec::Result<()> {
        w.word(self.entropy.is_some() as u64)?;
        if let Some(e) = &self.entropy {
            e.write(w)?;
        }
        Ok(())
    }
    pub(super) fn read_entropy(
        &mut self,
        r: &mut crate::transplant::codec::Reader<'_>,
    ) -> crate::transplant::codec::Result<()> {
        self.entropy = if r.flag()? {
            Some(Entropy::read(r)?)
        } else {
            None
        };
        Ok(())
    }
}
