//! Domain-local IOAPIC routing. Physical CPU destinations and host interrupt
//! configuration never cross this interface.
#[path = "ioapic_state.rs"]
mod state;
#[derive(Clone)]
pub struct IoApic {
    selected: u8,
    low: [u32; 24],
    high: [u32; 24],
    cpus: u8,
}
impl IoApic {
    pub fn new(cpus: u8) -> Option<Self> {
        (cpus > 0).then_some(Self {
            selected: 0,
            low: [1 << 16; 24],
            high: [0; 24],
            cpus,
        })
    }
    pub fn read(&self, offset: u16) -> Option<u32> {
        if offset == 0 {
            return Some(u32::from(self.selected));
        }
        if offset != 0x10 {
            return None;
        }
        Some(match self.selected {
            0 => 0,
            1 => (23 << 16) | 0x11,
            2 => 0,
            0x10..=0x3f => {
                let index = usize::from((self.selected - 0x10) / 2);
                if self.selected & 1 == 0 {
                    self.low[index]
                } else {
                    self.high[index]
                }
            }
            _ => return None,
        })
    }
    pub fn write(&mut self, offset: u16, value: u32) -> bool {
        if offset == 0 && value <= 255 {
            self.selected = value as u8;
            return true;
        }
        if offset != 0x10 || !(0x10..=0x3f).contains(&self.selected) {
            return false;
        }
        let index = usize::from((self.selected - 0x10) / 2);
        if self.selected & 1 != 0 {
            if value & 0xffffff != 0 || value >> 24 >= u32::from(self.cpus) {
                return false;
            }
            self.high[index] = value;
        } else {
            // Fixed delivery, physical destination, edge-triggered input.
            if value & !(0xff | (1 << 13) | (1 << 16)) != 0
                || value & (1 << 16) == 0 && (value as u8) < 16
            {
                return false;
            }
            self.low[index] = value;
        }
        true
    }
    pub fn route(&self, gsi: u8) -> Option<(u8, u8)> {
        let low = *self.low.get(usize::from(gsi))?;
        if low & (1 << 16) != 0 {
            return None;
        }
        Some(((self.high[usize::from(gsi)] >> 24) as u8, low as u8))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn routes_only_to_domain_cpus_and_masks_without_leaking_other_instances() {
        let mut io = IoApic::new(4).unwrap();
        let other = IoApic::new(4).unwrap();
        assert!(io.write(0, 0x31));
        assert!(!io.write(0x10, 4 << 24));
        assert!(io.write(0x10, 3 << 24));
        assert!(io.write(0, 0x30));
        assert!(!io.write(0x10, 0x440));
        assert!(io.write(0x10, 80));
        assert_eq!(io.route(16), Some((3, 80)));
        assert_eq!(other.route(16), None);
        assert!(io.write(0x10, (1 << 16) | 80));
        assert_eq!(io.route(16), None);
        assert_eq!(io.route(255), None);
    }
}
