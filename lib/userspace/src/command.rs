//! Explicit command startup payload. Handles 0..3 are stdio and payload VMO.
use alloc::{string::String, vec::Vec};
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
pub const STARTUP_MAGIC: u64 = u64::from_le_bytes(*b"BEXCMD01");
pub const MAX_BYTES: usize = 32768;
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CommandOptions {
    pub arguments: Vec<String>,
    pub environment: Vec<(String, String)>,
}
impl CommandOptions {
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        self.validate()?;
        let mut w = Encoder::new();
        w.word(1);
        w.word(self.arguments.len() as u64);
        for argument in &self.arguments {
            w.text(argument);
        }
        w.word(self.environment.len() as u64);
        for (name, value) in &self.environment {
            w.text(name);
            w.text(value);
        }
        let bytes = w.finish();
        if bytes.len() > MAX_BYTES {
            return Err(Error::Capacity);
        }
        Ok(bytes)
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() > MAX_BYTES {
            return Err(Error::Capacity);
        }
        let mut r = Decoder::new(bytes);
        if r.word()? != 1 {
            return Err(Error::InvalidData);
        }
        let mut s = Self::default();
        for _ in 0..r.count(64)? {
            s.arguments.push(r.text(1024)?.into());
        }
        for _ in 0..r.count(64)? {
            s.environment
                .push((r.text(256)?.into(), r.text(1024)?.into()));
        }
        r.finish()?;
        s.validate()?;
        Ok(s)
    }
    fn validate(&self) -> Result<(), Error> {
        if self.arguments.is_empty()
            || self.arguments.len() > 64
            || self.environment.len() > 64
            || self
                .arguments
                .iter()
                .any(|a| a.len() > 1024 || a.contains('\0'))
        {
            return Err(Error::InvalidData);
        }
        for (i, (n, v)) in self.environment.iter().enumerate() {
            if n.is_empty()
                || n.len() > 256
                || n.contains(['=', '\0'])
                || v.len() > 1024
                || v.contains('\0')
                || self.environment[..i].iter().any(|(old, _)| old == n)
            {
                return Err(Error::InvalidData);
            }
        }
        Ok(())
    }
}

/// Read the bounded immutable command metadata without consuming its VMO.
pub fn from_startup(
    startup: &crate::Startup,
) -> Result<Option<CommandOptions>, kernel_fidl::Status> {
    if startup.arg1 != STARTUP_MAGIC {
        return Ok(None);
    }
    if startup.resources.len() != 4 || startup.arg0 == 0 || startup.arg0 > MAX_BYTES as u64 {
        return Err(kernel_fidl::Status::ErrInvalidArgs);
    }
    let raw = startup.resources[3];
    if crate::Memory::object_info(raw)?.0 != kernel_fidl::ObjectType::Vmo {
        return Err(kernel_fidl::Status::ErrInvalidHandle);
    }
    let address = crate::Memory::map(raw, startup.arg0, 2)?;
    let decoded = CommandOptions::decode(unsafe {
        core::slice::from_raw_parts(address as *const u8, startup.arg0 as usize)
    })
    .map_err(|_| kernel_fidl::Status::ErrInvalidArgs);
    crate::Memory::unmap(address, startup.arg0)?;
    decoded.map(Some)
}
