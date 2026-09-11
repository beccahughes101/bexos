//! Replacement-owned preparation, called in bounded slices on CPU0.
use crate::arch::ArchAPI;
use crate::{mmu::PhysicalBackend, state::Global};
use bexos_kernel_core::{
    runtime::Runtime,
    transplant::{checksum, codec::Reader},
};
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

pub const API_MAGIC: u64 = u64::from_le_bytes(*b"BEXPRE01");
pub const API_VERSION: u64 = 2;
// Runtime record kind 6 is SOCKET. Allocator records need a distinct tag.
pub const FRAME_RECORD: u64 = 0xff << 56;
pub const RECEIPT_MAGIC: u64 = u64::from_le_bytes(*b"BEXLIVE1");
#[repr(C)]
struct Api {
    magic: u64,
    version: u64,
    entry: extern "C" fn(u64, u64, u64) -> i64,
    stack_top: *const u8,
    architecture: u64,
}
unsafe impl Sync for Api {}
unsafe extern "C" {
    static __boot_stacks_top: u8;
}
#[cfg(bexos_update_kernel)]
#[used]
#[unsafe(link_section = ".transplant_api")]
static API: Api = Api {
    magic: API_MAGIC,
    version: API_VERSION,
    entry: prepare_main,
    stack_top: core::ptr::addr_of!(__boot_stacks_top),
    architecture: bexos_boot::ARCHITECTURE_ID,
};

struct Prepared {
    runtime: Runtime<PhysicalBackend>,
    generation: u64,
    sequence: u64,
    digest: u64,
}
static PREPARED: Global<Option<Prepared>> = Global::new(None);
#[unsafe(no_mangle)]
pub static PREPARE_RETURN_SP: AtomicU64 = AtomicU64::new(0);
#[unsafe(no_mangle)]
pub static PREPARE_FAULT_ESR: AtomicU64 = AtomicU64::new(0);
#[unsafe(no_mangle)]
pub static PREPARE_FAULT_ELR: AtomicU64 = AtomicU64::new(0);
#[unsafe(no_mangle)]
pub static PREPARE_FAULT_FAR: AtomicU64 = AtomicU64::new(0);
static DEADLINE: AtomicU64 = AtomicU64::new(0);
static IN_CALLBACK: AtomicBool = AtomicBool::new(false);
pub fn in_callback() -> bool {
    IN_CALLBACK.load(Ordering::Acquire)
}

pub fn active() -> bool {
    PREPARE_RETURN_SP.load(Ordering::Acquire) != 0
}
pub fn recover_timeout(frame: *mut bexos_kernel_core::runtime::Context) {
    if active() && crate::migration::now_ms() >= DEADLINE.load(Ordering::Acquire) {
        unsafe extern "C" {
            fn kernel_prepare_failure();
        }
        let frame = unsafe { &mut *frame };
        crate::arch::CurrentArch::prepare_failure(
            frame,
            kernel_prepare_failure as *const () as u64,
        );
    }
}

