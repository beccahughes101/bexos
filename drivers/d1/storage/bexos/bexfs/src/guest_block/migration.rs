use super::*;
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
use bexos_userspace::live_migration::Resource;
impl SharedBlock {
    pub fn checkpoint(&self) -> Vec<u8> {
        self.0.checkpoint_backend(|b, next| {
            let mut w = Encoder::new();
            w.word(1);
            for n in [
                b.control.0.0,
                b.fifo.0,
                b.handle,
                b.va,
                b.id as u64,
                b.info.block_size as u64,
                b.info.block_count,
                b.info.max_transfer_blocks as u64,
                b.info.flags.0 as u64,
                next,
            ] {
                w.word(n);
            }
            w.finish()
        })
    }
    pub fn adopt(bytes: &[u8]) -> Result<Self, Error> {
        let mut r = Decoder::new(bytes);
        if r.word()? != 1 {
            return Err(Error::UnsupportedVersion);
        }
        let control = Rpc(Channel(r.word()?));
        let fifo = Channel(r.word()?);
        let handle = r.word()?;
        let va = r.word()?;
        let id = u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
        let info = BlockInfo {
            block_size: u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?,
            block_count: r.word()?,
            max_transfer_blocks: u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?,
            flags: BlockFlags(u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?),
        };
        let next = r.word()?;
        r.finish()?;
        if control.0.0 == 0
            || fifo.0 == 0
            || handle == 0
            || va == 0
            || va % 4096 != 0
            || id == 0
            || (info.block_size != 512 && info.block_size != 4096)
            || info.max_transfer_blocks == 0
            || u64::from(info.max_transfer_blocks) * u64::from(info.block_size) > TRANSFER_BYTES
            || next == 0
        {
            return Err(Error::InvalidData);
        }
        let backend = RpcBlock {
            control,
            fifo,
            handle,
            va,
            id,
            info,
            owned: false,
        };
        Ok(Self(Rc::new(FidlBlockDevice::adopt_backend(
            backend, info, next,
        ))))
    }
    pub fn resources(&self) -> Vec<Resource> {
        self.0.checkpoint_backend(|b, _| {
            alloc::vec![
                Resource::Handle(b.control.0.0),
                Resource::Handle(b.fifo.0),
                Resource::Mapping {
                    handle: b.handle,
                    offset: 0,
                    va: b.va,
                    size: TRANSFER_BYTES,
                    rights: 6
                }
            ]
        })
    }
    pub fn activate(&self) {
        self.0.activate_backend(|b| b.owned = true);
    }
}
