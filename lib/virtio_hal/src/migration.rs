use crate::{DmaAllocation, MmioMapping, SharedRange};
use alloc::vec::Vec;
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
pub fn encode_dma(w: &mut Encoder, entries: &[DmaAllocation]) {
    w.word(entries.len() as u64);
    for entry in entries {
        for value in [
            entry.paddr,
            entry.vaddr,
            entry.size,
            entry.handle,
            entry.token,
        ] {
            w.word(value);
        }
        w.word(entry.active as u64);
        w.word(entry.owned as u64);
    }
}

pub fn decode_dma(r: &mut Decoder<'_>) -> Result<Vec<DmaAllocation>, Error> {
    let mut out = Vec::new();
    for _ in 0..r.count(1024)? {
        out.push(DmaAllocation {
            paddr: r.word()?,
            vaddr: r.word()?,
            size: r.word()?,
            handle: r.word()?,
            token: r.word()?,
            active: r.flag()?,
            owned: r.flag()?,
        });
    }
    Ok(out)
}

pub fn encode_shared(w: &mut Encoder, entries: &[SharedRange]) {
    w.word(entries.len() as u64);
    for entry in entries {
        for value in [entry.vaddr, entry.paddr, entry.size, entry.vmo, entry.token] {
            w.word(value);
        }
        w.word(entry.active as u64);
        w.word(entry.owned as u64);
    }
}

pub fn decode_shared(r: &mut Decoder<'_>) -> Result<Vec<SharedRange>, Error> {
    let mut out = Vec::new();
    for _ in 0..r.count(1024)? {
        out.push(SharedRange {
            vaddr: r.word()?,
            paddr: r.word()?,
            size: r.word()?,
            vmo: r.word()?,
            token: r.word()?,
            active: r.flag()?,
            owned: r.flag()?,
        });
    }
    Ok(out)
}

pub fn encode_mmio(w: &mut Encoder, entries: &[MmioMapping]) {
    w.word(entries.len() as u64);
    for entry in entries {
        for value in [entry.paddr, entry.vaddr, entry.size, entry.handle] {
            w.word(value);
        }
        w.word(entry.active as u64);
        w.word(entry.owned as u64);
    }
}

pub fn decode_mmio(r: &mut Decoder<'_>) -> Result<Vec<MmioMapping>, Error> {
    let mut out = Vec::new();
    for _ in 0..r.count(1024)? {
        out.push(MmioMapping {
            paddr: r.word()?,
            vaddr: r.word()?,
            size: r.word()?,
            handle: r.word()?,
            active: r.flag()?,
            owned: r.flag()?,
        });
    }
    Ok(out)
}