/// Return ABI: zero success, negative rejection/fault/deadline. Candidate code
/// gets no old-heap references; input is canonical bytes in reserved RAM.
pub fn call(op: u64, pointer: u64, len: u64) -> Result<(), ()> {
    let api = unsafe { &*(bexos_boot::UPDATE_BASE as *const Api) };
    if api.magic != API_MAGIC
        || api.version != API_VERSION
        || api.architecture != bexos_boot::ARCHITECTURE_ID
    {
        return Err(());
    }
    // A single record adoption is bounded independently from the 30-second
    // preparation deadline. Loaded QEMU guests can legitimately need more than
    // one 10 ms timer interval to rebuild a large process/channel record.
    DEADLINE.store(crate::migration::now_ms() + 100, Ordering::Release);
    PREPARE_FAULT_ESR.store(0, Ordering::Release);
    PREPARE_FAULT_ELR.store(0, Ordering::Release);
    PREPARE_FAULT_FAR.store(0, Ordering::Release);
    let timer_deadline = crate::arch::CurrentArch::timer_deadline();
    crate::arch::CurrentArch::set_timer_interval(crate::arch::CurrentArch::timer_frequency() / 10);
    unsafe extern "C" {
        fn kernel_prepare_call(entry: u64, stack: u64, op: u64, pointer: u64, len: u64) -> i64;
    }
    let result = unsafe {
        kernel_prepare_call(
            api.entry as *const () as u64,
            api.stack_top as u64,
            op,
            pointer,
            len,
        )
    };
    crate::arch::CurrentArch::restore_timer_deadline(timer_deadline);
    if result != 0 || crate::migration::now_ms() >= DEADLINE.load(Ordering::Acquire) {
        crate::log_line(&alloc::format!(
            "heart-transplant: candidate preparation callback rejected op={op} len={len} result={result} esr={:#x} elr={:#x} far={:#x}",
            PREPARE_FAULT_ESR.load(Ordering::Acquire),
            PREPARE_FAULT_ELR.load(Ordering::Acquire),
            PREPARE_FAULT_FAR.load(Ordering::Acquire),
        ));
        Err(())
    } else {
        Ok(())
    }
}

extern "C" fn prepare_main(op: u64, pointer: u64, len: u64) -> i64 {
    IN_CALLBACK.store(true, Ordering::Release);
    let result: Result<(), i64> = match op {
        0 => {
            crate::memory::init_heap();
            PREPARED.with(|slot| {
                *slot = Some(Prepared {
                    runtime: Runtime::new(PhysicalBackend {
                        frames: crate::memory::Frames::empty(),
                    }),
                    generation: pointer,
                    sequence: 0,
                    digest: 0,
                })
            });
            Ok(())
        }
        1 => PREPARED.with(|slot| {
            let p = slot.as_mut().ok_or(-2i64)?;
            if pointer != bexos_boot::UPDATE_SNAPSHOT + 4096 || !(24..=32768).contains(&len) {
                return Err(-3);
            }
            let bytes = unsafe { core::slice::from_raw_parts(pointer as *const u8, len as usize) };
            let (bytes, tail) = bytes.split_at(bytes.len() - 8);
            let expected = u64::from_le_bytes(tail.try_into().map_err(|_| -3i64)?);
            if checksum(bytes) as u64 != expected {
                return Err(-4);
            }
            let sequence = u64::from_le_bytes(bytes[..8].try_into().map_err(|_| -3i64)?);
            if sequence != p.sequence + 1 {
                return Err(-5);
            }
            let record = &bytes[8..];
            let key = u64::from_le_bytes(record[..8].try_into().map_err(|_| -3i64)?);
            if key >> 56 == FRAME_RECORD >> 56 {
                let mut r = Reader::new(record);
                let index = (r.word().map_err(|_| -6i64)? & 0x00ff_ffff_ffff_ffff) as usize;
                p.runtime
                    .backend
                    .frames
                    .adopt_word(index, r.word().map_err(|_| -6i64)?)
                    .map_err(|_| -6i64)?;
                if !r.finished() {
                    return Err(-7);
                }
            } else {
                p.runtime.adopt_record(record).map_err(|_| -8i64)?;
            }
            p.sequence = sequence;
            p.digest = p.digest.rotate_left(7) ^ expected;
            Ok(())
        }),
        2 => PREPARED.with(|slot| {
            slot.as_ref()
                .ok_or(-2i64)?
                .runtime
                .validate_live_snapshot()
                .map_err(|_| -9i64)
        }),
        _ => Err(-10),
    };
    IN_CALLBACK.store(false, Ordering::Release);
    result.err().unwrap_or(0)
}

pub fn take(generation: u64, sequence: u64, digest: u64) -> Option<Runtime<PhysicalBackend>> {
    PREPARED.with(|slot| {
        let p = slot.as_ref()?;
        if p.generation != generation || p.sequence != sequence || p.digest != digest {
            return None;
        }
        Some(slot.take()?.runtime)
    })
}
