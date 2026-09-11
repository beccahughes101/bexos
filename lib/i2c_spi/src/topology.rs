use crate::types::{BusKind, I2cAddress, SpiMode, Status};
use alloc::{collections::BTreeSet, string::String, vec::Vec};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceProperty {
    pub key: String,
    pub value: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct I2cPeripheralConfig {
    pub address: I2cAddress,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpiPeripheralConfig {
    pub chip_select: u32,
    pub min_speed_hz: u32,
    pub max_speed_hz: u32,
    pub mode_mask: u8,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PeripheralConfig {
    I2c(I2cPeripheralConfig),
    Spi(SpiPeripheralConfig),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Peripheral {
    pub node_id: u64,
    pub name: String,
    pub properties: Vec<DeviceProperty>,
    pub config: PeripheralConfig,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ControllerConfig {
    pub node_id: u64,
    pub name: String,
    pub bus: BusKind,
    pub properties: Vec<DeviceProperty>,
    pub peripherals: Vec<Peripheral>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Topology {
    pub controllers: Vec<ControllerConfig>,
}

impl Topology {
    pub fn validate(&self) -> Result<(), Status> {
        let mut nodes = BTreeSet::new();
        for controller in &self.controllers {
            if controller.node_id == 0 || !nodes.insert(controller.node_id) {
                return Err(Status::InvalidArgs);
            }
            validate_properties(&controller.properties)?;
            let mut i2c_addresses = BTreeSet::new();
            let mut spi_chip_selects = BTreeSet::new();
            for peripheral in &controller.peripherals {
                if peripheral.node_id == 0 || !nodes.insert(peripheral.node_id) {
                    return Err(Status::InvalidArgs);
                }
                validate_properties(&peripheral.properties)?;
                match (&controller.bus, &peripheral.config) {
                    (BusKind::I2c, PeripheralConfig::I2c(i2c)) => {
                        if !i2c_addresses.insert((i2c.address.raw, i2c.address.kind as u8)) {
                            return Err(Status::AlreadyExists);
                        }
                    }
                    (BusKind::Spi, PeripheralConfig::Spi(spi)) => {
                        if spi.min_speed_hz == 0
                            || spi.max_speed_hz < spi.min_speed_hz
                            || spi.mode_mask == 0
                            || !spi_chip_selects.insert(spi.chip_select)
                        {
                            return Err(Status::InvalidArgs);
                        }
                    }
                    _ => return Err(Status::InvalidArgs),
                }
            }
        }
        Ok(())
    }

    pub fn controller(&self, node_id: u64) -> Option<&ControllerConfig> {
        self.controllers
            .iter()
            .find(|controller| controller.node_id == node_id)
    }

    pub fn peripheral(&self, node_id: u64) -> Option<(&ControllerConfig, &Peripheral)> {
        for controller in &self.controllers {
            if let Some(peripheral) = controller
                .peripherals
                .iter()
                .find(|peripheral| peripheral.node_id == node_id)
            {
                return Some((controller, peripheral));
            }
        }
        None
    }

    pub fn peripherals_for_bus(
        &self,
        bus: BusKind,
    ) -> impl Iterator<Item = (&ControllerConfig, &Peripheral)> {
        self.controllers
            .iter()
            .filter(move |controller| controller.bus == bus)
            .flat_map(|controller| {
                controller
                    .peripherals
                    .iter()
                    .map(move |peripheral| (controller, peripheral))
            })
    }
}

impl SpiPeripheralConfig {
    pub fn supports(&self, mode: SpiMode, speed_hz: u32) -> bool {
        speed_hz >= self.min_speed_hz
            && speed_hz <= self.max_speed_hz
            && self.mode_mask & mode.bit() != 0
    }
}

fn validate_properties(properties: &[DeviceProperty]) -> Result<(), Status> {
    let mut keys = BTreeSet::new();
    for property in properties {
        if property.key.is_empty() || !keys.insert(property.key.as_str()) {
            return Err(Status::InvalidArgs);
        }
    }
    Ok(())
}
