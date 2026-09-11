use bexos_tee_driver_trusty_ql_proto as wire;
pub const CREATE: u32 = 0x3200001e;
pub const SHUTDOWN: u32 = 0x3200001f;
pub const COMMAND: u32 = 0x32000020;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Transport,
    Rejected,
    InvalidResponse,
    Timeout,
    PeerClosed,
}
pub trait Transport {
    fn exchange(&mut self, operation: u32, length: usize, buffer: &mut [u8]) -> Result<i64, Error>;
    fn now_ns(&self) -> u64;
    /// Cold recovery may wait for the separate guest boot deadline. Live
    /// callers retain the 30-second transport/preparation bound by default.
    fn timeout_ns(&self) -> u64 {
        30_000_000_000
    }
    fn storage(
        &mut self,
        _request: &[u8],
        _response: &mut [u8; crate::storage::MAX_RESPONSE],
    ) -> Result<usize, Error> {
        Err(Error::Transport)
    }
}
pub struct Client<T> {
    transport: T,
    buffer: [u8; wire::QL_BUFFER_SIZE],
    open: bool,
    storage: Option<u32>,
    storage_reply: [u8; crate::storage::MAX_RESPONSE],
    storage_reply_len: usize,
}
#[path = "ql_state.rs"]
mod state;
pub const STATE_BYTES: usize = 32 + crate::storage::MAX_RESPONSE;
impl<T: Transport> Client<T> {
    pub fn new(mut transport: T) -> Result<Self, Error> {
        let mut buffer = [0; wire::QL_BUFFER_SIZE];
        if transport.exchange(CREATE, buffer.len(), &mut buffer)? != 0 {
            return Err(Error::Rejected);
        }
        Ok(Self {
            transport,
            buffer,
            open: true,
            storage: None,
            storage_reply: [0; crate::storage::MAX_RESPONSE],
            storage_reply_len: 0,
        })
    }
    pub fn connect_storage(&mut self) -> Result<(), Error> {
        if self.storage.is_some() {
            return Err(Error::Rejected);
        }
        self.storage = Some(self.connect(b"com.android.trusty.storage.proxy")?);
        Ok(())
    }
    pub fn close(&mut self) -> Result<(), Error> {
        if self.open {
            let started = self.transport.now_ns();
            loop {
                self.pump_storage()?;
                if self.storage_reply_len == 0 {
                    break;
                }
                if self.expired(started) {
                    return Err(Error::Timeout);
                }
            }
            if self
                .transport
                .exchange(SHUTDOWN, self.buffer.len(), &mut self.buffer)?
                != 0
            {
                return Err(Error::Rejected);
            }
            self.open = false;
            self.storage = None;
            self.storage_reply.fill(0);
            self.storage_reply_len = 0;
        }
        Ok(())
    }
    fn command(&mut self, operation: u16, handle: u32, payload: &[u8]) -> Result<(), Error> {
        if !self.open {
            return Err(Error::PeerClosed);
        }
        self.buffer.fill(0);
        wire::encode_command(&mut self.buffer, operation, handle, payload)
            .map_err(|_| Error::InvalidResponse)?;
        if self
            .transport
            .exchange(COMMAND, 16 + payload.len(), &mut self.buffer)?
            < 0
        {
            return Err(Error::Rejected);
        }
        let (header, _) =
            wire::response_payload(&self.buffer, operation).map_err(|_| Error::InvalidResponse)?;
        if header.status != 0 {
            return Err(Error::Rejected);
        }
        Ok(())
    }
    fn expired(&self, started: u64) -> bool {
        let now = self.transport.now_ns();
        now < started || now - started >= self.transport.timeout_ns()
    }
    // Pump once per AVB wait. Preserve an unsent reply across backpressure so
    // a completed RPMB write is never replayed just because TIPC was blocked.
    fn pump_storage(&mut self) -> Result<(), Error> {
        let Some(handle) = self.storage else {
            return Ok(());
        };
        if self.storage_reply_len == 0 {
            self.command(wire::QL_OP_GET_EVENT, handle, &0u64.to_le_bytes())?;
            let (_, event) = wire::response_payload(&self.buffer, wire::QL_OP_GET_EVENT)
                .map_err(|_| Error::InvalidResponse)?;
            if event.len() != 16 {
                return Err(Error::InvalidResponse);
            }
            let flags = u32::from_le_bytes(event[..4].try_into().unwrap());
            if flags == 0 && event == [0; 16] {
                return Ok(());
            }
            if u32::from_le_bytes(event[4..8].try_into().unwrap()) != handle {
                return Err(Error::InvalidResponse);
            }
            if flags & 6 != 0 {
                return Err(Error::PeerClosed);
            }
            if flags & 8 == 0 {
                return Ok(());
            }
            self.command(wire::QL_OP_RECV, handle, &[])?;
            let (_, payload) = wire::response_payload(&self.buffer, wire::QL_OP_RECV)
                .map_err(|_| Error::InvalidResponse)?;
            let length = self.transport.storage(payload, &mut self.storage_reply)?;
            if length == 0 || length > self.storage_reply.len() {
                return Err(Error::InvalidResponse);
            }
            self.storage_reply_len = length;
        }
        let reply = self.storage_reply;
        match self.command(wire::QL_OP_SEND, handle, &reply[..self.storage_reply_len]) {
            Ok(()) => {
                self.storage_reply_len = 0;
                self.storage_reply.fill(0);
                Ok(())
            }
            Err(Error::Rejected) => Ok(()),
            Err(error) => Err(error),
        }
    }
    pub fn connect(&mut self, port: &[u8]) -> Result<u32, Error> {
        if port.is_empty() || port.len() > 128 || port.contains(&0) {
            return Err(Error::InvalidResponse);
        }
        let mut payload = [0; 145];
        payload[..8].copy_from_slice(&1u64.to_le_bytes());
        payload[16..16 + port.len()].copy_from_slice(port);
        let started = self.transport.now_ns();
        loop {
            match self.command(wire::QL_OP_CONNECT, 0, &payload[..17 + port.len()]) {
                Ok(()) => {
                    let (header, _) = wire::response_payload(&self.buffer, wire::QL_OP_CONNECT)
                        .map_err(|_| Error::InvalidResponse)?;
                    if header.handle != 0 {
                        self.wait(header.handle, 1, 1, started)?;
                        return Ok(header.handle);
                    }
                    return Err(Error::InvalidResponse);
                }
                Err(Error::Rejected) => {}
                Err(error) => return Err(error),
            }
            if self.expired(started) {
                return Err(Error::Timeout);
            }
            self.pump_storage()?;
        }
    }
    fn wait(&mut self, handle: u32, cookie: u64, expected: u32, started: u64) -> Result<(), Error> {
        loop {
            self.pump_storage()?;
            self.command(wire::QL_OP_GET_EVENT, handle, &0u64.to_le_bytes())?;
            let (_, event) = wire::response_payload(&self.buffer, wire::QL_OP_GET_EVENT)
                .map_err(|_| Error::InvalidResponse)?;
            if event.len() != 16 {
                return Err(Error::InvalidResponse);
            }
            let flags = u32::from_le_bytes(event[..4].try_into().unwrap());
            if flags & 6 != 0 {
                return Err(Error::PeerClosed);
            }
            if u32::from_le_bytes(event[4..8].try_into().unwrap()) == handle
                && (cookie == 0 || u64::from_le_bytes(event[8..16].try_into().unwrap()) == cookie)
                && flags & expected != 0
            {
                return Ok(());
            }
            if self.expired(started) {
                return Err(Error::Timeout);
            }
        }
    }
    pub fn transact(
        &mut self,
        handle: u32,
        request: &[u8],
        response: &mut [u8],
    ) -> Result<usize, Error> {
        let started = self.send(handle, request)?;
        self.receive(handle, started, response)
    }
    /// Submit once. A retained client may resume receiving after owner handoff
    /// without sending the operation again.
    pub fn send(&mut self, handle: u32, request: &[u8]) -> Result<u64, Error> {
        let started = self.transport.now_ns();
        loop {
            match self.command(wire::QL_OP_SEND, handle, request) {
                Ok(()) => break,
                Err(Error::Rejected) => {
                    self.wait(handle, 0, 0x10, started)?;
                    if self.expired(started) {
                        return Err(Error::Timeout);
                    }
                }
                Err(error) => return Err(error),
            }
        }
        Ok(started)
    }
    pub fn receive(
        &mut self,
        handle: u32,
        started: u64,
        response: &mut [u8],
    ) -> Result<usize, Error> {
        if self.expired(started) {
            return Err(Error::Timeout);
        }
        self.wait(handle, 0, 8, started)?;
        self.command(wire::QL_OP_RECV, handle, &[])?;
        let (_, payload) = wire::response_payload(&self.buffer, wire::QL_OP_RECV)
            .map_err(|_| Error::InvalidResponse)?;
        if payload.len() > response.len() {
            return Err(Error::InvalidResponse);
        }
        response[..payload.len()].copy_from_slice(payload);
        Ok(payload.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Deadline {
        now: u64,
        timeout: u64,
    }
    impl Transport for Deadline {
        fn exchange(&mut self, _: u32, _: usize, _: &mut [u8]) -> Result<i64, Error> {
            Ok(0)
        }
        fn now_ns(&self) -> u64 {
            self.now
        }
        fn timeout_ns(&self) -> u64 {
            self.timeout
        }
    }
    #[test]
    fn cold_deadline_is_separate_and_live_limit_is_not_extended() {
        for timeout in [30_000_000_000, 600_000_000_000] {
            let mut client = Client::new(Deadline {
                now: timeout,
                timeout,
            })
            .unwrap();
            assert!(client.expired(0));
            client.transport.now -= 1;
            assert!(!client.expired(0));
            assert!(client.expired(timeout));
        }
    }
    struct StorageTransport {
        sends: usize,
        exchanges: usize,
        receives: usize,
        empty: bool,
    }
    impl Transport for StorageTransport {
        fn exchange(&mut self, operation: u32, _: usize, buffer: &mut [u8]) -> Result<i64, Error> {
            if operation == CREATE || operation == SHUTDOWN {
                return Ok(0);
            }
            let header = wire::decode_header(buffer).unwrap();
            assert_eq!(header.handle, 7);
            match header.opcode {
                wire::QL_OP_GET_EVENT => {
                    buffer[16..32].fill(0);
                    if !self.empty {
                        buffer[16..20].copy_from_slice(&8u32.to_le_bytes());
                        buffer[20..24].copy_from_slice(&7u32.to_le_bytes());
                    }
                    wire::encode_header(
                        buffer,
                        wire::Header::new(header.opcode | wire::QL_RESP_BIT, 7, 16),
                    )
                    .unwrap();
                }
                wire::QL_OP_RECV => {
                    self.receives += 1;
                    buffer[16] = 0x55;
                    wire::encode_header(
                        buffer,
                        wire::Header::new(header.opcode | wire::QL_RESP_BIT, 7, 1),
                    )
                    .unwrap();
                }
                wire::QL_OP_SEND => {
                    self.sends += 1;
                    assert_eq!(&buffer[16..18], &[0x12, 0x34]);
                    if self.sends == 1 {
                        return Ok(-1);
                    }
                    wire::encode_header(
                        buffer,
                        wire::Header::new(header.opcode | wire::QL_RESP_BIT, 7, 0),
                    )
                    .unwrap();
                }
                _ => panic!("unexpected command"),
            }
            Ok(0)
        }
        fn storage(
            &mut self,
            request: &[u8],
            response: &mut [u8; crate::storage::MAX_RESPONSE],
        ) -> Result<usize, Error> {
            self.exchanges += 1;
            assert_eq!(request, &[0x55]);
            response[..2].copy_from_slice(&[0x12, 0x34]);
            Ok(2)
        }
        fn now_ns(&self) -> u64 {
            0
        }
    }
    #[test]
    fn storage_backpressure_cannot_repeat_an_authoritative_rpmb_exchange() {
        let mut client = Client::new(StorageTransport {
            sends: 0,
            exchanges: 0,
            receives: 0,
            empty: false,
        })
        .unwrap();
        client.storage = Some(7);
        client.pump_storage().unwrap();
        assert_eq!(client.storage_reply_len, 2);
        client.pump_storage().unwrap();
        assert_eq!(client.storage_reply_len, 0);
        assert_eq!(client.transport.exchanges, 1);
        assert_eq!(client.transport.receives, 1);
        assert_eq!(client.transport.sends, 2);
        client.transport.empty = true;
        client.pump_storage().unwrap();
        assert_eq!(client.transport.exchanges, 1);
        assert!(client.storage_reply.iter().all(|byte| *byte == 0));
        client.close().unwrap();
        assert_eq!(client.storage, None);
    }
    struct FailedTransport {
        calls: usize,
        error: Error,
    }
    impl Transport for FailedTransport {
        fn exchange(&mut self, operation: u32, _: usize, _: &mut [u8]) -> Result<i64, Error> {
            self.calls += 1;
            assert!(self.calls <= 2, "fatal transport failure was retried");
            if operation == CREATE {
                Ok(0)
            } else {
                Err(self.error)
            }
        }
        fn now_ns(&self) -> u64 {
            0
        }
    }
    #[test]
    fn connection_and_send_preserve_fatal_errors_without_retrying() {
        for error in [
            Error::Transport,
            Error::InvalidResponse,
            Error::Timeout,
            Error::PeerClosed,
        ] {
            let mut client = Client::new(FailedTransport { calls: 0, error }).unwrap();
            assert_eq!(client.connect(b"com.android.trusty.avb"), Err(error));
            let mut client = Client::new(FailedTransport { calls: 0, error }).unwrap();
            assert_eq!(client.transact(1, &[0; 8], &mut [0; 16]), Err(error));
        }
    }
}
