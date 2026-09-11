use super::*;

pub trait DebugTransport {
    /// Independent console diagnostics, never part of the framed protocol stream.
    fn read_diagnostics(&mut self) -> Vec<u8> {
        Vec::new()
    }

    /// Bound an individual read; custom transports should honor this for responsive cancellation.
    fn set_read_timeout(&mut self, _timeout: Duration) -> std::io::Result<()> {
        Ok(())
    }
    fn write_all(&mut self, bytes: &[u8]) -> std::io::Result<()>;
    fn read_chunk(&mut self, bytes: &mut [u8]) -> std::io::Result<usize>;
}

pub struct UnixSocketTransport {
    stream: UnixStream,
    draining: bool,
}

impl UnixSocketTransport {
    pub fn connect(path: impl AsRef<Path>) -> std::io::Result<Self> {
        let stream = UnixStream::connect(path)?;
        stream.set_read_timeout(Some(Duration::from_secs(20)))?;
        stream.set_write_timeout(Some(Duration::from_secs(20)))?;
        Ok(Self {
            stream,
            draining: false,
        })
    }

    pub fn from_stream(stream: UnixStream) -> Self {
        Self {
            stream,
            draining: false,
        }
    }
}

impl DebugTransport for UnixSocketTransport {
    fn set_read_timeout(&mut self, timeout: Duration) -> std::io::Result<()> {
        if self.draining {
            return Ok(());
        }
        let result = self.stream.set_read_timeout(Some(timeout));
        #[cfg(target_os = "macos")]
        if result
            .as_ref()
            .is_err_and(|error| error.raw_os_error() == Some(22))
        {
            // XNU rejects setsockopt after both socket directions shut down,
            // even when a final response is still queued. Drain nonblocking
            // so neither the queued response nor the request deadline is lost.
            self.stream.set_nonblocking(true)?;
            self.draining = true;
            return Ok(());
        }
        result.map_err(|error| {
            std::io::Error::new(
                error.kind(),
                format!("set debug socket read timeout {timeout:?}: {error}"),
            )
        })
    }
    fn write_all(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.stream.write_all(bytes).map_err(|error| {
            std::io::Error::new(error.kind(), format!("write debug socket: {error}"))
        })
    }

    fn read_chunk(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        self.stream.read(bytes).map_err(|error| {
            std::io::Error::new(error.kind(), format!("read debug socket: {error}"))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn final_response_can_be_drained_after_peer_shutdown() {
        let (client, mut server) = UnixStream::pair().unwrap();
        server.write_all(b"final response").unwrap();
        drop(server);
        let mut transport = UnixSocketTransport::from_stream(client);
        transport
            .set_read_timeout(Duration::from_millis(100))
            .unwrap();
        let mut bytes = [0; 14];
        assert_eq!(transport.read_chunk(&mut bytes).unwrap(), bytes.len());
        assert_eq!(&bytes, b"final response");
        transport
            .set_read_timeout(Duration::from_millis(100))
            .unwrap();
        assert_eq!(transport.read_chunk(&mut bytes).unwrap(), 0);
    }
}
