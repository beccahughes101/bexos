use crate::ql::{Client, Error, Transport};
const PORT: &[u8] = b"com.android.trusty.avb";
pub const STATE_BYTES: usize = 32 + crate::ql::STATE_BYTES;
#[path = "avb_state.rs"]
mod state;
pub struct Avb<T> {
    client: Client<T>,
    channel: u32,
    pending_read: Option<(u32, u64)>,
}
impl<T: Transport> Avb<T> {
    pub fn connect(transport: T) -> Result<Self, Error> {
        Self::connect_mode(transport, false)
    }
    pub fn connect_with_storage(transport: T) -> Result<Self, Error> {
        Self::connect_mode(transport, true)
    }
    fn connect_mode(transport: T, storage: bool) -> Result<Self, Error> {
        let mut client = Client::new(transport)?;
        if storage && let Err(error) = client.connect_storage() {
            let _ = client.close();
            return Err(error);
        }
        let channel = match client.connect(PORT) {
            Ok(channel) => channel,
            Err(error) => {
                let _ = client.close();
                return Err(error);
            }
        };
        Ok(Self {
            client,
            channel,
            pending_read: None,
        })
    }
    pub fn read_rollback(&mut self, location: u32) -> Result<u64, Error> {
        self.begin_read_rollback(location)?;
        self.finish_read_rollback()
    }
    pub fn begin_read_rollback(&mut self, location: u32) -> Result<(), Error> {
        if location > 31 || self.pending_read.is_some() {
            return Err(Error::InvalidResponse);
        }
        let mut request = [0; 20];
        request[16..20].copy_from_slice(&location.to_le_bytes());
        let started = self.client.send(self.channel, &request)?;
        self.pending_read = Some((location, started));
        Ok(())
    }
    pub fn finish_read_rollback(&mut self) -> Result<u64, Error> {
        let (_, started) = self.pending_read.ok_or(Error::InvalidResponse)?;
        let mut response = [0; 16];
        let size = self.client.receive(self.channel, started, &mut response)?;
        check(&response[..size], 0, 16)?;
        self.pending_read = None;
        Ok(u64::from_le_bytes(response[8..16].try_into().unwrap()))
    }
    pub fn write_rollback(&mut self, location: u32, generation: u64) -> Result<(), Error> {
        if location > 31 || self.pending_read.is_some() {
            return Err(Error::InvalidResponse);
        }
        let mut request = [0; 20];
        request[..4].copy_from_slice(&2u32.to_le_bytes());
        request[8..16].copy_from_slice(&generation.to_le_bytes());
        request[16..20].copy_from_slice(&location.to_le_bytes());
        let mut response = [0; 16];
        let size = self
            .client
            .transact(self.channel, &request, &mut response)?;
        check(&response[..size], 2, 16)?;
        if u64::from_le_bytes(response[8..16].try_into().unwrap()) != generation {
            return Err(Error::Rejected);
        }
        Ok(())
    }
    pub fn read_lock_state(&mut self) -> Result<bool, Error> {
        if self.pending_read.is_some() {
            return Err(Error::InvalidResponse);
        }
        let mut request = [0; 8];
        request[..4].copy_from_slice(&10u32.to_le_bytes());
        let mut response = [0; 9];
        let size = self
            .client
            .transact(self.channel, &request, &mut response)?;
        check(&response[..size], 10, 9)?;
        if response[8] > 1 {
            return Err(Error::InvalidResponse);
        }
        Ok(response[8] == 1)
    }
    pub fn write_lock_state(&mut self, locked: bool) -> Result<(), Error> {
        if self.pending_read.is_some() {
            return Err(Error::InvalidResponse);
        }
        let mut request = [0; 9];
        request[..4].copy_from_slice(&12u32.to_le_bytes());
        request[8] = u8::from(locked);
        let mut response = [0; 8];
        let size = self
            .client
            .transact(self.channel, &request, &mut response)?;
        check(&response[..size], 12, 8)
    }
    pub fn lock_boot_state(&mut self) -> Result<(), Error> {
        if self.pending_read.is_some() {
            return Err(Error::InvalidResponse);
        }
        let mut request = [0; 8];
        request[..4].copy_from_slice(&14u32.to_le_bytes());
        let mut response = [0; 8];
        let size = self
            .client
            .transact(self.channel, &request, &mut response)?;
        check(&response[..size], 14, 8)
    }
    pub fn close(&mut self) -> Result<(), Error> {
        self.client.close()
    }
}
fn check(response: &[u8], command: u32, length: usize) -> Result<(), Error> {
    if response.len() < 8 || response[..4] != (command | 1).to_le_bytes() {
        return Err(Error::InvalidResponse);
    }
    if response[4..8] != [0; 4] {
        return if response.len() == 8 {
            Err(Error::Rejected)
        } else {
            Err(Error::InvalidResponse)
        };
    }
    if response.len() != length {
        return Err(Error::InvalidResponse);
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn approval_cannot_come_from_wrong_command_status_or_truncated_reply() {
        let mut reply = [0; 16];
        reply[0] = 1;
        assert_eq!(check(&reply, 0, 16), Ok(()));
        assert_eq!(check(&reply, 2, 16), Err(Error::InvalidResponse));
        for length in 0..16 {
            assert_eq!(check(&reply[..length], 0, 16), Err(Error::InvalidResponse));
        }
        reply[4] = 1;
        assert_eq!(check(&reply, 0, 16), Err(Error::InvalidResponse));
        assert_eq!(check(&reply[..8], 0, 16), Err(Error::Rejected));
    }
}
