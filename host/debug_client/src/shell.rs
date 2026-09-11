use super::*;
use bexos_debug_wire::*;
impl<T: DebugTransport> DebugClient<T> {
    fn shell_call(
        &mut self,
        method: u32,
        q: &ShellRequest,
    ) -> Result<ShellResponse, DebugClientError> {
        let mut payload = Vec::new();
        encode_shell_request(q, &mut payload);
        let frame = self.call(method, payload)?;
        let response = decode_shell_response(&frame.payload)?;
        check_debug_status(response.status, &response.message)?;
        if response.session_id == 0
            || (method != METHOD_SHELL_OPEN && response.session_id != q.session_id)
        {
            return Err(WireError::InvalidProto.into());
        }
        Ok(response)
    }
    pub fn open_shell(&mut self, q: &ShellRequest) -> Result<ShellResponse, DebugClientError> {
        self.shell_call(METHOD_SHELL_OPEN, q)
    }
    pub fn exchange_shell(
        &mut self,
        id: u64,
        input: &[u8],
        eof: bool,
    ) -> Result<ShellResponse, DebugClientError> {
        if input.len() > SHELL_CHUNK {
            return Err(WireError::PayloadTooLarge.into());
        }
        let r = self.shell_call(
            METHOD_SHELL_EXCHANGE,
            &ShellRequest {
                session_id: id,
                input: input.to_vec(),
                eof,
                ..Default::default()
            },
        )?;
        if r.consumed as usize > input.len() {
            return Err(WireError::InvalidProto.into());
        }
        Ok(r)
    }
    pub fn resize_shell(&mut self, id: u64, rows: u16, cols: u16) -> Result<(), DebugClientError> {
        self.shell_call(
            METHOD_SHELL_RESIZE,
            &ShellRequest {
                session_id: id,
                rows,
                cols,
                ..Default::default()
            },
        )?;
        Ok(())
    }
    pub fn set_shell_mode(&mut self, id: u64, mode: u32) -> Result<(), DebugClientError> {
        self.shell_call(
            METHOD_SHELL_MODE,
            &ShellRequest {
                session_id: id,
                mode,
                ..Default::default()
            },
        )?;
        Ok(())
    }
    pub fn signal_shell(&mut self, id: u64, signal: u32) -> Result<(), DebugClientError> {
        self.shell_call(
            METHOD_SHELL_SIGNAL,
            &ShellRequest {
                session_id: id,
                signal,
                ..Default::default()
            },
        )?;
        Ok(())
    }
    pub fn close_shell(&mut self, id: u64) -> Result<(), DebugClientError> {
        self.shell_call(
            METHOD_SHELL_CLOSE,
            &ShellRequest {
                session_id: id,
                ..Default::default()
            },
        )?;
        Ok(())
    }
}
