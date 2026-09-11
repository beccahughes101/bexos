use alloc::vec::Vec;

use crate::NetError;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TlsConfig {
    pub server_name: alloc::string::String,
    pub alpn: Vec<Vec<u8>>,
    pub root_generation: u64,
}

impl TlsConfig {
    pub fn new(server_name: &str, root_generation: u64) -> Result<Self, NetError> {
        if server_name.is_empty() || server_name.len() > 255 {
            return Err(NetError::InvalidArgs);
        }
        Ok(Self {
            server_name: server_name.into(),
            alpn: Vec::new(),
            root_generation,
        })
    }
}
