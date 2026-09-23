//! Bounded incremental HTTP/1.1 response framing for streaming downloads.
use crate::{NetError, http1::Response};
use alloc::{string::ToString, vec::Vec};
enum Body {
    Head,
    Fixed(usize),
    Close,
    ChunkSize,
    Chunk(usize),
    ChunkEnd,
    Trailers,
    Done,
}
pub struct Decoder {
    buffer: Vec<u8>,
    body: Body,
    response: Option<Response>,
    total: usize,
    maximum: usize,
    trailers: usize,
}
impl Decoder {
    pub fn new(maximum: usize) -> Self {
        Self {
            buffer: Vec::new(),
            body: Body::Head,
            response: None,
            total: 0,
            maximum,
            trailers: 0,
        }
    }
    pub fn feed(
        &mut self,
        bytes: &[u8],
        sink: &mut impl FnMut(u16, &[u8]) -> Result<(), NetError>,
    ) -> Result<(), NetError> {
        // The transport feeds at most 8 KiB; framing retains at most 16 KiB.
        if bytes.len() > 8192 {
            return Err(NetError::BodyTooLarge);
        }
        self.buffer.extend_from_slice(bytes);
        loop {
            match self.body {
                Body::Head => {
                    let Some(end) = self
                        .buffer
                        .windows(4)
                        .position(|w| w == b"\r\n\r\n")
                        .map(|i| i + 4)
                    else {
                        if self.buffer.len() > 16384 {
                            return Err(NetError::BodyTooLarge);
                        }
                        break;
                    };
                    if end > 16384 {
                        return Err(NetError::BodyTooLarge);
                    }
                    let head =
                        core::str::from_utf8(&self.buffer[..end]).map_err(|_| NetError::BadHttp)?;
                    let mut lines = head.split("\r\n");
                    let mut status = lines.next().ok_or(NetError::BadHttp)?.splitn(3, ' ');
                    if status.next() != Some("HTTP/1.1") {
                        return Err(NetError::BadHttp);
                    }
                    let status: u16 = status
                        .next()
                        .and_then(|s| s.parse().ok())
                        .filter(|s| (200..=599).contains(s))
                        .ok_or(NetError::BadHttp)?;
                    let mut headers = Vec::new();
                    let mut length = None;
                    let mut chunked = false;
                    for line in lines.filter(|s| !s.is_empty()) {
                        let (name, value) = line.split_once(':').ok_or(NetError::BadHttp)?;
                        if name.is_empty()
                            || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                        {
                            return Err(NetError::BadHttp);
                        }
                        let value = value.trim();
                        if name.eq_ignore_ascii_case("content-length") {
                            if length.is_some() || chunked {
                                return Err(NetError::BadHttp);
                            }
                            length = Some(value.parse::<usize>().map_err(|_| NetError::BadHttp)?);
                        }
                        if name.eq_ignore_ascii_case("transfer-encoding") {
                            if length.is_some() || chunked || !value.eq_ignore_ascii_case("chunked")
                            {
                                return Err(NetError::BadHttp);
                            }
                            chunked = true;
                        }
                        headers.push((name.to_string(), value.to_string()));
                    }
                    if length.is_some_and(|n| n > self.maximum) {
                        return Err(NetError::BodyTooLarge);
                    }
                    self.response = Some(Response {
                        status,
                        headers,
                        body: Vec::new(),
                    });
                    self.body = if chunked {
                        Body::ChunkSize
                    } else if let Some(n) = length {
                        Body::Fixed(n)
                    } else {
                        Body::Close
                    };
                    self.buffer.drain(..end);
                }
                Body::Fixed(n) | Body::Chunk(n) => {
                    if n == 0 {
                        self.body = if matches!(self.body, Body::Fixed(_)) {
                            Body::Done
                        } else {
                            Body::ChunkEnd
                        };
                        continue;
                    }
                    if self.buffer.is_empty() {
                        break;
                    }
                    let count = n.min(self.buffer.len());
                    self.emit(count, sink)?;
                    self.body = if matches!(self.body, Body::Fixed(_)) {
                        Body::Fixed(n - count)
                    } else {
                        Body::Chunk(n - count)
                    };
                }
                Body::Close => {
                    if !self.buffer.is_empty() {
                        self.emit(self.buffer.len(), sink)?;
                    }
                    break;
                }
                Body::ChunkSize => {
                    let Some(end) = self.buffer.windows(2).position(|w| w == b"\r\n") else {
                        if self.buffer.len() > 4096 {
                            return Err(NetError::BadHttp);
                        }
                        break;
                    };
                    if end > 4096 {
                        return Err(NetError::BadHttp);
                    }
                    let line =
                        core::str::from_utf8(&self.buffer[..end]).map_err(|_| NetError::BadHttp)?;
                    let text = line.split(';').next().unwrap();
                    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
                        return Err(NetError::BadHttp);
                    }
                    let n = usize::from_str_radix(text, 16).map_err(|_| NetError::BadHttp)?;
                    if n > self.maximum.saturating_sub(self.total) {
                        return Err(NetError::BodyTooLarge);
                    }
                    self.buffer.drain(..end + 2);
                    self.body = if n == 0 {
                        Body::Trailers
                    } else {
                        Body::Chunk(n)
                    };
                }
                Body::ChunkEnd => {
                    if self.buffer.len() < 2 {
                        break;
                    }
                    if &self.buffer[..2] != b"\r\n" {
                        return Err(NetError::BadHttp);
                    }
                    self.buffer.drain(..2);
                    self.body = Body::ChunkSize;
                }
                Body::Trailers => {
                    let Some(end) = self.buffer.windows(2).position(|w| w == b"\r\n") else {
                        if self.buffer.len() + self.trailers > 16384 {
                            return Err(NetError::BadHttp);
                        }
                        break;
                    };
                    self.trailers += end + 2;
                    if self.trailers > 16384 {
                        return Err(NetError::BadHttp);
                    }
                    self.buffer.drain(..end + 2);
                    if end == 0 {
                        self.body = Body::Done;
                    }
                }
                Body::Done => {
                    if !self.buffer.is_empty() {
                        return Err(NetError::BadHttp);
                    }
                    break;
                }
            }
        }
        Ok(())
    }
    fn emit(
        &mut self,
        count: usize,
        sink: &mut impl FnMut(u16, &[u8]) -> Result<(), NetError>,
    ) -> Result<(), NetError> {
        if count > self.maximum.saturating_sub(self.total) {
            return Err(NetError::BodyTooLarge);
        }
        sink(
            self.response.as_ref().ok_or(NetError::BadHttp)?.status,
            &self.buffer[..count],
        )?;
        self.total += count;
        self.buffer.drain(..count);
        Ok(())
    }
    pub fn done(&self) -> bool {
        matches!(self.body, Body::Done)
    }
    pub fn finish(self) -> Result<Response, NetError> {
        if !matches!(self.body, Body::Done | Body::Close) {
            return Err(NetError::BadHttp);
        }
        self.response.ok_or(NetError::BadHttp)
    }
}
