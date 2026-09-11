//! Modern virtio feature negotiation for devices behind the monitor's IOMMU.
//! Wider/overlapping writes cannot bypass ACCESS_PLATFORM or activate queues
//! while the device would interpret DMA addresses outside the platform domain.
#[derive(Clone, Copy, Default)]
pub struct Features {
    select: u32,
    platform: bool,
}
impl Features {
    pub(crate) fn snapshot(&self) -> [u8; 8] {
        let mut bytes = [0; 8];
        bytes[..4].copy_from_slice(&self.select.to_le_bytes());
        bytes[4] = u8::from(self.platform);
        bytes
    }
    pub(crate) fn restore(bytes: [u8; 8]) -> Option<Self> {
        let select = u32::from_le_bytes(bytes[..4].try_into().ok()?);
        (select <= 1 && bytes[4] <= 1 && bytes[5..] == [0; 3]).then_some(Self {
            select,
            platform: bytes[4] != 0,
        })
    }
    pub fn ready(&self) -> bool {
        self.platform
    }
    pub fn write(&mut self, offset: u64, bytes: u8, value: u64) -> bool {
        match (offset, bytes) {
            (0, 4) => value <= 1, // Device-feature word selection.
            (8, 4) if value <= 1 => {
                self.select = value as u32;
                true
            }
            (12, 4) if self.select == 0 => true,
            (12, 4) if value & 2 != 0 => {
                self.platform = true;
                true
            }
            (16 | 26, 2) => value == 0xffff, // Physical MSI-X vectors are never assigned.
            (20, 1) if value == 0 => {
                self.platform = false;
                self.select = 0;
                true
            }
            (20, 1) => value & !0x8f == 0 && (value & 12 == 0 || self.platform),
            (22, 2) => true, // Queue selection has no DMA side effect.
            (24 | 28, 2) | (32 | 40 | 48, 8) | (32 | 36 | 40 | 44 | 48 | 52, 4) => self.platform,
            _ => false,
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn feature_and_queue_writes_cannot_bypass_the_platform_mapping_path() {
        let mut features = Features::default();
        for (offset, bytes, value) in [
            (8, 8, 3),
            (12, 8, 3),
            (20, 2, 15),
            (28, 2, 1),
            (32, 8, 0x4000000),
            (20, 1, 15),
            (8, 4, 2),
        ] {
            assert!(!features.write(offset, bytes, value));
        }
        assert!(features.write(8, 4, 1));
        assert!(!features.write(12, 4, 1));
        assert!(!features.ready());
        assert!(features.write(12, 4, 3));
        assert!(features.write(20, 1, 15));
        assert!(features.ready());
        assert!(features.write(32, 8, 0x1000));
        assert!(features.write(28, 2, 1));
        assert!(!features.write(16, 2, 0));
        assert!(features.write(16, 2, 0xffff));
        assert!(!features.write(12, 4, 1));
        assert!(features.ready());
        assert!(features.write(20, 1, 0));
        assert!(!features.ready());
        assert!(!features.write(28, 2, 1));
    }
}
