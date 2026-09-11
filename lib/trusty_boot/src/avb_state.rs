use super::*;
impl<T: Transport> Avb<T> {
    pub fn suspend(self) -> (T, [u8; STATE_BYTES]) {
        let (transport, client) = self.client.suspend();
        let mut bytes = [0; STATE_BYTES];
        bytes[..8].copy_from_slice(b"BEXAV001");
        bytes[8..12].copy_from_slice(&self.channel.to_le_bytes());
        if let Some((location, started)) = self.pending_read {
            bytes[12..16].copy_from_slice(&1u32.to_le_bytes());
            bytes[16..20].copy_from_slice(&location.to_le_bytes());
            bytes[24..32].copy_from_slice(&started.to_le_bytes());
        }
        bytes[32..].copy_from_slice(&client);
        (transport, bytes)
    }
    /// # Safety
    /// The record is retained by the resident owner, the old client is consumed,
    /// and transport denotes the same still-connected secure endpoint.
    pub unsafe fn restore_protected(transport: T, bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() != STATE_BYTES || &bytes[..8] != b"BEXAV001" || bytes[20..24] != [0; 4] {
            return Err(Error::InvalidResponse);
        }
        let channel = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
        let pending = u32::from_le_bytes(bytes[12..16].try_into().unwrap());
        let location = u32::from_le_bytes(bytes[16..20].try_into().unwrap());
        let started = u64::from_le_bytes(bytes[24..32].try_into().unwrap());
        if channel == 0
            || pending > 1
            || location > 31
            || (pending == 0 && bytes[16..32] != [0; 16])
        {
            return Err(Error::InvalidResponse);
        }
        Ok(Self {
            client: unsafe { Client::restore_protected(transport, &bytes[32..]) }?,
            channel,
            pending_read: if pending == 1 {
                Some((location, started))
            } else {
                None
            },
        })
    }
}
