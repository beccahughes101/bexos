//! Queue secure requests before receiving either reply, exercising teed's
//! storage-proxy progress while another normal-world request is pending.
use super::{Channel, Memory, TeeManager, TeeServiceManager};
use bexos_debug_wire::{DebugStatusResponse, TeeSessionRequest, TeeUuidRequest};
use bexos_trusty_client::{
    protocol::{AVB_UUID, ORCHESTRATOR_UUID},
    services::*,
};
use tee_manager_fidl::{
    FidlDecode, FidlEncode, HandleRef, TeeManagerInvokeCommandRequest,
    TeeManagerInvokeCommandResponse, TeeStatus,
};

pub async fn run(manager: &mut TeeServiceManager) -> DebugStatusResponse {
    let mut sessions = alloc::vec::Vec::new();
    for uuid in [AVB_UUID, ORCHESTRATOR_UUID] {
        let opened = manager
            .open_session(TeeUuidRequest {
                uuid: uuid.to_vec(),
            })
            .await;
        if opened.status != 0 {
            for session_id in sessions {
                let _ = manager
                    .close_session(TeeSessionRequest { session_id })
                    .await;
            }
            return DebugStatusResponse {
                status: opened.status,
                message: "queued probe session open failed".into(),
            };
        }
        sessions.push(opened.session_id);
    }
    let result = queued(manager.channel, &sessions);
    if result.is_err() {
        // A failed exchange may leave an unread reply. Never let a later RPC
        // mistake that reply for its own completion.
        let _ = Memory::close(manager.channel.0);
        manager.channel = Channel(0);
    }
    let mut close_error = None;
    for session_id in sessions {
        let closed = manager
            .close_session(TeeSessionRequest { session_id })
            .await;
        if result.is_ok() && closed.status != 0 {
            close_error.get_or_insert(closed.status);
        }
    }
    if let Some(status) = close_error {
        return DebugStatusResponse {
            status,
            message: "queued probe close failed".into(),
        };
    }
    match result {
        Ok(()) => DebugStatusResponse {
            status: 0,
            message: "queued AVB storage and orchestrator requests completed".into(),
        },
        Err(message) => DebugStatusResponse {
            status: -8,
            message: message.into(),
        },
    }
}

fn queued(channel: Channel, sessions: &[u64]) -> Result<(), &'static str> {
    // Secure boot approves a generation without committing a new rollback
    // floor. Zero is valid on a freshly provisioned device. Compare with the
    // actual baseline rather than assuming boot itself advanced durable state.
    let baseline_request =
        encode_avb_rollback_index(AVB_CMD_READ_ROLLBACK_INDEX, 0, 0).map_err(|_| "AVB encode")?;
    send_request(
        channel,
        sessions[0],
        AVB_CMD_READ_ROLLBACK_INDEX,
        &baseline_request,
    )?;
    let baseline = decode_avb_u64(&receive_reply(channel)?, AVB_CMD_READ_ROLLBACK_INDEX)
        .map_err(|_| "baseline AVB response malformed")?;
    let requests = [
        (
            AVB_CMD_READ_ROLLBACK_INDEX,
            encode_avb_rollback_index(AVB_CMD_READ_ROLLBACK_INDEX, 0, 0)
                .map_err(|_| "AVB encode")?,
        ),
        (
            ORCHESTRATOR_CMD_GET_KERNEL_SLOT,
            encode_orchestrator_request(ORCHESTRATOR_CMD_GET_KERNEL_SLOT, 1)
                .map_err(|_| "orchestrator encode")?
                .to_vec(),
        ),
    ];
    for (session_id, (command_id, payload)) in sessions.iter().zip(&requests) {
        send_request(channel, *session_id, *command_id, payload)?;
    }
    for index in 0..2 {
        let bytes = receive_reply(channel)?;
        if index == 0 {
            if decode_avb_u64(&bytes, AVB_CMD_READ_ROLLBACK_INDEX)
                .map_err(|_| "queued AVB response malformed")?
                != baseline
            {
                return Err("queued AVB rollback floor changed");
            }
        } else if decode_orchestrator_response(&bytes)
            .map_err(|_| "queued orchestrator response malformed")?
            != 1
        {
            return Err("queued orchestrator slot differs");
        }
    }
    Ok(())
}

fn send_request(
    channel: Channel,
    session_id: u64,
    command_id: u32,
    payload: &[u8],
) -> Result<(), &'static str> {
    let vmo = Memory::from_bytes(payload).map_err(|_| "queued request VMO")?;
    let request = TeeManagerInvokeCommandRequest {
        session_id,
        command_id,
        payload: HandleRef { raw: vmo },
        payload_len: payload.len() as u64,
    };
    let mut bytes = [0; 256];
    let mut handles = [HandleRef { raw: 0 }; 1];
    bytes[..8].copy_from_slice(&7u64.to_le_bytes());
    let sent = request
        .encode(&mut bytes[8..], &mut handles)
        .map_err(|_| "queued request encode")
        .and_then(|n| {
            channel
                .send(&bytes[..8 + n.bytes], &[vmo])
                .map_err(|_| "queued request send")
        });
    let _ = Memory::close(vmo);
    sent
}

fn receive_reply(channel: Channel) -> Result<alloc::vec::Vec<u8>, &'static str> {
    let message = channel
        .recv_with_timeout(60)
        .map_err(|_| "queued secure reply timed out or disconnected")?;
    let handles: alloc::vec::Vec<_> = message
        .handles
        .iter()
        .map(|h| HandleRef { raw: *h })
        .collect();
    let result = (|| {
        let response = TeeManagerInvokeCommandResponse::decode(&message.bytes, &handles)
            .map_err(|_| "malformed queued reply")?;
        if response.status != TeeStatus::Ok || handles.len() != 1 || response.response_len > 4096 {
            return Err("queued secure request rejected");
        }
        super::read_vmo(response.response.raw, response.response_len)
            .map_err(|_| "queued reply VMO")
    })();
    for handle in message.handles {
        let _ = Memory::close(handle);
    }
    result
}
