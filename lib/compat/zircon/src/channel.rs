use crate::{AsHandleRef, Handle, HandleRef, Result, Status};
use alloc::vec::Vec;
use bexos_userspace::{Channel as NativeChannel, Memory, Socket as NativeSocket};

#[derive(Debug)]
pub struct Channel(Handle);

impl Channel {
    pub const unsafe fn from_raw(raw: u64) -> Self {
        Self(unsafe { Handle::from_raw(raw) })
    }

    pub fn read(&self) -> Result<(Vec<u8>, Vec<Handle>)> {
        NativeChannel(self.0.raw_handle())
            .recv_blocking()
            .map(|message| {
                (
                    message.bytes,
                    message
                        .handles
                        .into_iter()
                        .map(|raw| unsafe { Handle::from_raw(raw) })
                        .collect(),
                )
            })
            .map_err(Status::from)
    }

    pub fn write(&self, bytes: &[u8], handles: &[HandleRef<'_>]) -> Result<()> {
        let raw: Vec<_> = handles.iter().map(|handle| handle.raw_handle()).collect();
        NativeChannel(self.0.raw_handle())
            .send(bytes, &raw)
            .map_err(Status::from)
    }
}

impl AsHandleRef for Channel {
    fn as_handle_ref(&self) -> HandleRef<'_> {
        self.0.as_handle_ref()
    }
}

#[derive(Debug)]
pub struct Socket(Handle);

impl Socket {
    pub const unsafe fn from_raw(raw: u64) -> Self {
        Self(unsafe { Handle::from_raw(raw) })
    }

    pub fn read(&self, max_bytes: u32) -> Result<Vec<u8>> {
        NativeSocket(self.0.raw_handle())
            .read(max_bytes)
            .map_err(Status::from)
    }

    pub fn write(&self, bytes: &[u8]) -> Result<u64> {
        NativeSocket(self.0.raw_handle())
            .write(bytes)
            .map_err(Status::from)
    }

    pub fn pair() -> Result<(Self, Self)> {
        NativeSocket::pair()
            .map(|(a, b)| unsafe { (Self::from_raw(a.0), Self::from_raw(b.0)) })
            .map_err(Status::from)
    }
}

impl AsHandleRef for Socket {
    fn as_handle_ref(&self) -> HandleRef<'_> {
        self.0.as_handle_ref()
    }
}

pub fn close(raw: u64) -> Result<()> {
    Memory::close(raw).map_err(Status::from)
}
