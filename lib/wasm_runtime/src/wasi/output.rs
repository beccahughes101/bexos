use super::{
    output_state::{CAPACITY, Output, Target},
    state::{Input, Pollable},
    wasi::io::streams::{self, StreamError},
};
use crate::context::Context;
use std::task::Poll;
use wasmtime::{Result, bail, component::Resource};
impl Context {
    fn pump_output(&mut self, output: &Output) -> Result<()> {
        if self.restoring {
            bail!("external WASI operation during restore");
        }
        let mut state = output.lock();
        if state.failure.is_some() || matches!(state.target, Target::Closed) {
            return Ok(());
        }
        if !state.pending.is_empty() {
            if let Target::Socket(handle) = &state.target {
                match self.host.socket_ready(&**handle, true) {
                    Ok(true) => {}
                    Ok(false) => return Ok(()),
                    Err(error) => {
                        state.failure = Some(format!("{error:#}"));
                        state.pending.clear();
                        state.permit = 0;
                        state.flushing = false;
                        state.target = Target::Closed;
                        return Ok(());
                    }
                }
            }
            let result = match &state.target {
                Target::Closed => unreachable!(),
                Target::Log => {
                    self.host.log(&state.pending);
                    Ok(state.pending.len())
                }
                Target::File(entry, offset) => match offset.checked_add(state.pending.len() as u64)
                {
                    Some(_) => self
                        .host
                        .write_file(&*entry.handle, *offset, &state.pending),
                    None => Err(wasmtime::format_err!("stream position overflow")),
                },
                Target::Socket(handle) => self.host.socket_write(&**handle, &state.pending),
            };
            match result {
                Ok(n) if n <= state.pending.len() => {
                    if let Target::File(_, offset) = &mut state.target {
                        *offset = offset
                            .checked_add(n as u64)
                            .ok_or_else(|| wasmtime::format_err!("stream position overflow"))?;
                    }
                    state.pending.drain(..n);
                }
                result => {
                    state.failure = Some(match result {
                        Err(error) => format!("{error:#}"),
                        _ => "host exceeded output buffer".into(),
                    });
                    state.pending.clear();
                    state.permit = 0;
                    state.flushing = false;
                    state.target = Target::Closed;
                    return Ok(());
                }
            }
        }
        if state.pending.is_empty() && state.flushing {
            if let Target::File(entry, _) = &state.target {
                if let Err(error) = self.host.sync_file(&*entry.handle) {
                    state.failure = Some(format!("output flush: {error:?}"));
                    state.target = Target::Closed;
                }
            }
            state.flushing = false;
        }
        Ok(())
    }
    pub(crate) fn output_ready(&mut self, output: &Output) -> Result<bool> {
        self.pump_output(output)?;
        let state = output.lock();
        Ok(state.failure.is_some()
            || matches!(state.target, Target::Closed)
            || (state.pending.is_empty() && !state.flushing))
    }
    fn output_error(&mut self, output: &Output) -> Result<Option<StreamError>> {
        let mut state = output.lock();
        if let Some(error) = state.failure.take() {
            return Ok(Some(self.stream_error(wasmtime::format_err!("{error}"))?));
        }
        Ok(matches!(state.target, Target::Closed).then_some(StreamError::Closed))
    }
    async fn wait_output(&mut self, output: &Output) -> Result<()> {
        std::future::poll_fn(|cx| match self.output_ready(output) {
            Ok(true) => Poll::Ready(Ok(())),
            Err(error) => Poll::Ready(Err(error)),
            Ok(false) => {
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        })
        .await
    }
}
impl streams::HostOutputStream for Context {
    fn check_write(&mut self, r: Resource<Output>) -> Result<Result<u64, StreamError>> {
        let output = self.wasi.table.get(&r)?.clone();
        self.pump_output(&output)?;
        if let Some(error) = self.output_error(&output)? {
            return Ok(Err(error));
        }
        let mut state = output.lock();
        if state.pending.is_empty() && !state.flushing {
            state.permit = CAPACITY;
        }
        Ok(Ok(state.permit as u64))
    }
    fn write(&mut self, r: Resource<Output>, bytes: Vec<u8>) -> Result<Result<(), StreamError>> {
        if self.restoring {
            bail!("external WASI operation during restore");
        }
        let output = self.wasi.table.get(&r)?.clone();
        if let Some(error) = self.output_error(&output)? {
            return Ok(Err(error));
        }
        {
            let mut state = output.lock();
            if bytes.len() > state.permit {
                bail!("write exceeds check-write permit");
            }
            state.permit -= bytes.len();
            state.pending.extend_from_slice(&bytes);
        }
        self.pump_output(&output)?;
        Ok(match self.output_error(&output)? {
            Some(error) => Err(error),
            None => Ok(()),
        })
    }
    fn flush(&mut self, r: Resource<Output>) -> Result<Result<(), StreamError>> {
        if self.restoring {
            bail!("external WASI operation during restore");
        }
        let output = self.wasi.table.get(&r)?.clone();
        if let Some(error) = self.output_error(&output)? {
            return Ok(Err(error));
        }
        {
            let mut state = output.lock();
            state.permit = 0;
            state.flushing = true;
        }
        self.pump_output(&output)?;
        Ok(match self.output_error(&output)? {
            Some(error) => Err(error),
            None => Ok(()),
        })
    }
    async fn blocking_flush(&mut self, r: Resource<Output>) -> Result<Result<(), StreamError>> {
        let output = self.wasi.table.get(&r)?.clone();
        if let Err(error) = self.flush(Resource::new_borrow(r.rep()))? {
            return Ok(Err(error));
        }
        self.wait_output(&output).await?;
        Ok(match self.output_error(&output)? {
            Some(error) => Err(error),
            None => Ok(()),
        })
    }
    async fn blocking_write_and_flush(
        &mut self,
        r: Resource<Output>,
        bytes: Vec<u8>,
    ) -> Result<Result<(), StreamError>> {
        if bytes.len() > 4096 {
            bail!("blocking write exceeds WASI 4096-byte limit");
        }
        let output = self.wasi.table.get(&r)?.clone();
        self.wait_output(&output).await?;
        if let Err(error) = self.check_write(Resource::new_borrow(r.rep()))? {
            return Ok(Err(error));
        }
        if let Err(error) = self.write(Resource::new_borrow(r.rep()), bytes)? {
            return Ok(Err(error));
        }
        self.blocking_flush(r).await
    }
    fn subscribe(&mut self, r: Resource<Output>) -> Result<Resource<Pollable>> {
        let output = self.wasi.table.get(&r)?.clone();
        self.push(Pollable::Output(output))
    }
    fn write_zeroes(&mut self, r: Resource<Output>, len: u64) -> Result<Result<(), StreamError>> {
        if len > CAPACITY as u64 {
            bail!("stream write limit");
        }
        self.write(r, vec![0; len as usize])
    }
    async fn blocking_write_zeroes_and_flush(
        &mut self,
        r: Resource<Output>,
        len: u64,
    ) -> Result<Result<(), StreamError>> {
        if len > 4096 {
            bail!("blocking write exceeds WASI 4096-byte limit");
        }
        self.blocking_write_and_flush(r, vec![0; len as usize])
            .await
    }
    fn splice(
        &mut self,
        r: Resource<Output>,
        src: Resource<Input>,
        len: u64,
    ) -> Result<Result<u64, StreamError>> {
        use streams::HostInputStream;
        let permit = match self.check_write(Resource::new_borrow(r.rep()))? {
            Ok(n) => n,
            Err(e) => return Ok(Err(e)),
        };
        if permit == 0 || len == 0 {
            return Ok(Ok(0));
        }
        match self.read(src, len.min(permit))? {
            Ok(bytes) => {
                let n = bytes.len() as u64;
                Ok(self.write(r, bytes)?.map(|_| n))
            }
            Err(error) => Ok(Err(error)),
        }
    }
    async fn blocking_splice(
        &mut self,
        r: Resource<Output>,
        src: Resource<Input>,
        len: u64,
    ) -> Result<Result<u64, StreamError>> {
        use streams::HostInputStream;
        let output = self.wasi.table.get(&r)?.clone();
        self.wait_output(&output).await?;
        let permit = match self.check_write(Resource::new_borrow(r.rep()))? {
            Ok(n) => n,
            Err(e) => return Ok(Err(e)),
        };
        match self.blocking_read(src, len.min(permit)).await? {
            Ok(bytes) => {
                let n = bytes.len() as u64;
                Ok(self.write(r, bytes)?.map(|_| n))
            }
            Err(error) => Ok(Err(error)),
        }
    }
    fn drop(&mut self, r: Resource<Output>) -> Result<()> {
        self.delete(r)?;
        Ok(())
    }
}
