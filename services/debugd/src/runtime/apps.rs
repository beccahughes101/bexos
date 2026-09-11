use super::*;

pub(crate) struct LifecycleAppManager {
    pub(crate) awaiting_response: bool,
    pub(crate) channel: Channel,
    pub(crate) app_manager: Option<Channel>,
    pub(crate) upload: Option<LifecycleUpload>,
}

pub(crate) struct LifecycleUpload {
    pub(crate) upload_id: u64,
    pub(crate) archive_len: usize,
    pub(crate) archive: alloc::vec::Vec<u8>,
}

impl LifecycleAppManager {
    pub(crate) fn new(channel: Channel, app_manager: Option<Channel>) -> Self {
        Self {
            channel,
            app_manager,
            awaiting_response: false,
            upload: None,
        }
    }

    pub(crate) fn call_raw<Q>(
        &mut self,
        ordinal: u64,
        request: &Q,
    ) -> Result<
        (alloc::vec::Vec<u8>, alloc::vec::Vec<lifecycle::HandleRef>),
        lifecycle::FidlWireError,
    >
    where
        Q: LifecycleEncode,
    {
        self.call_raw_with_timeout(
            ordinal,
            request,
            bexos_userspace::ipc::DEADLINE_SECONDS * 1000,
        )
    }

    pub(crate) fn call_raw_with_timeout<Q>(
        &mut self,
        ordinal: u64,
        request: &Q,
        timeout_ms: u64,
    ) -> Result<
        (alloc::vec::Vec<u8>, alloc::vec::Vec<lifecycle::HandleRef>),
        lifecycle::FidlWireError,
    >
    where
        Q: LifecycleEncode,
    {
        self.call_raw_tracking_send(ordinal, request, timeout_ms, &mut false)
    }

    pub(crate) fn call_raw_tracking_send<Q>(
        &mut self,
        ordinal: u64,
        request: &Q,
        timeout_ms: u64,
        sent: &mut bool,
    ) -> Result<
        (alloc::vec::Vec<u8>, alloc::vec::Vec<lifecycle::HandleRef>),
        lifecycle::FidlWireError,
    >
    where
        Q: LifecycleEncode,
    {
        *sent = false;
        // The legacy lifecycle wire has no transaction IDs. After a timeout,
        // drain the one outstanding reply before issuing another request.
        if self.awaiting_response {
            let start = bexos_userspace::syscall::ticks();
            let ticks = bexos_userspace::syscall::frequency().saturating_mul(timeout_ms) / 1000;
            loop {
                match self.channel.try_recv() {
                    Ok(message) => {
                        for handle in message.handles {
                            let _ = Memory::close(handle);
                        }
                        self.awaiting_response = false;
                        break;
                    }
                    Err(kernel_fidl::Status::ErrTimedOut) => {}
                    Err(_) => return Err(lifecycle::FidlWireError::Transport),
                }
                if bexos_userspace::syscall::ticks().wrapping_sub(start) >= ticks {
                    return Err(lifecycle::FidlWireError::Transport);
                }
                bexos_userspace::yield_now();
            }
        }
        let mut request_bytes = alloc::vec![0; 65500];
        let mut request_handles = [lifecycle::HandleRef { raw: 0 }; 8];
        let encoded = request.encode(&mut request_bytes, &mut request_handles)?;
        let raw_handles = request_handles[..encoded.handles]
            .iter()
            .map(|h| h.raw)
            .collect::<alloc::vec::Vec<_>>();
        let mut message = alloc::vec::Vec::with_capacity(8 + encoded.bytes);
        message.extend_from_slice(&ordinal.to_le_bytes());
        message.extend_from_slice(&request_bytes[..encoded.bytes]);
        self.channel
            .send(&message, &raw_handles)
            .map_err(|_| lifecycle::FidlWireError::Transport)?;
        *sent = true;
        if ordinal == 9 {
            bexos_userspace::log("debugd: stored migration lifecycle request sent\n");
        }
        self.awaiting_response = true;
        let start = bexos_userspace::syscall::ticks();
        let timeout_ticks = bexos_userspace::syscall::frequency().saturating_mul(timeout_ms) / 1000;
        loop {
            match self.channel.try_recv() {
                Ok(message) => {
                    self.awaiting_response = false;
                    let response_handles = message
                        .handles
                        .iter()
                        .map(|h| lifecycle::HandleRef { raw: *h })
                        .collect::<alloc::vec::Vec<_>>();
                    return Ok((message.bytes, response_handles));
                }
                Err(kernel_fidl::Status::ErrTimedOut) => {}
                Err(_) => return Err(lifecycle::FidlWireError::Transport),
            }
            if bexos_userspace::syscall::ticks().wrapping_sub(start) >= timeout_ticks {
                return Err(lifecycle::FidlWireError::Transport);
            }
            bexos_userspace::yield_now();
        }
    }

    pub(crate) fn call_app_manager<Q>(
        &mut self,
        ordinal: u64,
        request: &Q,
    ) -> Result<
        (alloc::vec::Vec<u8>, alloc::vec::Vec<app_manager::HandleRef>),
        app_manager::FidlWireError,
    >
    where
        Q: AppManagerEncode,
    {
        let Some(channel) = self.app_manager else {
            return Err(app_manager::FidlWireError::Transport);
        };
        let mut request_bytes = alloc::vec![0; 65500];
        let mut request_handles = [app_manager::HandleRef { raw: 0 }; 8];
        let encoded = request.encode(&mut request_bytes, &mut request_handles)?;
        let message = bexos_userspace::Rpc(channel)
            .call_raw(
                ordinal,
                &request_bytes[..encoded.bytes],
                &request_handles[..encoded.handles]
                    .iter()
                    .map(|h| h.raw)
                    .collect::<alloc::vec::Vec<_>>(),
                true,
            )
            .map_err(|_| app_manager::FidlWireError::Transport)?;
        let response_handles = message
            .handles
            .iter()
            .map(|h| app_manager::HandleRef { raw: *h })
            .collect::<alloc::vec::Vec<_>>();
        Ok((message.bytes, response_handles))
    }
}
