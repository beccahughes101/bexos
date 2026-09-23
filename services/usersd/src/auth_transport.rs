//! Ordered TEE RPCs. Untagged replies must be drained after a timeout before
//! admitting another request. Checkpoint the pending flag with the channel;
//! draining a late reply never replays the operation that timed out.
use alloc::{vec, vec::Vec};
use bexos_userspace::{Channel, Memory, Rpc};
use tee_manager_fidl as tee;
use tee_manager_fidl::{FidlDecode as TeeFidlDecode, FidlEncode as TeeFidlEncode};
use user_manager_fidl::UserStatus;

pub(crate) struct TeeClient {
    channel: Channel,
    awaiting_response: bool,
}
struct TeeResponse {
    bytes: Vec<u8>,
    handles: Vec<tee::HandleRef>,
}
impl TeeClient {
    pub fn new(channel: Channel, awaiting_response: bool) -> Self {
        Self {
            channel,
            awaiting_response,
        }
    }
    pub fn channel(&self) -> Channel {
        self.channel
    }
    pub fn awaiting_response(&self) -> bool {
        self.awaiting_response
    }
    pub fn open_session(&mut self, uuid: [u8; 16]) -> Result<u64, UserStatus> {
        let response = self.call(5, &tee::TeeManagerOpenSessionRequest { uuid })?;
        let decoded =
            tee::TeeManagerOpenSessionResponse::decode(&response.bytes, &response.handles)
                .map_err(|_| UserStatus::Storage)?;
        if decoded.status != tee::TeeStatus::Ok {
            return Err(UserStatus::Storage);
        }
        Ok(decoded.session_id)
    }

    pub fn invoke(
        &mut self,
        session_id: u64,
        command_id: u32,
        payload: &[u8],
    ) -> Result<Vec<u8>, UserStatus> {
        // A drain timeout must not leak a payload VMO that was never sent.
        Rpc(self.channel)
            .drain_pending_with_timeout(
                bexos_userspace::ipc::DEADLINE_SECONDS,
                &mut self.awaiting_response,
            )
            .map_err(|_| UserStatus::Storage)?;
        let payload_vmo = Memory::from_bytes(payload).map_err(|_| UserStatus::Storage)?;
        let call = self.call(
            7,
            &tee::TeeManagerInvokeCommandRequest {
                session_id,
                command_id,
                payload: tee::HandleRef { raw: payload_vmo },
                payload_len: payload.len() as u64,
            },
        );
        let response = call?;
        let decoded = match tee::TeeManagerInvokeCommandResponse::decode(
            &response.bytes,
            &response.handles,
        ) {
            Ok(decoded) => decoded,
            Err(_) => {
                for handle in response.handles {
                    let _ = Memory::close(handle.raw);
                }
                return Err(UserStatus::Storage);
            }
        };
        if decoded.status != tee::TeeStatus::Ok {
            let _ = Memory::close(decoded.response.raw);
            return Err(UserStatus::AccessDenied);
        }
        if decoded.response_len == 0 || decoded.response_len > 64 * 1024 {
            let _ = Memory::close(decoded.response.raw);
            return Err(UserStatus::Storage);
        }
        let rounded = decoded
            .response_len
            .checked_add(4095)
            .map(|len| len & !4095)
            .ok_or(UserStatus::Storage)?;
        let va = Memory::map(decoded.response.raw, rounded, 2).map_err(|_| UserStatus::Storage)?;
        let bytes =
            unsafe { core::slice::from_raw_parts(va as *const u8, decoded.response_len as usize) }
                .to_vec();
        Memory::unmap(va, rounded).map_err(|_| UserStatus::Storage)?;
        Memory::close(decoded.response.raw).map_err(|_| UserStatus::Storage)?;
        Ok(bytes)
    }

    fn call<Q: TeeFidlEncode>(
        &mut self,
        ordinal: u64,
        request: &Q,
    ) -> Result<TeeResponse, UserStatus> {
        let mut bytes = vec![0; 65500];
        let mut handles = [tee::HandleRef { raw: 0 }; 8];
        let encoded = request
            .encode(&mut bytes, &mut handles)
            .map_err(|_| UserStatus::Storage)?;
        let message = Rpc(self.channel)
            .call_ordered_with_timeout(
                ordinal,
                &bytes[..encoded.bytes],
                &handles[..encoded.handles]
                    .iter()
                    .map(|handle| handle.raw)
                    .collect::<Vec<_>>(),
                bexos_userspace::ipc::DEADLINE_SECONDS,
                &mut self.awaiting_response,
            )
            .map_err(|error| {
                bexos_userspace::log(&alloc::format!(
                    "usersd: TEE transport failed ordinal={ordinal} error={error:?}\n"
                ));
                UserStatus::Storage
            })?;
        Ok(TeeResponse {
            bytes: message.bytes,
            handles: message
                .handles
                .iter()
                .map(|raw| tee::HandleRef { raw: *raw })
                .collect(),
        })
    }
}
