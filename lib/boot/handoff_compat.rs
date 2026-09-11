//! Normalize extensions before interpreting handoffs from saved firmware.
use crate::{BOOT_HANDOFF_VERSION, BootFramebuffer, BootHandoff};

impl BootHandoff {
    pub fn normalize_legacy_extensions(&mut self) {
        if self.version < 4 {
            self.secure_monitor_call_header = 0;
            self.secure_monitor_features = 0;
            self.framebuffer = BootFramebuffer::NONE;
        } else if self.version == 4 {
            // The graphics and secure-monitor branches independently used v4.
            // A framebuffer starts with a page-aligned address; the monitor ABI
            // header ends in its nonzero architecture ID and cannot match it.
            let graphics = BootFramebuffer::from_words([
                self.secure_monitor_call_header,
                self.secure_monitor_features,
                self.framebuffer.address,
                self.framebuffer.length,
                self.framebuffer.width,
                self.framebuffer.height,
            ]);
            self.framebuffer = graphics.unwrap_or(BootFramebuffer::NONE);
            if graphics.is_some() {
                self.secure_monitor_call_header = 0;
                self.secure_monitor_features = 0;
                self.version = BOOT_HANDOFF_VERSION;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn framebuffer() -> BootFramebuffer {
        BootFramebuffer {
            address: 0x9000_0000,
            length: 4096,
            width: 16,
            height: 16,
            stride: 64,
            format: 1,
        }
    }

    #[test]
    fn legacy_prefix_does_not_interpret_trailing_memory() {
        let mut h = BootHandoff::qemu_default(4096, 4);
        h.secure_monitor_call_header = u64::MAX;
        h.secure_monitor_features = u64::MAX;
        h.framebuffer = framebuffer();
        h.normalize_legacy_extensions();
        assert_eq!(h.secure_monitor_call_header, 0);
        assert_eq!(h.secure_monitor_features, 0);
        assert_eq!(h.framebuffer, BootFramebuffer::NONE);
    }

    #[test]
    fn v4_graphics_moves_after_monitor_prefix() {
        let f = framebuffer();
        let mut h = BootHandoff::qemu_default(4096, 4);
        h.version = 4;
        h.secure_monitor_call_header = f.address;
        h.secure_monitor_features = f.length;
        h.framebuffer = BootFramebuffer {
            address: f.width,
            length: f.height,
            width: f.stride,
            height: f.format,
            stride: u64::MAX,
            format: u64::MAX,
        };
        h.normalize_legacy_extensions();
        assert_eq!(h.version, BOOT_HANDOFF_VERSION);
        assert_eq!(h.framebuffer, f);
        assert_eq!(h.secure_monitor_call_header, 0);
        assert_eq!(&h.words()[18..24], &f.words());
        assert_eq!(core::mem::size_of::<BootHandoff>(), BootHandoff::WORDS * 8);
    }

    #[test]
    fn v4_monitor_preserves_abi_and_ignores_trailing_memory() {
        let mut h = BootHandoff::qemu_default(4096, 4);
        h.version = 4;
        h.secure_monitor_call_header = 0x4245_584d_0001_0002;
        h.secure_monitor_features = 1;
        h.framebuffer = framebuffer();
        h.normalize_legacy_extensions();
        assert_eq!(&h.words()[16..18], &[0x4245_584d_0001_0002, 1]);
        assert_eq!(h.framebuffer, BootFramebuffer::NONE);
    }

    #[test]
    fn v5_preserves_both_extensions() {
        let mut h = BootHandoff::qemu_default(4096, 4);
        h.version = BOOT_HANDOFF_VERSION;
        h.secure_monitor_call_header = 0x4245_584d_0001_0002;
        h.secure_monitor_features = 1;
        h.framebuffer = framebuffer();
        let words = h.words();
        h.normalize_legacy_extensions();
        assert_eq!(h.words(), words);
    }
}
