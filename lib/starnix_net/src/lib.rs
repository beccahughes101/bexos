//! RFC 70 Linux network-namespace state independent of Linux ABI marshalling.
//!
//! Host RFC 68 policy is represented by immutable attachment metadata. Linux
//! mutations are confined to namespaces and can never rewrite VLANs, physical
//! selectors, provider domains, or other platform-owned topology.

mod namespace;
mod netfilter;
mod netlink;
mod snapshot;

pub use namespace::*;
pub use netfilter::*;
pub use snapshot::{SNAPSHOT_VERSION, SnapshotError};

pub const CAP_NET_ADMIN: u64 = 1 << 12;
pub const CAP_NET_RAW: u64 = 1 << 13;
pub const CAP_SYS_ADMIN: u64 = 1 << 21;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Capabilities {
    pub permitted: u64,
    pub effective: u64,
    pub inheritable: u64,
}

impl Capabilities {
    pub fn root_network_defaults() -> Self {
        let value = CAP_NET_ADMIN | CAP_NET_RAW | CAP_SYS_ADMIN;
        Self {
            permitted: value,
            effective: value,
            inheritable: 0,
        }
    }

    pub fn has(self, capability: u64) -> bool {
        self.effective & capability != 0
    }

    pub fn set(
        &mut self,
        permitted: u64,
        effective: u64,
        inheritable: u64,
    ) -> Result<(), NetError> {
        let supported = CAP_NET_ADMIN | CAP_NET_RAW | CAP_SYS_ADMIN;
        if permitted & !supported != 0
            || effective & !permitted != 0
            || inheritable & !supported != 0
            || permitted & !self.permitted != 0
        {
            return Err(NetError::PermissionDenied);
        }
        self.permitted = permitted;
        self.effective = effective;
        self.inheritable = inheritable;
        Ok(())
    }

    pub fn apply_executable(
        &mut self,
        permitted: u64,
        inheritable: u64,
        effective: bool,
    ) -> Result<(), NetError> {
        let supported = CAP_NET_ADMIN | CAP_NET_RAW | CAP_SYS_ADMIN;
        if permitted & !supported != 0 || inheritable & !supported != 0 {
            return Err(NetError::Unsupported);
        }
        self.permitted = permitted | (self.inheritable & inheritable);
        self.effective = if effective { self.permitted } else { 0 };
        self.inheritable &= supported;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NetError {
    InvalidArgument,
    NotFound,
    AlreadyExists,
    PermissionDenied,
    Unsupported,
    ResourceExhausted,
    WouldBlock,
    NetworkUnreachable,
    CorruptSnapshot,
    UnsupportedSnapshot,
}
