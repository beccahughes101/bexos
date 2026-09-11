use alloc::vec::Vec;

use crate::NetError;

pub const DNS_MESSAGE_CONTENT_TYPE: &str = "application/dns-message";

pub fn encode_post(path: &str, authority: &str, dns_message: &[u8]) -> Result<Vec<u8>, NetError> {
    if path.is_empty() || authority.is_empty() || dns_message.is_empty() {
        return Err(NetError::InvalidArgs);
    }
    let mut out = Vec::new();
    out.extend_from_slice(b"POST ");
    out.extend_from_slice(path.as_bytes());
    out.extend_from_slice(b" HTTP/1.1\r\nHost: ");
    out.extend_from_slice(authority.as_bytes());
    out.extend_from_slice(b"\r\nContent-Type: application/dns-message\r\nAccept: application/dns-message\r\nContent-Length: ");
    append_decimal(dns_message.len(), &mut out);
    out.extend_from_slice(b"\r\nConnection: close\r\n\r\n");
    out.extend_from_slice(dns_message);
    Ok(out)
}

fn append_decimal(mut value: usize, out: &mut Vec<u8>) {
    let mut digits = [0u8; 20];
    let mut len = 0;
    loop {
        digits[len] = b'0' + (value % 10) as u8;
        len += 1;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    for i in (0..len).rev() {
        out.push(digits[i]);
    }
}
