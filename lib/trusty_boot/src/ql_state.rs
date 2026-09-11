//! Protected client ownership records. No create/shutdown or storage exchange
//! occurs while transferring ownership; completed RPMB replies are retained.
use super::*;
impl<T: Transport> Client<T> {
    pub fn suspend(self) -> (T, [u8; STATE_BYTES]) {
        let mut bytes = [0; STATE_BYTES];
        bytes[..8].copy_from_slice(b"BEXQL001");
        bytes[8] = u8::from(self.open);
        bytes[9] = u8::from(self.storage.is_some());
        bytes[12..16].copy_from_slice(&self.storage.unwrap_or(0).to_le_bytes());
        bytes[16..24].copy_from_slice(&(self.storage_reply_len as u64).to_le_bytes());
        bytes[32..32 + self.storage_reply_len]
            .copy_from_slice(&self.storage_reply[..self.storage_reply_len]);
        (self.transport, bytes)
    }
    /// # Safety
    /// Bytes originate in resident memory, the old client has been consumed,
    /// and the same registered transport owner and secure domain remain live.
    pub unsafe fn restore_protected(transport: T, bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() != STATE_BYTES
            || &bytes[..8] != b"BEXQL001"
            || bytes[8] != 1
            || bytes[9] > 1
            || bytes[10..12] != [0; 2]
            || bytes[24..32] != [0; 8]
        {
            return Err(Error::InvalidResponse);
        }
        let handle = u32::from_le_bytes(bytes[12..16].try_into().unwrap());
        let length = u64::from_le_bytes(bytes[16..24].try_into().unwrap());
        if (bytes[9] == 0) != (handle == 0)
            || length > crate::storage::MAX_RESPONSE as u64
            || (handle == 0 && length != 0)
            || bytes[32 + length as usize..].iter().any(|b| *b != 0)
        {
            return Err(Error::InvalidResponse);
        }
        Ok(Self {
            transport,
            buffer: [0; wire::QL_BUFFER_SIZE],
            open: true,
            storage: if handle == 0 { None } else { Some(handle) },
            storage_reply: bytes[32..].try_into().unwrap(),
            storage_reply_len: length as usize,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct NoIo;
    impl Transport for NoIo {
        fn exchange(&mut self, _: u32, _: usize, _: &mut [u8]) -> Result<i64, Error> {
            panic!("handoff must not perform I/O")
        }
        fn now_ns(&self) -> u64 {
            50
        }
    }
    #[test]
    fn handoff_preserves_completed_storage_reply_without_replaying_io() {
        let mut reply = [0; crate::storage::MAX_RESPONSE];
        reply[..536].fill(0x5a);
        let old = Client {
            transport: NoIo,
            buffer: [0; wire::QL_BUFFER_SIZE],
            open: true,
            storage: Some(17),
            storage_reply: reply,
            storage_reply_len: 536,
        };
        let (_, state) = old.suspend();
        let new = unsafe { Client::restore_protected(NoIo, &state) }.unwrap();
        assert_eq!(new.storage, Some(17));
        assert_eq!(new.storage_reply_len, 536);
        assert_eq!(new.storage_reply, reply);
        for offset in [0, 8, 9, 10, 24, 32 + 536] {
            let mut corrupt = state;
            corrupt[offset] ^= 0xff;
            assert!(unsafe { Client::restore_protected(NoIo, &corrupt) }.is_err());
        }
    }
}
