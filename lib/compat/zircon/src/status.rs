use kernel_fidl::Status as KernelStatus;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i32)]
pub enum Status {
    Ok = 0,
    InvalidArgs = -1,
    AccessDenied = -2,
    BadHandle = -3,
    AlreadyExists = -4,
    NotFound = -5,
    NoMemory = -6,
    TimedOut = -7,
    BadState = -8,
    PeerClosed = -9,
    NotSupported = -10,
    Internal = -127,
}

pub type Result<T> = core::result::Result<T, Status>;

impl From<KernelStatus> for Status {
    fn from(value: KernelStatus) -> Self {
        match value {
            KernelStatus::Ok => Self::Ok,
            KernelStatus::ErrInvalidHandle => Self::BadHandle,
            KernelStatus::ErrAccessDenied => Self::AccessDenied,
            KernelStatus::ErrNoMemory => Self::NoMemory,
            KernelStatus::ErrBufferTooSmall => Self::InvalidArgs,
            KernelStatus::ErrPeerClosed => Self::PeerClosed,
            KernelStatus::ErrTimedOut => Self::TimedOut,
            KernelStatus::ErrAlreadyExists => Self::AlreadyExists,
            KernelStatus::ErrInvalidArgs => Self::InvalidArgs,
            KernelStatus::ErrResourceExhausted => Self::BadState,
        }
    }
}

impl Status {
    pub const fn into_raw(self) -> i32 {
        self as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kernel_status_translation_is_stable() {
        assert_eq!(
            Status::from(KernelStatus::ErrInvalidArgs),
            Status::InvalidArgs
        );
        assert_eq!(Status::from(KernelStatus::ErrAccessDenied).into_raw(), -2);
    }
}
