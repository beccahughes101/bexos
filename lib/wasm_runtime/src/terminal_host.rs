//! Checked byte-stream operations shared by guest terminal adapters.
use crate::{
    bindings::bexos::wasm::kernel::StreamError,
    context::Context,
    resources::{Kind, READ, WRITE},
};
use wasmtime::{Result, bail};
const MAX_BYTES: usize = 32768;

impl Context {
    pub(crate) fn terminal_socket_pair(&mut self) -> Result<(u32, u32)> {
        if self.restoring {
            bail!("socket creation during restore");
        }
        let host = self.host.clone();
        // Reserve IDs and aggregate budget before allocating either endpoint.
        let (_, ids) = self.resources.receive(2, || {
            let (a, b) = host.socket_pair()?;
            if a.kind() != Kind::Socket || b.kind() != Kind::Socket {
                bail!("invalid host socket pair");
            }
            Ok(((), vec![a, b]))
        })?;
        Ok((ids[0], ids[1]))
    }
    pub(crate) fn terminal_socket_read(
        &mut self,
        id: u32,
        length: usize,
    ) -> Result<Result<Vec<u8>, StreamError>> {
        if self.restoring || length > MAX_BYTES {
            bail!("socket read admission");
        }
        let h = self.resources.get(id, Kind::Socket, READ)?.handle.clone();
        if length == 0 {
            return Ok(Ok(Vec::new()));
        }
        match self.host.socket_ready(&*h, false) {
            Ok(false) => return Ok(Err(StreamError::WouldBlock)),
            Err(_) => return Ok(Err(StreamError::Failed)),
            Ok(true) => (),
        }
        Ok(match self.host.socket_read(&*h, length) {
            Ok(bytes) if bytes.len() <= length => Ok(bytes),
            Ok(_) => bail!("host exceeded socket read limit"),
            Err(_) => Err(StreamError::Failed),
        })
    }
    pub(crate) fn terminal_socket_write(
        &mut self,
        id: u32,
        bytes: &[u8],
    ) -> Result<Result<u32, StreamError>> {
        if self.restoring || bytes.len() > MAX_BYTES {
            bail!("socket write admission");
        }
        let h = self.resources.get(id, Kind::Socket, WRITE)?.handle.clone();
        if bytes.is_empty() {
            return Ok(Ok(0));
        }
        Ok(match self.host.socket_write(&*h, bytes) {
            Ok(0) => Err(StreamError::WouldBlock),
            Ok(n) if n <= bytes.len() => Ok(n as u32),
            Ok(_) => bail!("host exceeded socket write limit"),
            Err(_) => Err(StreamError::Failed),
        })
    }
    pub(crate) fn terminal_socket_ready(&self, id: u32, write: bool) -> Result<bool> {
        if self.restoring {
            bail!("socket polling during restore");
        }
        let h = self
            .resources
            .get(id, Kind::Socket, if write { WRITE } else { READ })?;
        self.host.socket_ready(&*h.handle, write)
    }
    pub(crate) fn terminal_socket_shutdown(&self, id: u32, read: bool, write: bool) -> Result<()> {
        if self.restoring || (!read && !write) {
            bail!("socket shutdown admission");
        }
        let rights = if read { READ } else { 0 } | if write { WRITE } else { 0 };
        let h = self.resources.get(id, Kind::Socket, rights)?;
        self.host.socket_half_close(&*h.handle, read, write)
    }
}
