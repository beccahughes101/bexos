//! Build boot state from the authenticated snapshot, independent of guest
//! build outputs. The monitor fixes the architecture, RAM map and CPU count.
pub fn normal(bootfs_length: usize) -> [u8; bexos_boot::BootHandoff::WORDS * 8] {
    use bexos_boot::{BOOT_HANDOFF_VERSION, BootHandoff};
    let mut handoff = BootHandoff::qemu_x86_64(bootfs_length as u64, 4);
    if cfg!(feature = "secure_product") {
        handoff.version = BOOT_HANDOFF_VERSION;
        handoff.secure_monitor_call_header = bexos_secure_monitor_abi::HEADER;
        handoff.secure_monitor_features = 1;
    }
    let mut bytes = [0; BootHandoff::WORDS * 8];
    for (word, output) in handoff.words().iter().zip(bytes.chunks_exact_mut(8)) {
        output.copy_from_slice(&word.to_le_bytes());
    }
    bytes
}
