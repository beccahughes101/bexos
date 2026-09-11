use crate::{
    arbitration::{
        ActiveTransfer, BusController, ClientEndpoint, Completion, LockLease, QueuedTransfer,
    },
    backend::{DeterministicBackend, status_to_word, word_to_status},
    topology::*,
    types::*,
};
use alloc::{collections::VecDeque, string::String, vec::Vec};
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};

pub const CONTROLLER_RECORD_VERSION: u64 = 1;

pub fn encode_controller(w: &mut Encoder, controller: &BusController) {
    w.word(CONTROLLER_RECORD_VERSION);
    encode_controller_config(w, &controller.config);
    w.word(controller.clients.len() as u64);
    for client in &controller.clients {
        encode_client(w, client);
    }
    w.word(controller.queue.len() as u64);
    for queued in &controller.queue {
        encode_queued(w, queued);
    }
    match &controller.active {
        Some(active) => {
            w.word(1);
            encode_active(w, active);
        }
        None => w.word(0),
    }
    w.word(controller.completions.len() as u64);
    for completion in &controller.completions {
        encode_completion(w, completion);
    }
    match controller.current_spi_config {
        Some((node, mode, speed)) => {
            w.word(1);
            w.word(node);
            w.word(mode as u64);
            w.word(speed as u64);
        }
        None => w.word(0),
    }
}

pub fn decode_controller(r: &mut Decoder<'_>) -> Result<BusController, Error> {
    if r.word()? != CONTROLLER_RECORD_VERSION {
        return Err(Error::UnsupportedVersion);
    }
    let config = decode_controller_config(r)?;
    let mut controller = BusController::new(config);
    for _ in 0..r.count(1024)? {
        controller.clients.push(decode_client(r)?);
    }
    let mut queue = VecDeque::new();
    for _ in 0..r.count(MAX_PENDING_BUNDLES_PER_CONTROLLER)? {
        queue.push_back(decode_queued(r)?);
    }
    controller.queue = queue;
    controller.active = if r.flag()? {
        Some(decode_active(r)?)
    } else {
        None
    };
    let mut completions = VecDeque::new();
    for _ in 0..r.count(1024)? {
        completions.push_back(decode_completion(r)?);
    }
    controller.completions = completions;
    controller.current_spi_config = if r.flag()? {
        let node = r.word()?;
        let mode = decode_spi_mode(r.word()?)?;
        let speed = u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
        Some((node, mode, speed))
    } else {
        None
    };
    controller.config.validate_as_single()?;
    Ok(controller)
}

pub fn encode_backend(w: &mut Encoder, backend: &DeterministicBackend) {
    backend.encode(w);
}

pub fn decode_backend(r: &mut Decoder<'_>) -> Result<DeterministicBackend, Error> {
    DeterministicBackend::decode(r)
}

fn encode_controller_config(w: &mut Encoder, config: &ControllerConfig) {
    w.word(config.node_id);
    w.text(&config.name);
    w.word(match config.bus {
        BusKind::I2c => 1,
        BusKind::Spi => 2,
    });
    encode_properties(w, &config.properties);
    w.word(config.peripherals.len() as u64);
    for peripheral in &config.peripherals {
        w.word(peripheral.node_id);
        w.text(&peripheral.name);
        encode_properties(w, &peripheral.properties);
        match &peripheral.config {
            PeripheralConfig::I2c(i2c) => {
                w.word(1);
                w.word(i2c.address.raw as u64);
                w.word((i2c.address.kind == I2cAddressKind::TenBit) as u64);
            }
            PeripheralConfig::Spi(spi) => {
                w.word(2);
                w.word(spi.chip_select as u64);
                w.word(spi.min_speed_hz as u64);
                w.word(spi.max_speed_hz as u64);
                w.word(spi.mode_mask as u64);
            }
        }
    }
}

