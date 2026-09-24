//! Boot-only snapshots of external payloads. The EFI signature authenticates
//! the verifier and its root key; AVB authenticates these separately supplied
//! bytes. Domain execution cannot overlap loading or mutate these snapshots.
use core::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

const KERNEL_BYTES: usize = 64 * 1024 * 1024;
const BOOTFS_BYTES: usize = bexos_secure_monitor::boot_verify::BOOTFS_MAX_BYTES;
const METADATA_BYTES: usize = 64 * 1024;
static STATE: AtomicU8 = AtomicU8::new(0);
static KERNEL_LENGTH: AtomicUsize = AtomicUsize::new(0);
static BOOTFS_LENGTH: AtomicUsize = AtomicUsize::new(0);
static METADATA_LENGTH: AtomicUsize = AtomicUsize::new(0);
#[cfg_attr(
    feature = "resident_nucleus",
    unsafe(link_section = ".resident.payload")
)]
static mut KERNEL: [u8; KERNEL_BYTES] = [0; KERNEL_BYTES];
#[cfg_attr(
    feature = "resident_nucleus",
    unsafe(link_section = ".resident.payload")
)]
static mut BOOTFS: [u8; BOOTFS_BYTES] = [0; BOOTFS_BYTES];
#[cfg_attr(
    feature = "resident_nucleus",
    unsafe(link_section = ".resident.payload")
)]
static mut METADATA: [u8; METADATA_BYTES] = [0; METADATA_BYTES];

pub fn load() {
    assert!(
        STATE
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    );
    // These fixed-capacity buffers belong to the EFI-reserved monitor image.
    // The actual lengths are not compiled into firmware: authenticated AVB
    // descriptors must cover the complete immutable snapshots, with no suffix.
    let loaded = unsafe {
        use bexos_secure_monitor::fw_cfg::read_dma;
        [
            read_dma(b"opt/bexos/kernel", &mut *core::ptr::addr_of_mut!(KERNEL)),
            read_dma(b"opt/bexos/bootfs", &mut *core::ptr::addr_of_mut!(BOOTFS)),
            read_dma(b"opt/bexos/vbmeta", &mut *core::ptr::addr_of_mut!(METADATA)),
        ]
    };
    let [Some(kernel), Some(bootfs), Some(metadata)] = loaded else {
        crate::log("monitor-runtime: boot payload snapshot rejected\n");
        crate::halt();
    };
    KERNEL_LENGTH.store(kernel, Ordering::Relaxed);
    BOOTFS_LENGTH.store(bootfs, Ordering::Relaxed);
    METADATA_LENGTH.store(metadata, Ordering::Relaxed);
    STATE.store(2, Ordering::Release);
}

pub fn kernel() -> &'static [u8] {
    assert_eq!(STATE.load(Ordering::Acquire), 2);
    unsafe { &(&*core::ptr::addr_of!(KERNEL))[..KERNEL_LENGTH.load(Ordering::Relaxed)] }
}
pub fn bootfs() -> &'static [u8] {
    assert_eq!(STATE.load(Ordering::Acquire), 2);
    unsafe { &(&*core::ptr::addr_of!(BOOTFS))[..BOOTFS_LENGTH.load(Ordering::Relaxed)] }
}
pub fn metadata() -> &'static [u8] {
    assert_eq!(STATE.load(Ordering::Acquire), 2);
    unsafe { &(&*core::ptr::addr_of!(METADATA))[..METADATA_LENGTH.load(Ordering::Relaxed)] }
}
