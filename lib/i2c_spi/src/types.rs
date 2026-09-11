use alloc::vec::Vec;

pub const MAX_OPERATIONS_PER_BUNDLE: usize = 16;
pub const MAX_TRANSFER_BYTES_PER_BUNDLE: usize = 4096;
pub const MAX_PENDING_BUNDLES_PER_CONTROLLER: usize = 64;
pub const MAX_REQUEST_DEADLINE_MS: u64 = 1000;
pub const MAX_LOCK_LEASE_MS: u64 = 100;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Status {
    Ok,
    InvalidHandle,
    AccessDenied,
    NoMemory,
    BufferTooSmall,
    PeerClosed,
    TimedOut,
    AlreadyExists,
    InvalidArgs,
    NotFound,
    BadState,
    Unsupported,
    Nack,
    ArbitrationLost,
    QueueFull,
    TooLarge,
    LockExpired,
    InjectedFailure,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BusKind {
    I2c,
    Spi,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum I2cAddressKind {
    SevenBit,
    TenBit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct I2cAddress {
    pub raw: u16,
    pub kind: I2cAddressKind,
}

impl I2cAddress {
    pub fn new(raw: u16, ten_bit: bool) -> Result<Self, Status> {
        let kind = if ten_bit {
            if raw > 0x03ff {
                return Err(Status::InvalidArgs);
            }
            I2cAddressKind::TenBit
        } else {
            if raw > 0x007f {
                return Err(Status::InvalidArgs);
            }
            I2cAddressKind::SevenBit
        };
        Ok(Self { raw, kind })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum I2cOp {
    Write(Vec<u8>),
    Read(usize),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct I2cBundle {
    pub operations: Vec<I2cOp>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SpiMode {
    Mode0,
    Mode1,
    Mode2,
    Mode3,
}

impl SpiMode {
    pub fn bit(self) -> u8 {
        1 << (self as u8)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SpiOp {
    Write(Vec<u8>),
    Read(usize),
    FullDuplex(Vec<u8>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpiBundle {
    pub operations: Vec<SpiOp>,
    pub speed_hz: u32,
    pub mode: SpiMode,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Request {
    I2c(I2cBundle),
    Spi(SpiBundle),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReadChunk {
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransferOutcome {
    pub status: Status,
    pub reads: Vec<ReadChunk>,
    pub operations_completed: u32,
}

impl TransferOutcome {
    pub fn ok(reads: Vec<ReadChunk>, operations_completed: u32) -> Self {
        Self {
            status: Status::Ok,
            reads,
            operations_completed,
        }
    }
}

pub fn validate_i2c_bundle(bundle: &I2cBundle) -> Result<usize, Status> {
    validate_ops(!bundle.operations.is_empty(), bundle.operations.len())?;
    let mut total = 0usize;
    for op in &bundle.operations {
        total = total
            .checked_add(match op {
                I2cOp::Write(bytes) => bytes.len(),
                I2cOp::Read(len) => *len,
            })
            .ok_or(Status::TooLarge)?;
    }
    if total > MAX_TRANSFER_BYTES_PER_BUNDLE {
        return Err(Status::TooLarge);
    }
    Ok(total)
}

pub fn validate_spi_bundle(bundle: &SpiBundle) -> Result<usize, Status> {
    validate_ops(!bundle.operations.is_empty(), bundle.operations.len())?;
    let mut total = 0usize;
    for op in &bundle.operations {
        total = total
            .checked_add(match op {
                SpiOp::Write(bytes) | SpiOp::FullDuplex(bytes) => bytes.len(),
                SpiOp::Read(len) => *len,
            })
            .ok_or(Status::TooLarge)?;
    }
    if total > MAX_TRANSFER_BYTES_PER_BUNDLE {
        return Err(Status::TooLarge);
    }
    Ok(total)
}

fn validate_ops(non_empty: bool, count: usize) -> Result<(), Status> {
    if !non_empty {
        return Err(Status::InvalidArgs);
    }
    if count > MAX_OPERATIONS_PER_BUNDLE {
        return Err(Status::TooLarge);
    }
    Ok(())
}
