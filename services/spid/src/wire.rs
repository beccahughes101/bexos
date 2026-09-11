use bexos_i2c_spi::{ReadChunk, SpiMode, Status, TransferOutcome};
use bexos_userspace::{Channel, Memory};
use i2c_spi_fidl::{FidlEncode, HandleRef};

pub fn envelope(bytes: &[u8]) -> Option<(u64, &[u8])> {
    Some((
        u64::from_le_bytes(bytes.get(..8)?.try_into().ok()?),
        bytes.get(8..)?,
    ))
}

pub fn close(handles: &[u64]) {
    for handle in handles {
        let _ = Memory::close(*handle);
    }
}

pub fn reply<T: FidlEncode>(channel: Channel, response: &T) {
    let mut out = [0; 8192];
    let mut handles = [HandleRef { raw: 0 }; 4];
    if let Ok(encoded) = response.encode(&mut out, &mut handles) {
        let owned: alloc::vec::Vec<_> = handles[..encoded.handles]
            .iter()
            .map(|handle| handle.raw)
            .collect();
        if channel.send(&out[..encoded.bytes], &owned).is_err() {
            close(&owned);
        }
    }
}

pub fn status(status: Status) -> i2c_spi_fidl::Status {
    match status {
        Status::Ok => i2c_spi_fidl::Status::Ok,
        Status::InvalidHandle => i2c_spi_fidl::Status::ErrInvalidHandle,
        Status::AccessDenied => i2c_spi_fidl::Status::ErrAccessDenied,
        Status::NoMemory => i2c_spi_fidl::Status::ErrNoMemory,
        Status::BufferTooSmall => i2c_spi_fidl::Status::ErrBufferTooSmall,
        Status::PeerClosed => i2c_spi_fidl::Status::ErrPeerClosed,
        Status::TimedOut => i2c_spi_fidl::Status::ErrTimedOut,
        Status::AlreadyExists => i2c_spi_fidl::Status::ErrAlreadyExists,
        Status::InvalidArgs => i2c_spi_fidl::Status::ErrInvalidArgs,
        Status::NotFound => i2c_spi_fidl::Status::ErrNotFound,
        Status::BadState => i2c_spi_fidl::Status::ErrBadState,
        Status::Unsupported => i2c_spi_fidl::Status::ErrUnsupported,
        Status::Nack => i2c_spi_fidl::Status::ErrNack,
        Status::ArbitrationLost => i2c_spi_fidl::Status::ErrArbitrationLost,
        Status::QueueFull => i2c_spi_fidl::Status::ErrQueueFull,
        Status::TooLarge => i2c_spi_fidl::Status::ErrTooLarge,
        Status::LockExpired => i2c_spi_fidl::Status::ErrLockExpired,
        Status::InjectedFailure => i2c_spi_fidl::Status::ErrInjectedFailure,
    }
}

pub fn spi_mode(mode: i2c_spi_fidl::SpiMode) -> SpiMode {
    match mode {
        i2c_spi_fidl::SpiMode::Mode0 => SpiMode::Mode0,
        i2c_spi_fidl::SpiMode::Mode1 => SpiMode::Mode1,
        i2c_spi_fidl::SpiMode::Mode2 => SpiMode::Mode2,
        i2c_spi_fidl::SpiMode::Mode3 => SpiMode::Mode3,
    }
}

pub fn reply_spi_transfer(channel: Channel, outcome: &TransferOutcome) {
    let reads: alloc::vec::Vec<_> = outcome
        .reads
        .iter()
        .map(|read: &ReadChunk| i2c_spi_fidl::SpiReadData {
            bytes: read.bytes.as_slice(),
        })
        .collect();
    let completion = i2c_spi_fidl::SpiTransferCompletion {
        status: status(outcome.status),
        reads: i2c_spi_fidl::WireVector::from_slice(&reads),
        operations_completed: outcome.operations_completed,
    };
    reply(
        channel,
        &i2c_spi_fidl::SpiDeviceTransferResponse { completion },
    );
}
