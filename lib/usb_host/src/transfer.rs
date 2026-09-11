use usb_host_fidl::{TransferDirection, TransferRequest};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransferError {
    InvalidEndpoint,
    InvalidDirection,
    OutOfBounds,
    TooLarge,
    Timeout,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransferLimits {
    pub max_transfer_bytes: u32,
    pub max_buffers: u32,
    pub allow_control: bool,
}

impl Default for TransferLimits {
    fn default() -> Self {
        Self {
            max_transfer_bytes: 1024 * 1024,
            max_buffers: 1024,
            allow_control: true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BufferRegistration {
    pub id: u32,
    pub size_bytes: u64,
    pub direction: TransferDirection,
}

impl Default for BufferRegistration {
    fn default() -> Self {
        Self {
            id: 0,
            size_bytes: 0,
            direction: TransferDirection::None,
        }
    }
}

pub fn validate_buffer(
    offset: u64,
    size: u64,
    object_size: u64,
    direction: TransferDirection,
    limits: TransferLimits,
) -> Result<(), TransferError> {
    if matches!(direction, TransferDirection::None) {
        return Err(TransferError::InvalidDirection);
    }
    if size == 0 || size > u64::from(limits.max_transfer_bytes) {
        return Err(TransferError::TooLarge);
    }
    let end = offset.checked_add(size).ok_or(TransferError::OutOfBounds)?;
    if end > object_size {
        return Err(TransferError::OutOfBounds);
    }
    Ok(())
}

pub fn validate_transfer(
    request: &TransferRequest,
    buffer: Option<BufferRegistration>,
    limits: TransferLimits,
) -> Result<(), TransferError> {
    if request.transfer_id == 0 {
        return Err(TransferError::InvalidEndpoint);
    }
    let endpoint = request.endpoint & 0x0f;
    if endpoint == 0 && !limits.allow_control {
        return Err(TransferError::InvalidEndpoint);
    }
    if endpoint > 15 {
        return Err(TransferError::InvalidEndpoint);
    }
    if request.timeout_us == 0 {
        return Err(TransferError::Timeout);
    }
    if request.length > limits.max_transfer_bytes {
        return Err(TransferError::TooLarge);
    }
    if request.length == 0 {
        return Ok(());
    }
    let Some(buffer) = buffer else {
        return Err(TransferError::OutOfBounds);
    };
    if buffer.id != request.buffer_id {
        return Err(TransferError::OutOfBounds);
    }
    if !directions_compatible(request.direction, buffer.direction) {
        return Err(TransferError::InvalidDirection);
    }
    let end = request
        .buffer_offset
        .checked_add(u64::from(request.length))
        .ok_or(TransferError::OutOfBounds)?;
    if end > buffer.size_bytes {
        return Err(TransferError::OutOfBounds);
    }
    Ok(())
}

fn directions_compatible(request: TransferDirection, buffer: TransferDirection) -> bool {
    matches!(
        (request, buffer),
        (TransferDirection::In, TransferDirection::In)
            | (TransferDirection::Out, TransferDirection::Out)
            | (TransferDirection::None, _)
    )
}