fn decode_controller_config(r: &mut Decoder<'_>) -> Result<ControllerConfig, Error> {
    let node_id = r.word()?;
    let name = String::from(r.text(128)?);
    let bus = match r.word()? {
        1 => BusKind::I2c,
        2 => BusKind::Spi,
        _ => return Err(Error::InvalidData),
    };
    let properties = decode_properties(r)?;
    let mut peripherals = Vec::new();
    for _ in 0..r.count(256)? {
        let node_id = r.word()?;
        let name = String::from(r.text(128)?);
        let properties = decode_properties(r)?;
        let config = match r.word()? {
            1 => PeripheralConfig::I2c(I2cPeripheralConfig {
                address: I2cAddress::new(
                    u16::try_from(r.word()?).map_err(|_| Error::InvalidData)?,
                    r.flag()?,
                )
                .map_err(|_| Error::InvalidData)?,
            }),
            2 => PeripheralConfig::Spi(SpiPeripheralConfig {
                chip_select: u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?,
                min_speed_hz: u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?,
                max_speed_hz: u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?,
                mode_mask: u8::try_from(r.word()?).map_err(|_| Error::InvalidData)?,
            }),
            _ => return Err(Error::InvalidData),
        };
        peripherals.push(Peripheral {
            node_id,
            name,
            properties,
            config,
        });
    }
    Ok(ControllerConfig {
        node_id,
        name,
        bus,
        properties,
        peripherals,
    })
}

fn encode_properties(w: &mut Encoder, properties: &[DeviceProperty]) {
    w.word(properties.len() as u64);
    for property in properties {
        w.text(&property.key);
        w.word(property.value as u64);
    }
}

fn decode_properties(r: &mut Decoder<'_>) -> Result<Vec<DeviceProperty>, Error> {
    let mut out = Vec::new();
    for _ in 0..r.count(64)? {
        out.push(DeviceProperty {
            key: String::from(r.text(128)?),
            value: u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?,
        });
    }
    Ok(out)
}

fn encode_client(w: &mut Encoder, client: &ClientEndpoint) {
    w.word(client.channel);
    w.word(client.peripheral_node_id);
    w.word(client.allowed_methods.len() as u64);
    for ordinal in &client.allowed_methods {
        w.word(*ordinal);
    }
    match &client.lock {
        Some(lock) => {
            w.word(1);
            w.word(lock.owner_channel);
            w.word(lock.expires_at_ms);
            w.word(lock.expired as u64);
        }
        None => w.word(0),
    }
}

fn decode_client(r: &mut Decoder<'_>) -> Result<ClientEndpoint, Error> {
    let channel = r.word()?;
    let peripheral_node_id = r.word()?;
    let mut allowed_methods = Vec::new();
    for _ in 0..r.count(16)? {
        allowed_methods.push(r.word()?);
    }
    let lock = if r.flag()? {
        Some(LockLease {
            owner_channel: r.word()?,
            expires_at_ms: r.word()?,
            expired: r.flag()?,
        })
    } else {
        None
    };
    Ok(ClientEndpoint {
        channel,
        peripheral_node_id,
        allowed_methods,
        lock,
    })
}

fn encode_queued(w: &mut Encoder, queued: &QueuedTransfer) {
    w.word(queued.request_id);
    w.word(queued.client_channel);
    w.word(queued.peripheral_node_id);
    encode_request(w, &queued.request);
    w.word(queued.deadline_ms);
    match queued.lock_owner {
        Some(owner) => {
            w.word(1);
            w.word(owner);
        }
        None => {
            w.word(0);
            w.word(0);
        }
    }
}

fn decode_queued(r: &mut Decoder<'_>) -> Result<QueuedTransfer, Error> {
    Ok(QueuedTransfer {
        request_id: r.word()?,
        client_channel: r.word()?,
        peripheral_node_id: r.word()?,
        request: decode_request(r)?,
        deadline_ms: r.word()?,
        lock_owner: if r.flag()? {
            Some(r.word()?)
        } else {
            let _ = r.word()?;
            None
        },
    })
}

fn encode_active(w: &mut Encoder, active: &ActiveTransfer) {
    encode_queued(w, &active.queued);
    w.word(active.backend_token);
    w.word(active.ready_at_ms);
}

fn decode_active(r: &mut Decoder<'_>) -> Result<ActiveTransfer, Error> {
    Ok(ActiveTransfer {
        queued: decode_queued(r)?,
        backend_token: r.word()?,
        ready_at_ms: r.word()?,
    })
}

fn encode_completion(w: &mut Encoder, completion: &Completion) {
    w.word(completion.request_id);
    w.word(completion.client_channel);
    encode_outcome(w, &completion.outcome);
}

fn decode_completion(r: &mut Decoder<'_>) -> Result<Completion, Error> {
    Ok(Completion {
        request_id: r.word()?,
        client_channel: r.word()?,
        outcome: decode_outcome(r)?,
    })
}

