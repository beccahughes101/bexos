use alloc::vec::Vec;
use bexos_userspace::Channel;
use update_manager_fidl::{FidlEncode, HandleRef};

pub fn envelope(bytes: &[u8]) -> (u64, &[u8]) {
    if bytes.len() < 8 {
        return (0, &[]);
    }
    let mut raw = [0; 8];
    raw.copy_from_slice(&bytes[..8]);
    (u64::from_le_bytes(raw), &bytes[8..])
}

pub fn handle_refs(handles: &[u64]) -> Vec<HandleRef> {
    handles.iter().map(|raw| HandleRef { raw: *raw }).collect()
}

pub fn send_response<T: FidlEncode>(channel: Channel, response: &T) {
    let mut out = alloc::vec![0; 65500];
    let mut out_handles = [HandleRef { raw: 0 }; 8];
    if let Ok(encoded) = response.encode(&mut out, &mut out_handles) {
        let raw: Vec<_> = out_handles[..encoded.handles]
            .iter()
            .map(|handle| handle.raw)
            .collect();
        let _ = channel.send(&out[..encoded.bytes], &raw);
    }
}
