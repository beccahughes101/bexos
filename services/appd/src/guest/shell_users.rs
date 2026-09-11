//! Ordered authentication IPC, isolated from CLI and application-launch queries.
use super::{state, user_manager};
use bexos_userspace::{Channel, Memory, Message, Rpc};

pub(super) fn call(
    state: &mut state::AppdState,
    ordinal: u64,
    request: &impl user_manager::FidlEncode,
) -> Option<Message> {
    if state.shell.users_channel == 0 {
        let (client, provider) = Channel::pair().ok()?;
        if state
            .users
            .send(
                b"bexos.user.UserManager|UserManager|Public|1,6,7||bexos.platform.appd|0|bg",
                &[provider.0],
            )
            .is_err()
        {
            let _ = Memory::close(client.0);
            let _ = Memory::close(provider.0);
            return None;
        }
        state.shell.users_channel = client.0;
    }
    let mut bytes = vec![0; 1024];
    let encoded = match request.encode(&mut bytes, &mut []) {
        Ok(encoded) => encoded,
        Err(_) => {
            bytes.fill(0);
            core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
            return None;
        }
    };
    // A read may queue behind the CLI's durable account mutation (up to 300s).
    // Retain the outstanding-reply flag through timeouts and heart transplant:
    // a previous ListUsers response must never authenticate a later Login.
    let result = Rpc(Channel(state.shell.users_channel))
        .call_ordered_with_timeout(
            ordinal,
            &bytes[..encoded.bytes],
            &[],
            360,
            &mut state.shell.users_awaiting_response,
        )
        .ok();
    bytes.fill(0);
    core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
    result
}
