#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PsciFunction {
    Version,
    CpuSuspend64,
    CpuOn64,
    SystemOff,
    SystemReset,
    Features,
}

impl PsciFunction {
    pub const fn id(self) -> u64 {
        match self {
            Self::Version => 0x8400_0000,
            Self::CpuSuspend64 => 0xc400_0001,
            Self::CpuOn64 => 0xc400_0003,
            Self::SystemOff => 0x8400_0008,
            Self::SystemReset => 0x8400_0009,
            Self::Features => 0x8400_000a,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PsciVersion {
    pub major: u16,
    pub minor: u16,
}

impl PsciVersion {
    pub const fn decode(value: u64) -> Result<Self, PsciError> {
        let signed = value as i64;
        if signed < 0 {
            return Err(PsciError::from_i64(signed));
        }
        Ok(Self {
            major: ((value >> 16) & 0xffff) as u16,
            minor: (value & 0xffff) as u16,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PsciError {
    NotSupported,
    InvalidParameters,
    Denied,
    AlreadyOn,
    OnPending,
    InternalFailure,
    NotPresent,
    Disabled,
    InvalidAddress,
    UnexpectedReturn,
    Unknown(i64),
}

impl PsciError {
    pub const fn from_i64(value: i64) -> Self {
        match value {
            -1 => Self::NotSupported,
            -2 => Self::InvalidParameters,
            -3 => Self::Denied,
            -4 => Self::AlreadyOn,
            -5 => Self::OnPending,
            -6 => Self::InternalFailure,
            -7 => Self::NotPresent,
            -8 => Self::Disabled,
            -9 => Self::InvalidAddress,
            other => Self::Unknown(other),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SystemPowerOperation {
    Standby,
    Reboot,
    Poweroff,
}

pub const STANDBY_POWER_STATE: u64 = 0;

pub const fn decode_status(value: u64) -> Result<(), PsciError> {
    let signed = value as i64;
    if signed < 0 {
        Err(PsciError::from_i64(signed))
    } else {
        Ok(())
    }
}

pub const fn decode_feature_status(value: u64) -> Result<(), PsciError> {
    decode_status(value)
}

pub const fn function_for_power_operation(operation: SystemPowerOperation) -> PsciFunction {
    match operation {
        SystemPowerOperation::Standby => PsciFunction::CpuSuspend64,
        SystemPowerOperation::Reboot => PsciFunction::SystemReset,
        SystemPowerOperation::Poweroff => PsciFunction::SystemOff,
    }
}
