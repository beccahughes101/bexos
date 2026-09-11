use bexos_userspace::{Channel, Memory};
use block_fidl::{FidlEncode, HandleRef};

pub fn envelope(bytes: &[u8]) -> Option<(u64, &[u8])> {
    Some((
        u64::from_le_bytes(bytes.get(..8)?.try_into().ok()?),
        bytes.get(8..)?,
    ))
}

pub fn close(handles: &[u64]) {
    for handle in handles {
        let _ = Memory::close(*handle);
    }
}

pub fn refs(handles: &[u64]) -> alloc::vec::Vec<HandleRef> {
    handles.iter().map(|raw| HandleRef { raw: *raw }).collect()
}

pub fn reply<T: FidlEncode>(channel: Channel, response: &T) {
    let mut out = [0; 512];
    let mut handles = [HandleRef { raw: 0 }; 2];
    if let Ok(encoded) = response.encode(&mut out, &mut handles) {
        let owned: alloc::vec::Vec<_> = handles[..encoded.handles]
            .iter()
            .map(|handle| handle.raw)
            .collect();
        if channel.send(&out[..encoded.bytes], &owned).is_err() {
            close(&owned);
        }
    }
}
