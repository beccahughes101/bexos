use alloc::vec::Vec;
use bexos_userspace::Channel;
use kernel_fidl::{Status as KernelStatus, SystemPowerState as KernelSystemPowerState};
use power_fidl::{FidlEncode, HandleRef, Status, SystemPowerState};

pub fn envelope(bytes: &[u8]) -> (u64, &[u8]) {
    if bytes.len() < 8 {
        return (0, &[]);
    }
    let mut raw = [0; 8];
    raw.copy_from_slice(&bytes[..8]);
    (u64::from_le_bytes(raw), &bytes[8..])
}

pub fn metadata_protocol(metadata: &str) -> Option<&str> {
    metadata.split('|').nth(1)
}

pub fn handle_refs(handles: &[u64]) -> Vec<HandleRef> {
    handles.iter().map(|raw| HandleRef { raw: *raw }).collect()
}

pub fn send_response<T: FidlEncode>(channel: Channel, response: &T) {
    let mut out = [0; 256];
    let mut handles = [HandleRef { raw: 0 }; 4];
    if let Ok(encoded) = response.encode(&mut out, &mut handles) {
        let raw: Vec<_> = handles[..encoded.handles].iter().map(|h| h.raw).collect();
        let _ = channel.send(&out[..encoded.bytes], &raw);
    }
}

pub const fn to_kernel_state(state: SystemPowerState) -> KernelSystemPowerState {
    match state {
        SystemPowerState::Active => KernelSystemPowerState::Active,
        SystemPowerState::SuspendToRam => KernelSystemPowerState::SuspendToRam,
        SystemPowerState::SuspendToDisk => KernelSystemPowerState::SuspendToDisk,
        SystemPowerState::Reboot => KernelSystemPowerState::Reboot,
        SystemPowerState::Poweroff => KernelSystemPowerState::Poweroff,
    }
}

pub const fn from_kernel_status(status: KernelStatus) -> Status {
    match status {
        KernelStatus::Ok => Status::Ok,
        KernelStatus::ErrInvalidHandle => Status::ErrInvalidHandle,
        KernelStatus::ErrAccessDenied => Status::ErrAccessDenied,
        KernelStatus::ErrNoMemory => Status::ErrNoMemory,
        KernelStatus::ErrBufferTooSmall => Status::ErrBufferTooSmall,
        KernelStatus::ErrPeerClosed => Status::ErrPeerClosed,
        KernelStatus::ErrTimedOut => Status::ErrTimedOut,
        KernelStatus::ErrAlreadyExists => Status::ErrAlreadyExists,
        KernelStatus::ErrInvalidArgs => Status::ErrInvalidArgs,
        KernelStatus::ErrResourceExhausted => Status::ErrResourceExhausted,
    }
}
