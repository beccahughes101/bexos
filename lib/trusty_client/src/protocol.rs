use alloc::string::String;
use alloc::vec::Vec;

pub const KEYMINT_UUID: [u8; 16] = [
    0x5f, 0x90, 0x2a, 0xce, 0x5e, 0x5c, 0x4c, 0xd8, 0xae, 0x54, 0x87, 0xb8, 0x8c, 0x22, 0xdd, 0xaf,
];
pub const GATEKEEPER_UUID: [u8; 16] = [
    0x38, 0xba, 0x0c, 0xdc, 0xdf, 0x0e, 0x11, 0xe4, 0x98, 0x69, 0x23, 0x3f, 0xb6, 0xae, 0x47, 0x95,
];
pub const AVB_UUID: [u8; 16] = [
    0x90, 0x5b, 0xcb, 0x84, 0x2d, 0xc4, 0x41, 0x9d, 0xb9, 0x51, 0x72, 0x55, 0x02, 0x7d, 0x31, 0x8c,
];
pub const AUTHMGR_FE_UUID: [u8; 16] = [
    0x9b, 0x3c, 0x1e, 0x9e, 0x18, 0x08, 0x4b, 0x98, 0x8f, 0xa9, 0x85, 0x92, 0xdf, 0xf3, 0xa3, 0x37,
];
pub const AUTHMGR_BE_UUID: [u8; 16] = [
    0xf4, 0x76, 0x89, 0x56, 0x62, 0xd9, 0x49, 0x04, 0x95, 0x12, 0x86, 0xdf, 0x36, 0x0d, 0x8d, 0x50,
];
pub const STORAGE_UUID: [u8; 16] = [
    0xce, 0xa8, 0x70, 0x6d, 0x6c, 0xb4, 0x49, 0xf3, 0xb9, 0x94, 0x29, 0xe0, 0xe4, 0x78, 0xbd, 0x29,
];
pub const ORCHESTRATOR_UUID: [u8; 16] = [
    0x2b, 0x45, 0x58, 0x4f, 0x53, 0x06, 0x40, 0x02, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x06,
];

pub const KEYMINT_PORT: &str = "com.android.trusty.keymint";
pub const KEYMINT_SECURE_PORT: &str = "com.android.trusty.keymaster.secure";
pub const GATEKEEPER_PORT: &str = "com.android.trusty.gatekeeper";
pub const AVB_PORT: &str = "com.android.trusty.avb";
pub const AUTHMGR_BE_PORT: &str = "com.android.trusty.rust.authmgr.V1";
pub const STORAGE_PROXY_PORT: &str = "com.android.trusty.storage.proxy";
pub const ORCHESTRATOR_PORT: &str = "com.bexos.orchestrator";

pub const MAX_ALIAS_LEN: usize = 128;
pub const MAX_AUTH_TOKEN_LEN: usize = 512;
pub const MAX_BLOB_LEN: usize = 4096;
pub const MAX_PAYLOAD_LEN: usize = 1024 * 1024;
pub const HW_AUTH_TOKEN_TIMEOUT_SECS: u64 = 300;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TrustyWireError {
    InvalidArgs,
    InvalidResponse,
    SecureService(i32),
}

pub type TrustyResult<T> = Result<T, TrustyWireError>;

pub struct Cursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Cursor<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    pub fn u8(&mut self) -> TrustyResult<u8> {
        let value = *self
            .bytes
            .get(self.offset)
            .ok_or(TrustyWireError::InvalidResponse)?;
        self.offset += 1;
        Ok(value)
    }

    pub fn u32(&mut self) -> TrustyResult<u32> {
        let end = self
            .offset
            .checked_add(4)
            .ok_or(TrustyWireError::InvalidResponse)?;
        let bytes = self
            .bytes
            .get(self.offset..end)
            .ok_or(TrustyWireError::InvalidResponse)?;
        self.offset = end;
        Ok(u32::from_le_bytes(bytes.try_into().unwrap()))
    }

    pub fn u64(&mut self) -> TrustyResult<u64> {
        let end = self
            .offset
            .checked_add(8)
            .ok_or(TrustyWireError::InvalidResponse)?;
        let bytes = self
            .bytes
            .get(self.offset..end)
            .ok_or(TrustyWireError::InvalidResponse)?;
        self.offset = end;
        Ok(u64::from_le_bytes(bytes.try_into().unwrap()))
    }

    pub fn bytes(&mut self) -> TrustyResult<&'a [u8]> {
        let len = usize::try_from(self.u32()?).map_err(|_| TrustyWireError::InvalidResponse)?;
        let end = self
            .offset
            .checked_add(len)
            .ok_or(TrustyWireError::InvalidResponse)?;
        let bytes = self
            .bytes
            .get(self.offset..end)
            .ok_or(TrustyWireError::InvalidResponse)?;
        self.offset = end;
        Ok(bytes)
    }

    pub fn finish(self) -> TrustyResult<()> {
        if self.offset == self.bytes.len() {
            Ok(())
        } else {
            Err(TrustyWireError::InvalidResponse)
        }
    }
}

pub fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

pub fn put_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}

pub fn put_bytes(out: &mut Vec<u8>, bytes: &[u8]) -> TrustyResult<()> {
    let len = u32::try_from(bytes.len()).map_err(|_| TrustyWireError::InvalidArgs)?;
    put_u32(out, len);
    out.extend_from_slice(bytes);
    Ok(())
}

pub fn validate_alias(alias: &str) -> TrustyResult<()> {
    if alias.is_empty()
        || alias.len() > MAX_ALIAS_LEN
        || alias
            .bytes()
            .any(|b| !b.is_ascii_alphanumeric() && b != b'.' && b != b'-' && b != b'_' && b != b':')
    {
        Err(TrustyWireError::InvalidArgs)
    } else {
        Ok(())
    }
}

pub fn service_ports() -> Vec<String> {
    [
        KEYMINT_PORT,
        KEYMINT_SECURE_PORT,
        GATEKEEPER_PORT,
        STORAGE_PROXY_PORT,
        AVB_PORT,
        AUTHMGR_BE_PORT,
        ORCHESTRATOR_PORT,
    ]
    .iter()
    .map(|port| (*port).into())
    .collect()
}
