use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::NetError;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequestHead {
    pub method: String,
    pub authority: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
}

pub fn encode_get(authority: &str, path: &str, out: &mut Vec<u8>) -> Result<(), NetError> {
    if authority.is_empty() || path.is_empty() || !path.starts_with('/') {
        return Err(NetError::InvalidArgs);
    }
    out.extend_from_slice(b"GET ");
    out.extend_from_slice(path.as_bytes());
    out.extend_from_slice(b" HTTP/1.1\r\nHost: ");
    out.extend_from_slice(authority.as_bytes());
    out.extend_from_slice(b"\r\nConnection: close\r\n\r\n");
    Ok(())
}

pub fn encode_get_with_user_agent(
    authority: &str,
    path: &str,
    user_agent: &str,
    out: &mut Vec<u8>,
) -> Result<(), NetError> {
    if authority.is_empty()
        || path.is_empty()
        || !path.starts_with('/')
        || user_agent.is_empty()
        || user_agent.len() > 128
    {
        return Err(NetError::InvalidArgs);
    }
    out.extend_from_slice(b"GET ");
    out.extend_from_slice(path.as_bytes());
    out.extend_from_slice(b" HTTP/1.1\r\nHost: ");
    out.extend_from_slice(authority.as_bytes());
    out.extend_from_slice(b"\r\nConnection: close\r\nUser-Agent: ");
    out.extend_from_slice(user_agent.as_bytes());
    out.extend_from_slice(b"\r\nAccept: */*\r\n\r\n");
    Ok(())
}

pub fn status_code(response: &[u8]) -> Result<u16, NetError> {
    let line_end = response
        .windows(2)
        .position(|w| w == b"\r\n")
        .ok_or(NetError::InvalidArgs)?;
    let line = core::str::from_utf8(&response[..line_end]).map_err(|_| NetError::InvalidArgs)?;
    let mut parts = line.split(' ');
    if parts.next() != Some("HTTP/1.1") {
        return Err(NetError::InvalidArgs);
    }
    parts
        .next()
        .and_then(|code| code.parse().ok())
        .ok_or(NetError::InvalidArgs)
}

pub fn parse_response(bytes: &[u8], cap: usize) -> Result<Response, NetError> {
    let header_end = bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or(NetError::BadHttp)?
        + 4;
    let headers = core::str::from_utf8(&bytes[..header_end]).map_err(|_| NetError::BadHttp)?;
    let mut lines = headers.split("\r\n");
    let status = lines
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or(NetError::BadHttp)?;
    let mut parsed_headers = Vec::new();
    let mut content_len = None;
    let mut chunked = false;
    for line in lines.filter(|line| !line.is_empty()) {
        let (name, value) = line.split_once(':').ok_or(NetError::BadHttp)?;
        let value = value.trim().to_string();
        if name.eq_ignore_ascii_case("content-length") {
            if content_len.is_some() || chunked {
                return Err(NetError::BadHttp);
            }
            content_len = Some(value.parse::<usize>().map_err(|_| NetError::BadHttp)?);
        }
        if name.eq_ignore_ascii_case("transfer-encoding") {
            if content_len.is_some() || !value.eq_ignore_ascii_case("chunked") {
                return Err(NetError::BadHttp);
            }
            chunked = true;
        }
        parsed_headers.push((name.to_string(), value));
    }
    let body = &bytes[header_end..];
    let body = if chunked {
        decode_chunked_body(body, cap)?
    } else if let Some(expected) = content_len {
        if expected != body.len() {
            return Err(NetError::BadHttp);
        }
        if body.len() > cap {
            return Err(NetError::BodyTooLarge);
        }
        body.to_vec()
    } else {
        if body.len() > cap {
            return Err(NetError::BodyTooLarge);
        }
        body.to_vec()
    };
    Ok(Response {
        status,
        headers: parsed_headers,
        body,
    })
}

fn decode_chunked_body(bytes: &[u8], cap: usize) -> Result<Vec<u8>, NetError> {
    let mut offset = 0usize;
    let mut out = Vec::new();
    loop {
        let line_end = find_crlf(&bytes[offset..]).ok_or(NetError::BadHttp)? + offset;
        let line = core::str::from_utf8(&bytes[offset..line_end]).map_err(|_| NetError::BadHttp)?;
        let size_text = line.split(';').next().ok_or(NetError::BadHttp)?;
        let size = usize::from_str_radix(size_text.trim(), 16).map_err(|_| NetError::BadHttp)?;
        offset = line_end.checked_add(2).ok_or(NetError::BadHttp)?;
        if size == 0 {
            let trailer_end = bytes[offset..]
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .map(|pos| offset + pos + 4)
                .or_else(|| {
                    bytes[offset..]
                        .starts_with(b"\r\n")
                        .then_some(offset.saturating_add(2))
                })
                .ok_or(NetError::BadHttp)?;
            if trailer_end != bytes.len() {
                return Err(NetError::BadHttp);
            }
            return Ok(out);
        }
        let end = offset.checked_add(size).ok_or(NetError::BodyTooLarge)?;
        if end.checked_add(2).ok_or(NetError::BadHttp)? > bytes.len()
            || &bytes[end..end + 2] != b"\r\n"
        {
            return Err(NetError::BadHttp);
        }
        out.extend_from_slice(&bytes[offset..end]);
        if out.len() > cap {
            return Err(NetError::BodyTooLarge);
        }
        offset = end + 2;
    }
}

fn find_crlf(bytes: &[u8]) -> Option<usize> {
    bytes.windows(2).position(|window| window == b"\r\n")
}
