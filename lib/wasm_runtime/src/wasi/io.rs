use super::{
    state::*,
    wasi::io::{
        error, poll,
        streams::{self, StreamError},
    },
};
use crate::context::Context;
use std::task::Poll;
use wasmtime::{Result, bail, component::Resource};
impl error::Host for Context {}
impl error::HostError for Context {
    fn to_debug_string(&mut self, r: Resource<IoError>) -> Result<String> {
        Ok(self.wasi.table.get(&r)?.0.clone())
    }
    fn drop(&mut self, r: Resource<IoError>) -> Result<()> {
        self.delete(r)?;
        Ok(())
    }
}
impl Context {
    pub(crate) fn stream_error(&mut self, e: wasmtime::Error) -> Result<StreamError> {
        Ok(StreamError::LastOperationFailed(
            self.push(IoError(e.to_string()))?,
        ))
    }
    fn is_ready(&mut self, r: &Resource<Pollable>) -> Result<bool> {
        if self.restoring {
            wasmtime::bail!("external WASI operation during restore");
        }

        match self.wasi.table.get(r)?.clone() {
            Pollable::Output(output) => self.output_ready(&output),
            // Errors make the operation ready so it can report the network error.
            Pollable::Network(h, udp, write) => {
                Ok(self.host.network_ready(&*h, udp, write).unwrap_or(true))
            }
            Pollable::Ready => Ok(true),
            Pollable::Timer(t) => Ok(self.host.monotonic_ns() >= t),
            Pollable::Socket(h, w) => self.host.socket_ready(&*h, w),
        }
    }
}
impl poll::HostPollable for Context {
    fn ready(&mut self, r: Resource<Pollable>) -> Result<bool> {
        self.is_ready(&r)
    }
    async fn block(&mut self, r: Resource<Pollable>) -> Result<()> {
        std::future::poll_fn(|cx| match self.is_ready(&r) {
            Ok(true) => Poll::Ready(Ok(())),
            Err(e) => Poll::Ready(Err(e)),
            _ => {
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        })
        .await
    }
    fn drop(&mut self, r: Resource<Pollable>) -> Result<()> {
        self.delete(r)?;
        Ok(())
    }
}
impl poll::Host for Context {
    async fn poll(&mut self, input: Vec<Resource<Pollable>>) -> Result<Vec<u32>> {
        if input.is_empty() || input.len() > 256 {
            bail!("invalid poll list");
        }
        std::future::poll_fn(|cx| {
            let mut ready = Vec::new();
            for (i, r) in input.iter().enumerate() {
                match self.is_ready(r) {
                    Ok(true) => ready.push(i as u32),
                    Err(e) => return Poll::Ready(Err(e)),
                    _ => {}
                }
            }
            if ready.is_empty() {
                cx.waker().wake_by_ref();
                Poll::Pending
            } else {
                Poll::Ready(Ok(ready))
            }
        })
        .await
    }
}
impl streams::Host for Context {}
impl streams::HostInputStream for Context {
    fn read(&mut self, r: Resource<Input>, len: u64) -> Result<Result<Vec<u8>, StreamError>> {
        if self.restoring {
            wasmtime::bail!("external WASI operation during restore");
        }

        let len = len.min(32768) as usize;
        let input = self.wasi.table.get(&r)?.clone();
        if len == 0 {
            return Ok(Ok(Vec::new()));
        }
        if let Input::Socket(ref h) = input {
            match self.host.socket_ready(&**h, false) {
                Ok(true) => {}
                Ok(false) => return Ok(Ok(Vec::new())),
                Err(error) => return Ok(Err(self.stream_error(error)?)),
            }
        }
        let result = match input {
            Input::Empty => return Ok(Err(StreamError::Closed)),
            Input::File(ref h, offset) => self.host.read_file(&*h.handle, offset, len),
            Input::Socket(ref h) => self.host.socket_read(&**h, len),
        };
        match result {
            Ok(bytes) => {
                if let Input::File(_, pos) = self.wasi.table.get_mut(&r)? {
                    *pos = pos
                        .checked_add(bytes.len() as u64)
                        .ok_or_else(|| wasmtime::format_err!("stream position overflow"))?;
                }
                if bytes.is_empty() && len != 0 {
                    Ok(Err(StreamError::Closed))
                } else {
                    Ok(Ok(bytes))
                }
            }
            Err(e) => Ok(Err(self.stream_error(e)?)),
        }
    }
    async fn blocking_read(
        &mut self,
        r: Resource<Input>,
        len: u64,
    ) -> Result<Result<Vec<u8>, StreamError>> {
        if self.restoring {
            wasmtime::bail!("external WASI operation during restore");
        }

        let handle = match self.wasi.table.get(&r)? {
            Input::Socket(h) => Some(h.clone()),
            _ => None,
        };
        if let Some(handle) = handle {
            let ready = std::future::poll_fn(|cx| match self.host.socket_ready(&*handle, false) {
                Ok(true) => Poll::Ready(Ok(())),
                Err(e) => Poll::Ready(Err(e)),
                Ok(false) => {
                    cx.waker().wake_by_ref();
                    Poll::Pending
                }
            })
            .await;
            if let Err(error) = ready {
                return Ok(Err(self.stream_error(error)?));
            }
        }
        self.read(r, len)
    }
    fn skip(&mut self, r: Resource<Input>, len: u64) -> Result<Result<u64, StreamError>> {
        Ok(self.read(r, len)?.map(|b| b.len() as u64))
    }
    async fn blocking_skip(
        &mut self,
        r: Resource<Input>,
        len: u64,
    ) -> Result<Result<u64, StreamError>> {
        Ok(self.blocking_read(r, len).await?.map(|b| b.len() as u64))
    }
    fn subscribe(&mut self, r: Resource<Input>) -> Result<Resource<Pollable>> {
        let p = match self.wasi.table.get(&r)? {
            Input::Socket(h) => Pollable::Socket(h.clone(), false),
            _ => Pollable::Ready,
        };
        self.push(p)
    }
    fn drop(&mut self, r: Resource<Input>) -> Result<()> {
        self.delete(r)?;
        Ok(())
    }
}
