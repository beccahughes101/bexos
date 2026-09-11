//! Bounded root-port bus numbering and retained downstream MMIO allocation.
//! Configuration happens only at cold boot; polling and replacement never
//! reprogram a bridge window or probe BAR sizes on an existing endpoint.
use super::*;
pub const MAX_BUSES: u8 = 16;
pub const ECAM_SIZE: u64 = MAX_BUSES as u64 * 0x10_0000;
const WINDOW: u64 = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RootPort {
    pub address: PciAddress,
    pub secondary: u8,
    pub base: u64,
    pub limit: u64,
    pub cursor: u64,
}
impl RootPort {
    pub fn validate(&self, root: RootBusConfig) -> Result<(), PciError> {
        if self.address.bus != 0
            || self.address.device >= 32
            || self.address.function >= 8
            || self.secondary == 0
            || self.secondary >= MAX_BUSES
            || self.base < root.mmio_base
            || self.limit > root.mmio_limit
            || self.limit.checked_sub(self.base) != Some(WINDOW)
            || self.base % WINDOW != 0
            || !(self.base..=self.limit).contains(&self.cursor)
        {
            return Err(PciError::ConfigAccess);
        }
        Ok(())
    }
}
impl<C: ConfigSpace> PciRootBus<C> {
    fn power_on_slot(&mut self, address: PciAddress) -> Result<(), PciError> {
        let mut capability = self.config.read8(address, 0x34)? as u16;
        for _ in 0..48 {
            if capability == 0 {
                return Ok(());
            }
            if !(0x40..=0xfc).contains(&capability) || capability & 3 != 0 {
                return Err(PciError::ConfigAccess);
            }
            if self.config.read8(address, capability)? == 0x10 {
                if capability > 0xe4 {
                    return Err(PciError::ConfigAccess);
                }
                let flags = self.config.read16(address, capability + 2)?;
                let slot = self.config.read32(address, capability + 0x14)?;
                if flags & 0x100 != 0 && slot & 2 != 0 {
                    // Empty hotplug slots reset powered off. Clear PCC before
                    // scanning; setting bus numbers alone leaves future devices
                    // inaccessible. Never echo the write-one interlock toggle.
                    let control = self.config.read16(address, capability + 0x18)?;
                    let mut enabled = control & !(0x400 | 0x800);
                    if slot & 0x10 != 0 {
                        enabled = (enabled & !0x300) | 0x100;
                    }
                    self.config.write16(address, capability + 0x18, enabled)?;
                }
                return Ok(());
            }
            capability = self.config.read8(address, capability + 1)? as u16;
        }
        Err(PciError::ConfigAccess)
    }
    /// Reserve a window for each direct root port, including empty slots. This
    /// initial implementation supports direct downstream buses; switches and
    /// nested bridges require a separate hierarchical resource allocator.
    pub fn configure_root_ports(&mut self) -> Result<Vec<RootPort>, PciError> {
        let mut ports = Vec::new();
        for device in self.discover_bus0()? {
            if device.class != 6 || device.subclass != 4 {
                continue;
            }
            let address = device.address;
            if self.config.read8(address, REG_HEADER_TYPE)? & 0x7f != 1 {
                return Err(PciError::ConfigAccess);
            }
            let secondary = u8::try_from(ports.len() + 1).map_err(|_| PciError::BarTooLarge)?;
            if secondary >= MAX_BUSES {
                return Err(PciError::BarTooLarge);
            }
            let base = align_resource_base(self.next_mmio_base, WINDOW);
            let limit = base.checked_add(WINDOW).ok_or(PciError::BarTooLarge)?;
            if limit > self.bus.mmio_limit {
                return Err(PciError::BarTooLarge);
            }
            let command = self.config.read16(address, REG_COMMAND)?;
            self.config.write16(address, REG_COMMAND, command & !7)?;
            self.config.write32(
                address,
                0x18,
                (secondary as u32) << 8 | (secondary as u32) << 16,
            )?;
            // One non-prefetchable window forwards both kinds of MMIO. Disable
            // the separate prefetchable and I/O windows rather than alias them.
            self.config.write16(address, 0x1c, 0x00f0)?;
            self.config.write32(address, 0x24, 0x0000_fff0)?;
            self.config.write32(address, 0x28, 0)?;
            self.config.write32(address, 0x2c, 0)?;
            self.config.write32(
                address,
                0x20,
                ((base >> 16) as u32 & 0xfff0) | ((limit - 1) as u32 & 0xfff0_0000),
            )?;
            self.config
                .write16(address, REG_COMMAND, (command & !1) | 6)?;
            self.power_on_slot(address)?;
            self.next_mmio_base = limit;
            ports.push(RootPort {
                address,
                secondary,
                base,
                limit,
                cursor: base,
            });
        }
        Ok(ports)
    }

    pub fn initialize_downstream(
        &mut self,
        port: &mut RootPort,
        device: PciDevice,
    ) -> Result<DeviceNodeInfo, PciError> {
        port.validate(self.bus)?;
        if device.address.bus != port.secondary || (device.class == 6 && device.subclass == 4) {
            return Err(PciError::ConfigAccess);
        }
        let root = self.bus;
        let cursor = self.next_mmio_base;
        self.bus = RootBusConfig {
            mmio_base: port.base,
            mmio_limit: port.limit,
        };
        self.next_mmio_base = port.cursor;
        let result = self.initialize_device(device);
        if result.is_ok() {
            port.cursor = self.next_mmio_base;
        }
        self.bus = root;
        self.next_mmio_base = cursor;
        result
    }
}