pub fn encode_request(w: &mut Encoder, request: &Request) {
    match request {
        Request::I2c(bundle) => {
            w.word(1);
            w.word(bundle.operations.len() as u64);
            for op in &bundle.operations {
                match op {
                    I2cOp::Write(bytes) => {
                        w.word(1);
                        w.bytes(bytes);
                        w.word(0);
                    }
                    I2cOp::Read(len) => {
                        w.word(2);
                        w.bytes(&[]);
                        w.word(*len as u64);
                    }
                }
            }
        }
        Request::Spi(bundle) => {
            w.word(2);
            w.word(bundle.speed_hz as u64);
            w.word(bundle.mode as u64);
            w.word(bundle.operations.len() as u64);
            for op in &bundle.operations {
                match op {
                    SpiOp::Write(bytes) => {
                        w.word(1);
                        w.bytes(bytes);
                        w.word(0);
                    }
                    SpiOp::Read(len) => {
                        w.word(2);
                        w.bytes(&[]);
                        w.word(*len as u64);
                    }
                    SpiOp::FullDuplex(bytes) => {
                        w.word(3);
                        w.bytes(bytes);
                        w.word(bytes.len() as u64);
                    }
                }
            }
        }
    }
}

pub fn decode_request(r: &mut Decoder<'_>) -> Result<Request, Error> {
    Ok(match r.word()? {
        1 => {
            let mut operations = Vec::new();
            for _ in 0..r.count(MAX_OPERATIONS_PER_BUNDLE)? {
                operations.push(match r.word()? {
                    1 => {
                        let bytes = r.bytes(MAX_TRANSFER_BYTES_PER_BUNDLE)?.to_vec();
                        let _ = r.word()?;
                        I2cOp::Write(bytes)
                    }
                    2 => {
                        let _ = r.bytes(0)?;
                        I2cOp::Read(usize::try_from(r.word()?).map_err(|_| Error::InvalidData)?)
                    }
                    _ => return Err(Error::InvalidData),
                });
            }
            Request::I2c(I2cBundle { operations })
        }
        2 => {
            let speed_hz = u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
            let mode = decode_spi_mode(r.word()?)?;
            let mut operations = Vec::new();
            for _ in 0..r.count(MAX_OPERATIONS_PER_BUNDLE)? {
                operations.push(match r.word()? {
                    1 => {
                        let bytes = r.bytes(MAX_TRANSFER_BYTES_PER_BUNDLE)?.to_vec();
                        let _ = r.word()?;
                        SpiOp::Write(bytes)
                    }
                    2 => {
                        let _ = r.bytes(0)?;
                        SpiOp::Read(usize::try_from(r.word()?).map_err(|_| Error::InvalidData)?)
                    }
                    3 => {
                        let bytes = r.bytes(MAX_TRANSFER_BYTES_PER_BUNDLE)?.to_vec();
                        let _ = r.word()?;
                        SpiOp::FullDuplex(bytes)
                    }
                    _ => return Err(Error::InvalidData),
                });
            }
            Request::Spi(SpiBundle {
                operations,
                speed_hz,
                mode,
            })
        }
        _ => return Err(Error::InvalidData),
    })
}

fn encode_outcome(w: &mut Encoder, outcome: &TransferOutcome) {
    w.word(status_to_word(outcome.status));
    w.word(outcome.operations_completed as u64);
    w.word(outcome.reads.len() as u64);
    for read in &outcome.reads {
        w.bytes(&read.bytes);
    }
}

fn decode_outcome(r: &mut Decoder<'_>) -> Result<TransferOutcome, Error> {
    let status = word_to_status(r.word()?).ok_or(Error::InvalidData)?;
    let operations_completed = u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
    let mut reads = Vec::new();
    for _ in 0..r.count(MAX_OPERATIONS_PER_BUNDLE)? {
        reads.push(ReadChunk {
            bytes: r.bytes(MAX_TRANSFER_BYTES_PER_BUNDLE)?.to_vec(),
        });
    }
    Ok(TransferOutcome {
        status,
        reads,
        operations_completed,
    })
}

fn decode_spi_mode(raw: u64) -> Result<SpiMode, Error> {
    match raw {
        0 => Ok(SpiMode::Mode0),
        1 => Ok(SpiMode::Mode1),
        2 => Ok(SpiMode::Mode2),
        3 => Ok(SpiMode::Mode3),
        _ => Err(Error::InvalidData),
    }
}

trait ValidateSingleController {
    fn validate_as_single(&self) -> Result<(), Error>;
}

impl ValidateSingleController for ControllerConfig {
    fn validate_as_single(&self) -> Result<(), Error> {
        Topology {
            controllers: alloc::vec![self.clone()],
        }
        .validate()
        .map_err(|_| Error::InvalidData)
    }
}
