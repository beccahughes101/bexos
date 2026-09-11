use alloc::vec::Vec;

use crate::NetError;

pub const NTS_KE_PORT: u16 = 4460;
pub const NTP_PORT: u16 = 123;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NtsState {
    pub server: alloc::string::String,
    pub c2s_key: Vec<u8>,
    pub s2c_key: Vec<u8>,
    pub cookies: Vec<Vec<u8>>,
}

impl NtsState {
    pub fn usable(&self) -> bool {
        !self.server.is_empty()
            && self.c2s_key.len() == 32
            && self.s2c_key.len() == 32
            && !self.cookies.is_empty()
    }
}

pub fn validate_cookie(cookie: &[u8]) -> Result<(), NetError> {
    if cookie.len() < 16 || cookie.len() > 1024 {
        Err(NetError::InvalidArgs)
    } else {
        Ok(())
    }
}
