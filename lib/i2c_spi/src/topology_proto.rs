use crate::{
    topology::*,
    types::{BusKind, I2cAddress, Status},
};
use alloc::{string::String, vec::Vec};

pub fn decode(bytes: &[u8]) -> Result<Topology, Status> {
    let mut p = Parser::new(bytes);
    let mut topology = Topology::default();
    while let Some(field) = p.field()? {
        match field.number {
            1 => topology.controllers.push(decode_controller(field.bytes?)?),
            _ => {}
        }
    }
    topology.validate()?;
    Ok(topology)
}

fn decode_controller(bytes: &[u8]) -> Result<ControllerConfig, Status> {
    let mut p = Parser::new(bytes);
    let mut node_id = 0;
    let mut name = String::new();
    let mut bus = None;
    let mut properties = Vec::new();
    let mut peripherals = Vec::new();
    while let Some(field) = p.field()? {
        match field.number {
            1 => node_id = field.varint?,
            2 => {
                name = String::from(
                    core::str::from_utf8(field.bytes?).map_err(|_| Status::InvalidArgs)?,
                )
            }
            3 => {
                bus = Some(match field.varint? {
                    1 => BusKind::I2c,
                    2 => BusKind::Spi,
                    _ => return Err(Status::InvalidArgs),
                });
            }
            4 => properties.push(decode_property(field.bytes?)?),
            5 => peripherals.push(decode_peripheral(field.bytes?)?),
            _ => {}
        }
    }
    Ok(ControllerConfig {
        node_id,
        name,
        bus: bus.ok_or(Status::InvalidArgs)?,
        properties,
        peripherals,
    })
}

fn decode_peripheral(bytes: &[u8]) -> Result<Peripheral, Status> {
    let mut p = Parser::new(bytes);
    let mut node_id = 0;
    let mut name = String::new();
    let mut properties = Vec::new();
    let mut config = None;
    while let Some(field) = p.field()? {
        match field.number {
            1 => node_id = field.varint?,
            2 => {
                name = String::from(
                    core::str::from_utf8(field.bytes?).map_err(|_| Status::InvalidArgs)?,
                )
            }
            3 => properties.push(decode_property(field.bytes?)?),
            4 => config = Some(PeripheralConfig::I2c(decode_i2c(field.bytes?)?)),
            5 => config = Some(PeripheralConfig::Spi(decode_spi(field.bytes?)?)),
            _ => {}
        }
    }
    Ok(Peripheral {
        node_id,
        name,
        properties,
        config: config.ok_or(Status::InvalidArgs)?,
    })
}

fn decode_i2c(bytes: &[u8]) -> Result<I2cPeripheralConfig, Status> {
    let mut p = Parser::new(bytes);
    let mut address = 0u16;
    let mut ten_bit = false;
    while let Some(field) = p.field()? {
        match field.number {
            1 => address = u16::try_from(field.varint?).map_err(|_| Status::InvalidArgs)?,
            2 => ten_bit = field.varint? != 0,
            _ => {}
        }
    }
    Ok(I2cPeripheralConfig {
        address: I2cAddress::new(address, ten_bit)?,
    })
}

fn decode_spi(bytes: &[u8]) -> Result<SpiPeripheralConfig, Status> {
    let mut p = Parser::new(bytes);
    let mut cfg = SpiPeripheralConfig {
        chip_select: 0,
        min_speed_hz: 0,
        max_speed_hz: 0,
        mode_mask: 0,
    };
    while let Some(field) = p.field()? {
        match field.number {
            1 => cfg.chip_select = u32::try_from(field.varint?).map_err(|_| Status::InvalidArgs)?,
            2 => {
                cfg.min_speed_hz = u32::try_from(field.varint?).map_err(|_| Status::InvalidArgs)?
            }
            3 => {
                cfg.max_speed_hz = u32::try_from(field.varint?).map_err(|_| Status::InvalidArgs)?
            }
            4 => cfg.mode_mask = u8::try_from(field.varint?).map_err(|_| Status::InvalidArgs)?,
            _ => {}
        }
    }
    Ok(cfg)
}

fn decode_property(bytes: &[u8]) -> Result<DeviceProperty, Status> {
    let mut p = Parser::new(bytes);
    let mut key = String::new();
    let mut value = 0;
    while let Some(field) = p.field()? {
        match field.number {
            1 => {
                key = String::from(
                    core::str::from_utf8(field.bytes?).map_err(|_| Status::InvalidArgs)?,
                )
            }
            2 => value = u32::try_from(field.varint?).map_err(|_| Status::InvalidArgs)?,
            _ => {}
        }
    }
    Ok(DeviceProperty { key, value })
}

struct Field<'a> {
    number: u32,
    varint: Result<u64, Status>,
    bytes: Result<&'a [u8], Status>,
}

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    fn field(&mut self) -> Result<Option<Field<'a>>, Status> {
        if self.pos == self.bytes.len() {
            return Ok(None);
        }
        let key = self.varint()?;
        let number = (key >> 3) as u32;
        let wire = key & 0x7;
        match wire {
            0 => {
                let value = self.varint()?;
                Ok(Some(Field {
                    number,
                    varint: Ok(value),
                    bytes: Err(Status::InvalidArgs),
                }))
            }
            2 => {
                let len = usize::try_from(self.varint()?).map_err(|_| Status::InvalidArgs)?;
                let end = self.pos.checked_add(len).ok_or(Status::InvalidArgs)?;
                let bytes = self.bytes.get(self.pos..end).ok_or(Status::InvalidArgs)?;
                self.pos = end;
                Ok(Some(Field {
                    number,
                    varint: Err(Status::InvalidArgs),
                    bytes: Ok(bytes),
                }))
            }
            _ => Err(Status::InvalidArgs),
        }
    }

    fn varint(&mut self) -> Result<u64, Status> {
        let mut out = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = *self.bytes.get(self.pos).ok_or(Status::InvalidArgs)?;
            self.pos += 1;
            out |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Ok(out);
            }
        }
        Err(Status::InvalidArgs)
    }
}
