//! Terminal RPCs over BXD1. Socket handles stay on the target.
use super::*;
pub const METHOD_SHELL_OPEN: u32 = 112;
pub const METHOD_SHELL_EXCHANGE: u32 = 113;
pub const METHOD_SHELL_RESIZE: u32 = 114;
pub const METHOD_SHELL_MODE: u32 = 115;
pub const METHOD_SHELL_SIGNAL: u32 = 116;
pub const METHOD_SHELL_CLOSE: u32 = 117;
pub const SHELL_CHUNK: usize = 16 * 1024;
/// Deliberately does not implement Debug: opening a session contains credentials.
#[derive(Default)]
pub struct ShellRequest {
    pub system: bool,
    pub uid: u64,
    pub user: String,
    pub password: String,
    pub session_id: u64,
    pub input: Vec<u8>,
    pub eof: bool,
    pub rows: u16,
    pub cols: u16,
    pub pixel_width: u32,
    pub pixel_height: u32,
    pub mode: u32,
    pub signal: u32,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ShellResponse {
    pub status: i32,
    pub message: String,
    pub session_id: u64,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub consumed: u32,
    pub exited: bool,
    pub exit_code: i32,
    pub uid: u64,
    pub provider: String,
}
pub fn encode_shell_request(q: &ShellRequest, out: &mut Vec<u8>) {
    out.clear();
    for (f, v) in [
        (1, q.system as u64),
        (2, q.uid),
        (5, q.session_id),
        (7, q.eof as u64),
        (8, q.rows as u64),
        (9, q.cols as u64),
        (10, q.pixel_width as u64),
        (11, q.pixel_height as u64),
        (12, q.mode as u64),
        (13, q.signal as u64),
    ] {
        put_varint_field(out, f, v);
    }
    put_string(out, 3, &q.user);
    put_string(out, 4, &q.password);
    put_len_field(out, 6, &q.input);
}
pub fn decode_shell_request(bytes: &[u8]) -> Result<ShellRequest, WireError> {
    let mut q = ShellRequest::default();
    read_fields(bytes, |f, w, v| {
        match (f, w) {
            (1, 0) => q.system = read_varint_from(v)? != 0,
            (2, 0) => q.uid = read_varint_from(v)?,
            (3, 2) => q.user = read_string(v)?,
            (4, 2) => q.password = read_string(v)?,
            (5, 0) => q.session_id = read_varint_from(v)?,
            (6, 2) => q.input = v.to_vec(),
            (7, 0) => q.eof = read_varint_from(v)? != 0,
            (8, 0) => {
                q.rows = u16::try_from(read_varint_from(v)?).map_err(|_| WireError::InvalidProto)?
            }
            (9, 0) => {
                q.cols = u16::try_from(read_varint_from(v)?).map_err(|_| WireError::InvalidProto)?
            }
            (10, 0) => {
                q.pixel_width =
                    u32::try_from(read_varint_from(v)?).map_err(|_| WireError::InvalidProto)?
            }
            (11, 0) => {
                q.pixel_height =
                    u32::try_from(read_varint_from(v)?).map_err(|_| WireError::InvalidProto)?
            }
            (12, 0) => {
                q.mode = u32::try_from(read_varint_from(v)?).map_err(|_| WireError::InvalidProto)?
            }
            (13, 0) => {
                q.signal =
                    u32::try_from(read_varint_from(v)?).map_err(|_| WireError::InvalidProto)?
            }
            _ => {}
        }
        Ok(())
    })?;
    if q.input.len() > SHELL_CHUNK || q.user.len() > 64 || q.password.len() > 256 {
        return Err(WireError::PayloadTooLarge);
    }
    Ok(q)
}
pub fn encode_shell_response(q: &ShellResponse, out: &mut Vec<u8>) {
    out.clear();
    for (f, v) in [
        (1, q.status as u64),
        (3, q.session_id),
        (6, q.consumed as u64),
        (7, q.exited as u64),
        (8, q.exit_code as u64),
        (9, q.uid),
    ] {
        put_varint_field(out, f, v);
    }
    put_string(out, 2, &q.message);
    put_len_field(out, 4, &q.stdout);
    put_len_field(out, 5, &q.stderr);
    put_string(out, 10, &q.provider);
}
pub fn decode_shell_response(bytes: &[u8]) -> Result<ShellResponse, WireError> {
    let mut q = ShellResponse::default();
    read_fields(bytes, |f, w, v| {
        match (f, w) {
            (1, 0) => q.status = read_varint_from(v)? as i32,
            (2, 2) => q.message = read_string(v)?,
            (3, 0) => q.session_id = read_varint_from(v)?,
            (4, 2) => q.stdout = v.to_vec(),
            (5, 2) => q.stderr = v.to_vec(),
            (6, 0) => {
                q.consumed =
                    u32::try_from(read_varint_from(v)?).map_err(|_| WireError::InvalidProto)?
            }
            (7, 0) => q.exited = read_varint_from(v)? != 0,
            (8, 0) => q.exit_code = read_varint_from(v)? as i32,
            (9, 0) => q.uid = read_varint_from(v)?,
            (10, 2) => q.provider = read_string(v)?,
            _ => {}
        }
        Ok(())
    })?;
    if q.stdout.len() > SHELL_CHUNK || q.stderr.len() > SHELL_CHUNK {
        return Err(WireError::PayloadTooLarge);
    }
    Ok(q)
}
