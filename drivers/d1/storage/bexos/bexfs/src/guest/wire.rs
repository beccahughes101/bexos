use alloc::vec;
use alloc::vec::Vec;
use bexos_userspace::Channel;
use fs_fidl::{FidlEncode, HandleRef};

pub(crate) fn refs(handles: &[u64]) -> Vec<HandleRef> {
    handles
        .iter()
        .map(|handle| HandleRef { raw: *handle })
        .collect()
}

pub(crate) fn envelope(bytes: &[u8]) -> (u64, &[u8]) {
    assert!(bytes.len() >= 8);
    (
        u64::from_le_bytes(bytes[..8].try_into().unwrap()),
        &bytes[8..],
    )
}

pub(crate) fn reply<Q: FidlEncode>(channel: Channel, response: &Q) {
    let mut bytes = vec![0; 65500];
    let mut handles = [HandleRef { raw: 0 }; 16];
    let encoded = response
        .encode(&mut bytes, &mut handles)
        .expect("filesystem encode");
    // A client can close its endpoint while a durability operation is still
    // completing. The filesystem remains healthy even though that reply no
    // longer has a receiver, so do not terminate the whole service.
    let _ = channel.send(
        &bytes[..encoded.bytes],
        &handles[..encoded.handles]
            .iter()
            .map(|handle| handle.raw)
            .collect::<Vec<_>>(),
    );
}
