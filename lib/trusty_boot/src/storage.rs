//! Heap-free boot storage proxy. Only opaque authenticated RPMB frames cross
//! this boundary; the bootloader neither knows the key nor interprets frames.
use crate::ql::Error;

pub const MAX_FRAMES: usize = 4096;
pub const MAX_RESPONSE: usize = 24 + MAX_FRAMES;
pub trait Rpmb {
    fn exchange(&mut self, request: &[u8], response: &mut [u8]) -> Result<(), Error>;
}

pub fn dispatch(
    rpmb: &mut impl Rpmb,
    request: &[u8],
    response: &mut [u8; MAX_RESPONSE],
) -> Result<usize, Error> {
    let word = |offset| u32::from_le_bytes(request[offset..offset + 4].try_into().unwrap());
    if request.len() < 24
        || word(0) & 1 != 0
        || word(12) as usize != request.len()
        || word(16) != 0
        || word(20) != 0
    {
        return Err(Error::InvalidResponse);
    }
    response.fill(0);
    response[..24].copy_from_slice(&request[..24]);
    response[..4].copy_from_slice(&(word(0) | 1).to_le_bytes());
    response[8..12].fill(0);
    let mut size = 24;
    let result = if word(8) & !0x1e != 0 {
        2u32
    } else if word(0) == 16 {
        if request.len() < 40 {
            2
        } else {
            let reliable = u64::from(word(24));
            let written = u64::from(word(28));
            let read = word(32) as usize;
            let total = reliable + written;
            if total == 0
                || total > MAX_FRAMES as u64
                || total != (request.len() - 40) as u64
                || reliable % 512 != 0
                || written % 512 != 0
                || read == 0
                || read > MAX_FRAMES
                || read % 512 != 0
                || word(36) != 0
            {
                2
            } else {
                rpmb.exchange(&request[40..], &mut response[24..24 + read])?;
                size += read;
                0
            }
        }
    } else if word(0) == 4 {
        5
    } else {
        3
    };
    response[12..16].copy_from_slice(&(size as u32).to_le_bytes());
    response[16..20].copy_from_slice(&result.to_le_bytes());
    Ok(size)
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Device(usize);
    impl Rpmb for Device {
        fn exchange(&mut self, request: &[u8], response: &mut [u8]) -> Result<(), Error> {
            self.0 += 1;
            assert_eq!(request, [0xa5; 512]);
            assert_eq!(response.len(), 512);
            response.fill(0x5a);
            Ok(())
        }
    }
    fn request() -> [u8; 552] {
        let mut bytes = [0; 552];
        for (offset, value) in [(0, 16u32), (4, 99), (12, 552), (24, 512), (32, 512)] {
            bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        bytes[40..].fill(0xa5);
        bytes
    }
    #[test]
    fn authenticated_frames_are_opaque_and_reply_identity_is_retained() {
        let mut device = Device(0);
        let mut out = [0; MAX_RESPONSE];
        assert_eq!(dispatch(&mut device, &request(), &mut out), Ok(536));
        assert_eq!(device.0, 1);
        assert_eq!(&out[4..8], &99u32.to_le_bytes());
        assert_eq!(&out[24..536], &[0x5a; 512]);
        assert_eq!(&out[16..24], &[0; 8]);
    }
    #[test]
    fn malformed_lengths_flags_and_reserved_fields_never_reach_rpmb() {
        let mut device = Device(0);
        let mut out = [0; MAX_RESPONSE];
        for (offset, value) in [
            (0, 17u32),
            (8, 1),
            (12, 551),
            (16, 1),
            (20, 1),
            (24, u32::MAX),
            (28, u32::MAX),
            (32, 0),
            (32, 4097),
            (32, 511),
            (36, 1),
        ] {
            let mut input = request();
            input[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
            let result = dispatch(&mut device, &input, &mut out);
            assert!(result.is_err() || out[16..20] == 2u32.to_le_bytes());
        }
        for size in 0..40 {
            assert!(dispatch(&mut device, &request()[..size], &mut out).is_err());
        }
        assert_eq!(device.0, 0);
    }
}
