//! Reusable, bounded RPC storage for handle-free graphics control messages.
//! Each instance owns one request/response exchange on its channel; unsolicited
//! events must use a separate endpoint. A timed-out endpoint is closed because
//! these legacy responses carry no transaction ID for safely discarding it.
use crate::{close, now_us, stream::read_no_handles, wait};
use bexos_userspace::Channel;
use graphics_fidl::{FidlDecode, FidlEncode, Status};

const RPC_DEADLINE_US: u64 = 60_000_000;

pub struct Rpc {
    request: [u8; 4096],
    response: [u8; 1024],
}
impl Default for Rpc {
    fn default() -> Self {
        Self {
            request: [0; 4096],
            response: [0; 1024],
        }
    }
}
impl Rpc {
    pub fn call<Q: FidlEncode, R: for<'a> FidlDecode<'a>>(
        &mut self,
        channel: &mut Channel,
        ordinal: u64,
        request: &Q,
    ) -> Result<R, Status> {
        if channel.0 == 0 {
            return Err(Status::ErrIo);
        }
        let size = request
            .encode(&mut self.request[8..], &mut [])
            .map_err(|_| Status::ErrInvalidArgs)?;
        self.request[..8].copy_from_slice(&ordinal.to_le_bytes());
        channel
            .send(&self.request[..8 + size.bytes], &[])
            .map_err(|_| Status::ErrIo)?;
        let deadline = now_us().saturating_add(RPC_DEADLINE_US);
        loop {
            match read_no_handles(*channel, &mut self.response) {
                Ok(bytes) => match R::decode(bytes, &[]) {
                    Ok(response) => return Ok(response),
                    Err(_) => {
                        close(&[channel.0]);
                        channel.0 = 0;
                        return Err(Status::ErrInvalidArgs);
                    }
                },
                Err(kernel_fidl::Status::ErrTimedOut) => {}
                Err(_) => {
                    close(&[channel.0]);
                    channel.0 = 0;
                    return Err(Status::ErrIo);
                }
            }
            if now_us() >= deadline {
                close(&[channel.0]);
                channel.0 = 0;
                return Err(Status::ErrTimedOut);
            }
            wait(&[*channel], deadline);
        }
    }
}
