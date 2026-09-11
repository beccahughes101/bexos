//! Reusable provider session: socket ownership, control dispatch, and migration codec.
use crate::discipline::Discipline;
use crate::transport::{Message, Transport};
use alloc::{vec, vec::Vec};
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
use kernel_fidl::Status;
use tty_fidl::*;
pub struct ProviderSession {
    pub control: u64,
    pub frontend: [u64; 3],
    pub streams: [u64; 3],
    pub taken: bool,
    pub size: WindowSize,
    pub discipline: Discipline,
    pub signals: Vec<Signal>,
    pub exited: bool,
    pub exit_code: i32,
}
impl ProviderSession {
    pub fn new_with(control: u64, transport: &impl Transport) -> Result<Self, Status> {
        let mut frontend = [0; 3];
        let mut streams = [0; 3];
        for i in 0..3 {
            match transport.socket_pair() {
                Ok((a, b)) => {
                    frontend[i] = a;
                    streams[i] = b;
                }
                Err(e) => {
                    for h in frontend.into_iter().chain(streams) {
                        if h != 0 {
                            let _ = transport.close(h);
                        }
                    }
                    return Err(e);
                }
            }
        }
        Ok(Self {
            control,
            frontend,
            streams,
            taken: false,
            size: WindowSize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            },
            discipline: Discipline::default(),
            signals: Vec::new(),
            exited: false,
            exit_code: 0,
        })
    }
    pub fn poll_with(&mut self, transport: &impl Transport) -> Result<bool, Status> {
        match transport.receive(self.control) {
            Ok(m) => {
                self.handle_with(m, transport)?;
                Ok(true)
            }
            Err(Status::ErrTimedOut) => Ok(false),
            Err(e) => Err(e),
        }
    }
    fn reply<Q: FidlEncode>(&self, transport: &impl Transport, q: &Q) -> Result<(), Status> {
        let mut b = vec![0; 4096];
        let mut hs = [HandleRef { raw: 0 }; 4];
        let n = q
            .encode(&mut b, &mut hs)
            .map_err(|_| Status::ErrInvalidArgs)?;
        transport.send(
            self.control,
            &b[..n.bytes],
            &hs[..n.handles].iter().map(|h| h.raw).collect::<Vec<_>>(),
        )
    }
    pub fn handle_with(&mut self, m: Message, transport: &impl Transport) -> Result<(), Status> {
        for h in &m.handles {
            let _ = transport.close(*h);
        }
        if m.bytes.len() < 8 {
            return Err(Status::ErrInvalidArgs);
        }
        let ordinal = u64::from_le_bytes(m.bytes[..8].try_into().unwrap());
        let bytes = &m.bytes[8..];
        match ordinal {
            1 => {
                let status = if self.taken {
                    Status::ErrAlreadyExists
                } else {
                    Status::Ok
                };
                let hs = if self.taken { [0; 3] } else { self.frontend };
                self.reply(
                    transport,
                    &PtySessionTakeIoStreamsResponse {
                        status,
                        stdin_stream: HandleRef { raw: hs[0] },
                        stdout_stream: HandleRef { raw: hs[1] },
                        stderr_stream: HandleRef { raw: hs[2] },
                    },
                )?;
                if !self.taken {
                    self.taken = true;
                    self.frontend = [0; 3];
                }
            }
            2 => {
                let status = match PtySessionSetWindowSizeRequest::decode(bytes, &[]) {
                    Ok(q) if q.size.rows > 0 && q.size.cols > 0 => {
                        self.size = q.size;
                        Status::Ok
                    }
                    _ => Status::ErrInvalidArgs,
                };
                self.reply(transport, &PtySessionSetWindowSizeResponse { status })?;
            }
            3 => {
                let status = match PtySessionSetTerminalModeRequest::decode(bytes, &[]) {
                    Ok(q) if q.flags.0 & !7 == 0 => {
                        self.discipline.mode = q.flags.0;
                        Status::Ok
                    }
                    _ => Status::ErrInvalidArgs,
                };
                self.reply(transport, &PtySessionSetTerminalModeResponse { status })?;
            }
            4 => {
                let status = match PtySessionSendSignalRequest::decode(bytes, &[]) {
                    Ok(q) if self.signals.len() < 32 => {
                        self.signals.push(q.signal);
                        Status::Ok
                    }
                    Ok(_) => Status::ErrResourceExhausted,
                    Err(_) => Status::ErrInvalidArgs,
                };
                self.reply(transport, &PtySessionSendSignalResponse { status })?;
            }
            5 => self.reply(
                transport,
                &PtySessionGetStatusResponse {
                    status: Status::Ok,
                    exited: self.exited,
                    exit_code: self.exit_code,
                },
            )?,
            6 => {
                self.exited = true;
                self.reply(transport, &PtySessionCloseResponse { status: Status::Ok })?;
            }
            _ => return Err(Status::ErrInvalidArgs),
        }
        Ok(())
    }
    pub fn handles(&self) -> Vec<u64> {
        core::iter::once(self.control)
            .chain(self.frontend)
            .chain(self.streams)
            .filter(|h| *h != 0)
            .collect()
    }
    pub fn close_with(&self, transport: &impl Transport) {
        for h in self.handles() {
            let _ = transport.close(h);
        }
    }
    pub fn encode(&self, w: &mut Encoder) {
        w.word(self.control);
        for h in self.frontend.into_iter().chain(self.streams) {
            w.word(h);
        }
        w.word(self.taken as u64);
        for n in [
            self.size.rows as u64,
            self.size.cols as u64,
            self.size.pixel_width as u64,
            self.size.pixel_height as u64,
            self.discipline.mode as u64,
            self.exited as u64,
            self.exit_code as u64,
        ] {
            w.word(n);
        }
        w.bytes(&self.discipline.pending);
        w.word(self.signals.len() as u64);
        for s in &self.signals {
            w.word(*s as u64);
        }
    }
    pub fn decode(r: &mut Decoder<'_>) -> Result<Self, Error> {
        let control = r.word()?;
        let mut frontend = [0; 3];
        let mut streams = [0; 3];
        for h in frontend.iter_mut().chain(streams.iter_mut()) {
            *h = r.word()?;
        }
        let taken = r.flag()?;
        let rows = u16::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
        let cols = u16::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
        let pixel_width = u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
        let pixel_height = u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
        let mode = r.word()?;
        let handles: Vec<_> = core::iter::once(control)
            .chain(frontend)
            .chain(streams)
            .filter(|h| *h != 0)
            .collect();
        if mode > 7
            || control == 0
            || streams.contains(&0)
            || rows == 0
            || cols == 0
            || (taken && frontend != [0; 3])
            || (!taken && frontend.contains(&0))
            || handles
                .iter()
                .enumerate()
                .any(|(i, h)| handles[..i].contains(h))
        {
            return Err(Error::InvalidData);
        }
        let exited = r.flag()?;
        let exit_code = r.word()? as i32;
        let pending = r.bytes(crate::discipline::LIMIT)?.to_vec();
        let n = r.count(32)?;
        let mut signals = Vec::new();
        for _ in 0..n {
            let value = u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
            signals.push(Signal::decode_value(value).map_err(|_| Error::InvalidData)?);
        }
        Ok(Self {
            control,
            frontend,
            streams,
            taken,
            size: WindowSize {
                rows,
                cols,
                pixel_width,
                pixel_height,
            },
            discipline: Discipline {
                mode: mode as u32,
                pending,
            },
            signals,
            exited,
            exit_code,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn checkpoint_preserves_terminal_state_and_handles() {
        let s = ProviderSession {
            control: 1,
            frontend: [0; 3],
            streams: [2, 3, 4],
            taken: true,
            size: WindowSize {
                rows: 42,
                cols: 123,
                pixel_width: 0,
                pixel_height: 0,
            },
            discipline: Discipline {
                mode: 7,
                pending: vec![b'x'; crate::discipline::LIMIT],
            },
            signals: vec![Signal::Interrupt],
            exited: false,
            exit_code: 0,
        };
        let mut w = Encoder::new();
        s.encode(&mut w);
        let bytes = w.finish();
        let mut r = Decoder::new(&bytes);
        let restored = ProviderSession::decode(&mut r).unwrap();
        r.finish().unwrap();
        assert_eq!(restored.handles(), s.handles());
        assert_eq!(restored.discipline.pending, s.discipline.pending);
        assert!(bytes.len() < 65536);
        assert_eq!(restored.size.rows, 42);
        assert_eq!(restored.signals, vec![Signal::Interrupt]);
    }
    #[test]
    fn corrupt_snapshot_is_rejected() {
        let bytes = [0; 8];
        assert!(ProviderSession::decode(&mut Decoder::new(&bytes)).is_err());
    }
}

#[cfg(not(target_family = "wasm"))]
impl ProviderSession {
    pub fn new(control: u64) -> Result<Self, Status> {
        Self::new_with(control, &crate::transport::NativeTransport)
    }
    pub fn poll(&mut self) -> Result<bool, Status> {
        self.poll_with(&crate::transport::NativeTransport)
    }
    pub fn handle(&mut self, m: bexos_userspace::Message) -> Result<(), Status> {
        self.handle_with(
            Message {
                bytes: m.bytes,
                handles: m.handles,
            },
            &crate::transport::NativeTransport,
        )
    }
    pub fn close(&self) {
        self.close_with(&crate::transport::NativeTransport)
    }
}
