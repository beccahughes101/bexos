//! Query the actual host Vulkan implementation through the guest Venus context.
//! Ordinals come from Bazel's pinned Mesa protocol; this is a transport probe,
//! independent of and insufficient to validate the full Mesa/wgpu renderer.
use mesa_venus_protocol::{ENUMERATE_INSTANCE_VERSION, GENERATE_REPLY, SET_REPLY_STREAM};

pub const REPLY_OFFSET: usize = 64;
pub fn command(resource: u32) -> [u8; 52] {
    let mut data = [0; 52];
    // SetReplyStream: opcode, flags, non-null pointer, resource, offset, size.
    data[0..4].copy_from_slice(&SET_REPLY_STREAM.to_le_bytes());
    data[8..16].copy_from_slice(&1u64.to_le_bytes());
    data[16..20].copy_from_slice(&resource.to_le_bytes());
    data[20..28].copy_from_slice(&(REPLY_OFFSET as u64).to_le_bytes());
    data[28..36].copy_from_slice(&64u64.to_le_bytes());
    // EnumerateInstanceVersion: opcode, generate-reply, non-null output pointer.
    data[36..40].copy_from_slice(&ENUMERATE_INSTANCE_VERSION.to_le_bytes());
    data[40..44].copy_from_slice(&GENERATE_REPLY.to_le_bytes());
    data[44..52].copy_from_slice(&1u64.to_le_bytes());
    data
}
pub fn decode(reply: &[u8; 20]) -> Option<u32> {
    let word = |offset| u32::from_le_bytes(reply[offset..offset + 4].try_into().unwrap());
    if word(0) != ENUMERATE_INSTANCE_VERSION
        || word(4) != 0
        || u64::from_le_bytes(reply[8..16].try_into().unwrap()) != 1
    {
        return None;
    }
    let version = word(16);
    // Vulkan variant zero, major version one, at least 1.1 for Venus.
    (version >> 22 == 1 && ((version >> 12) & 1023) >= 1).then_some(version)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reply_rejects_wrong_command_status_pointer_and_version() {
        let mut reply = [0; 20];
        reply[..4].copy_from_slice(&ENUMERATE_INSTANCE_VERSION.to_le_bytes());
        reply[8..16].copy_from_slice(&1u64.to_le_bytes());
        let version = (1u32 << 22) | (3 << 12) | 7;
        reply[16..].copy_from_slice(&version.to_le_bytes());
        assert_eq!(decode(&reply), Some(version));
        for index in [0, 4, 8, 12, 19] {
            let mut invalid = reply;
            invalid[index] ^= 128;
            assert_eq!(decode(&invalid), None);
        }
        assert!(command(9)[4..8].iter().all(|v| *v == 0));
    }
}
