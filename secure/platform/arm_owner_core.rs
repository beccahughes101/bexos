#![no_std]

//! Register protocol and immutable candidate preparation for the permanent
//! AArch64 S-EL2 owner. Execution and cutover stay in `arm_owner.S`; this core
//! owns bounds checking, AVB authentication, and ELF loading.

use bexos_secure_firmware::{Architecture, Component};
use bexos_secure_monitor_abi::{
    HEADER, PINNED_HANDLE_TAG, Request, SHARED_READ, Status, firmware as abi,
};
use core::ffi::c_void;
use core::panic::PanicInfo;
use core::sync::atomic::{AtomicBool, Ordering};

mod arm_persistent;

#[allow(dead_code)]
mod layout {
    include!(env!("BEXOS_ARM_OWNER_LAYOUT"));
}

const ROOT: &[u8] = include_bytes!(env!("BEXOS_ARM_OWNER_ROOT"));
const ELF_HEADER_BYTES: usize = 64;
const PROGRAM_HEADER_BYTES: usize = 56;
const PT_LOAD: u32 = 1;
const TRUSTY_RUNTIME_BYTES: usize = 14 * 1024 * 1024;
const CANDIDATE_MANIFEST_MAGIC: &[u8; 8] = b"BEXARM01";
const CANDIDATE_MANIFEST_BYTES: usize = 32;

static LOCK: AtomicBool = AtomicBool::new(false);
static mut REGISTERED_TOKEN: u64 = 0;
static mut REGISTERED_ADDRESS: u64 = 0;
static mut REGISTERED_LENGTH: u64 = 0;
static mut STAGED_LENGTH: u64 = 0;
static mut STAGED_GENERATION: u64 = 0;
static mut STAGED_COMPONENT: u64 = 0;
static mut STAGED_WRITTEN: u64 = 0;
static mut STAGED_SEALED: bool = false;
static mut ACTIVE_GENERATION: u64 = 1;
static mut ACTIVE_SLOT: u64 = 1;
static mut STAGED_SLOT: u64 = 0;
static mut STAGED_DIGEST: [u8; 32] = [0; 32];
static mut STAGED_IMAGE_LENGTH: u64 = 0;
static mut REPORT: [u64; 4] = [abi::IDLE, 0, 0, 1];
static mut REVISION: u64 = 0;
static mut TRANSPORT_GENERATION: u64 = 1;
static mut ACTIVATION_ACTION: u64 = 0;
static mut SOURCE_GENERATION: u64 = 1;
static mut SOURCE_SLOT: u64 = 1;
static mut CANDIDATE_READY: bool = false;
static mut BOOT_TRIAL: bool = false;

struct Guard;
impl Guard {
    fn acquire() -> Result<Self, Status> {
        LOCK.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .map(|_| Self)
            .map_err(|_| Status::Busy)
    }
}
impl Drop for Guard {
    fn drop(&mut self) {
        LOCK.store(false, Ordering::Release);
    }
}

#[panic_handler]
fn panic(_: &PanicInfo<'_>) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

fn word(bytes: &[u8], offset: usize) -> Option<u64> {
    Some(u64::from_le_bytes(
        bytes.get(offset..offset + 8)?.try_into().ok()?,
    ))
}

fn half(bytes: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        bytes.get(offset..offset + 2)?.try_into().ok()?,
    ))
}

fn double(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        bytes.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

fn candidate_manifest(image: &[u8]) -> Result<(u64, u64, u64), Status> {
    let mut found = None;
    for offset in 0..image.len().saturating_sub(CANDIDATE_MANIFEST_BYTES - 1) {
        if image.get(offset..offset + 8) != Some(CANDIDATE_MANIFEST_MAGIC) {
            continue;
        }
        if found.is_some() {
            return Err(Status::InvalidArgs);
        }
        found = Some((
            word(image, offset + 8).ok_or(Status::InvalidArgs)?,
            word(image, offset + 16).ok_or(Status::InvalidArgs)?,
            word(image, offset + 24).ok_or(Status::InvalidArgs)?,
        ));
    }
    found.ok_or(Status::InvalidArgs)
}

unsafe fn clear_stage() {
    unsafe {
        STAGED_LENGTH = 0;
        STAGED_GENERATION = 0;
        STAGED_COMPONENT = 0;
        STAGED_WRITTEN = 0;
        STAGED_SEALED = false;
        STAGED_SLOT = 0;
        STAGED_DIGEST = [0; 32];
        STAGED_IMAGE_LENGTH = 0;
        ACTIVATION_ACTION = 0;
    }
}

fn valid_owner(owner: u64) -> bool {
    unsafe { REGISTERED_TOKEN != 0 && owner == (REGISTERED_TOKEN | PINNED_HANDLE_TAG) }
}

unsafe fn load_candidate(image: &[u8], base: u64) -> Result<(), Status> {
    if image.len() < ELF_HEADER_BYTES
        || &image[..7] != b"\x7fELF\x02\x01\x01"
        || half(image, 16) != Some(3)
        || half(image, 18) != Some(183)
        || half(image, 54) != Some(PROGRAM_HEADER_BYTES as u16)
    {
        return Err(Status::InvalidArgs);
    }
    let headers = usize::from(half(image, 56).ok_or(Status::InvalidArgs)?);
    let table = usize::try_from(word(image, 32).ok_or(Status::InvalidArgs)?)
        .map_err(|_| Status::InvalidArgs)?;
    let table_end = headers
        .checked_mul(PROGRAM_HEADER_BYTES)
        .and_then(|n| table.checked_add(n))
        .filter(|end| *end <= image.len())
        .ok_or(Status::InvalidArgs)?;
    let bank = unsafe { core::slice::from_raw_parts_mut(base as *mut u8, TRUSTY_RUNTIME_BYTES) };
    bank.fill(0);
    let mut loaded = 0usize;
    for header in image[table..table_end].chunks_exact(PROGRAM_HEADER_BYTES) {
        if double(header, 0) != Some(PT_LOAD) {
            continue;
        }
        let source = usize::try_from(word(header, 8).ok_or(Status::InvalidArgs)?)
            .map_err(|_| Status::InvalidArgs)?;
        let destination = usize::try_from(word(header, 24).ok_or(Status::InvalidArgs)?)
            .map_err(|_| Status::InvalidArgs)?;
        let file_bytes = usize::try_from(word(header, 32).ok_or(Status::InvalidArgs)?)
            .map_err(|_| Status::InvalidArgs)?;
        let memory_bytes = usize::try_from(word(header, 40).ok_or(Status::InvalidArgs)?)
            .map_err(|_| Status::InvalidArgs)?;
        if file_bytes > memory_bytes
            || source
                .checked_add(file_bytes)
                .is_none_or(|end| end > image.len())
            || destination
                .checked_add(memory_bytes)
                .is_none_or(|end| end > bank.len())
        {
            bank.fill(0);
            return Err(Status::InvalidArgs);
        }
        bank[destination..destination + file_bytes]
            .copy_from_slice(&image[source..source + file_bytes]);
        loaded = loaded.checked_add(1).ok_or(Status::InvalidArgs)?;
    }
    if loaded == 0 || word(image, 24) != Some(0) {
        bank.fill(0);
        return Err(Status::InvalidArgs);
    }
    Ok(())
}

fn query(field: u64) -> Result<u64, Status> {
    unsafe {
        match field {
            abi::QUERY_CAPABILITIES => Ok(abi::CAP_TRUSTY_REBOOT | abi::CAP_TRUSTY_LIVE),
            abi::QUERY_MIGRATION_ABI => Ok(1),
            abi::QUERY_TRANSPORT_GENERATION => Ok(TRANSPORT_GENERATION),
            abi::QUERY_REVISION => Ok(REVISION),
            abi::QUERY_TRUSTY_GENERATION => Ok(ACTIVE_GENERATION),
            abi::QUERY_MONITOR_GENERATION => Ok(0),
            0..=3 => Ok(REPORT[field as usize]),
            _ => Err(Status::InvalidArgs),
        }
    }
}

fn handle(registers: &[u64; 8]) -> Result<u64, Status> {
    let public = [
        HEADER,
        registers[1],
        registers[2],
        registers[3],
        registers[4],
        registers[5],
        registers[6],
        registers[7],
    ];
    if let Ok(request) = Request::decode(public) {
        return unsafe {
            match request {
                Request::RegisterPinned {
                    token,
                    address,
                    length,
                    access,
                } if access & SHARED_READ != 0 => {
                    if REGISTERED_TOKEN != 0 && REGISTERED_TOKEN != token {
                        return Err(Status::Busy);
                    }
                    REGISTERED_TOKEN = token;
                    REGISTERED_ADDRESS = address;
                    REGISTERED_LENGTH = length;
                    Ok(token | PINNED_HANDLE_TAG)
                }
                Request::Unregister { handle } if valid_owner(handle) => {
                    if !CANDIDATE_READY {
                        clear_stage();
                    }
                    REGISTERED_TOKEN = 0;
                    REGISTERED_ADDRESS = 0;
                    REGISTERED_LENGTH = 0;
                    Ok(0)
                }
                Request::RegisterPinned { .. } | Request::Unregister { .. } => {
                    Err(Status::InvalidHandle)
                }
                _ => Err(Status::Unsupported),
            }
        };
    }
    let call = abi::Call::decode(public)?;
    if !valid_owner(call.owner()) {
        return Err(Status::InvalidHandle);
    }
    unsafe {
        match call {
            abi::Call::Query { field, .. } => query(field),
            abi::Call::Begin {
                length,
                generation,
                component,
                ..
            } if component == abi::TRUSTY && generation > ACTIVE_GENERATION => {
                clear_stage();
                STAGED_LENGTH = length;
                STAGED_GENERATION = generation;
                STAGED_COMPONENT = component;
                STAGED_SLOT = if ACTIVE_SLOT == 1 { 2 } else { 1 };
                REPORT = [abi::STAGED, generation, component, STAGED_SLOT];
                REVISION = REVISION.wrapping_add(1);
                Ok(0)
            }
            abi::Call::Write { offset, length, .. }
                if STAGED_LENGTH != 0
                    && !STAGED_SEALED
                    && offset == STAGED_WRITTEN
                    && length <= REGISTERED_LENGTH
                    && offset
                        .checked_add(length)
                        .is_some_and(|end| end <= STAGED_LENGTH) =>
            {
                core::ptr::copy_nonoverlapping(
                    REGISTERED_ADDRESS as *const u8,
                    (layout::UPLOAD_BASE + offset) as *mut u8,
                    length as usize,
                );
                STAGED_WRITTEN += length;
                Ok(STAGED_WRITTEN)
            }
            abi::Call::Seal { .. } if STAGED_LENGTH != 0 && STAGED_WRITTEN == STAGED_LENGTH => {
                let bundle = core::slice::from_raw_parts_mut(
                    layout::UPLOAD_BASE as *mut u8,
                    STAGED_LENGTH as usize,
                );
                {
                    let verified = bexos_secure_firmware::verify(
                        bundle,
                        ROOT,
                        Architecture::Aarch64,
                        Component::Trusty,
                        ACTIVE_GENERATION,
                        ACTIVE_GENERATION,
                    )
                    .map_err(|_| Status::AccessDenied)?;
                    if verified.generation != STAGED_GENERATION {
                        return Err(Status::AccessDenied);
                    }
                    let (generation, migration_abi, fixture) = candidate_manifest(verified.image)?;
                    if generation != verified.generation || migration_abi != 1 || fixture > 3 {
                        REPORT = [
                            abi::ROLLED_BACK,
                            STAGED_GENERATION,
                            abi::TRUSTY,
                            STAGED_SLOT,
                        ];
                        REVISION = REVISION.wrapping_add(1);
                        return Err(Status::AccessDenied);
                    }
                }
                let state = arm_persistent::load_state().map_err(|_| Status::AccessDenied)?;
                if state.active.generation != ACTIVE_GENERATION
                    || arm_persistent::slot_number(state.active.slot) != ACTIVE_SLOT
                {
                    REPORT = [abi::RECOVERY_REQUIRED, STAGED_GENERATION, abi::TRUSTY, 0];
                    REVISION = REVISION.wrapping_add(1);
                    return Err(Status::AccessDenied);
                }
                let (_, identity) = arm_persistent::install(state, bundle, ROOT)
                    .map_err(|_| Status::AccessDenied)?;
                let verified_image = arm_persistent::load_image(identity, ROOT, bundle)
                    .map_err(|_| Status::AccessDenied)?;
                let base = if STAGED_SLOT == 1 {
                    layout::TRUSTY_A_BASE
                } else {
                    layout::TRUSTY_B_BASE
                };
                load_candidate(verified_image, base)?;
                STAGED_DIGEST = identity.digest;
                STAGED_IMAGE_LENGTH = identity.length;
                STAGED_SEALED = true;
                bundle.fill(0);
                Ok(STAGED_GENERATION)
            }
            abi::Call::Abort { .. } if CANDIDATE_READY => Ok(0),
            abi::Call::Abort { .. } => {
                core::slice::from_raw_parts_mut(
                    layout::UPLOAD_BASE as *mut u8,
                    STAGED_LENGTH as usize,
                )
                .fill(0);
                if STAGED_LENGTH != 0 && REPORT[0] != abi::COMMITTED && REPORT[0] != abi::PENDING {
                    let base = if STAGED_SLOT == 1 {
                        layout::TRUSTY_A_BASE
                    } else {
                        layout::TRUSTY_B_BASE
                    };
                    core::slice::from_raw_parts_mut(base as *mut u8, TRUSTY_RUNTIME_BYTES).fill(0);
                    // RolledBack reports the discarded candidate slot.  The
                    // normal-world codec derives the still-active slot as its
                    // opposite, matching the executed-candidate rollback path.
                    REPORT = [
                        abi::ROLLED_BACK,
                        STAGED_GENERATION,
                        abi::TRUSTY,
                        STAGED_SLOT,
                    ];
                    REVISION = REVISION.wrapping_add(1);
                }
                clear_stage();
                Ok(0)
            }
            abi::Call::Activate { mode, .. } if STAGED_SEALED => {
                match mode {
                    abi::ON_REBOOT => {
                        let state =
                            arm_persistent::load_state().map_err(|_| Status::AccessDenied)?;
                        let slot = if STAGED_SLOT == 1 {
                            bexos_secure_firmware::selection::Slot::A
                        } else {
                            bexos_secure_firmware::selection::Slot::B
                        };
                        let candidate = bexos_secure_firmware::selection::Identity {
                            slot,
                            generation: STAGED_GENERATION,
                            digest: STAGED_DIGEST,
                            length: STAGED_IMAGE_LENGTH,
                        };
                        arm_persistent::save_state(arm_persistent::State {
                            revision: state.revision,
                            phase: arm_persistent::Phase::Pending,
                            active: state.active,
                            pending: Some(candidate),
                        })
                        .map_err(|_| Status::AccessDenied)?;
                        REPORT = [abi::PENDING, STAGED_GENERATION, abi::TRUSTY, STAGED_SLOT];
                    }
                    abi::LIVE => {
                        SOURCE_GENERATION = ACTIVE_GENERATION;
                        SOURCE_SLOT = ACTIVE_SLOT;
                        CANDIDATE_READY = false;
                        REPORT = [abi::APPLYING, STAGED_GENERATION, abi::TRUSTY, STAGED_SLOT];
                        ACTIVATION_ACTION = STAGED_SLOT;
                    }
                    _ => return Err(Status::InvalidArgs),
                }
                REVISION = REVISION.wrapping_add(1);
                Ok(STAGED_GENERATION)
            }
            abi::Call::Resolve { disposition, .. }
                if CANDIDATE_READY && REPORT[0] == abi::APPLYING =>
            {
                match disposition {
                    abi::RESOLVE_COMMIT => commit_candidate(),
                    abi::RESOLVE_ROLLBACK => rollback_candidate(),
                    _ => Err(Status::InvalidArgs),
                }
            }
            _ => Err(Status::InvalidArgs),
        }
    }
}

/// `registers` contains the SMC envelope in x0..x7. Return values replace x0
/// and x1 with protocol status and value for the owner's NS_RETURN call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bexos_arm_owner_firmware(registers: *mut [u64; 8]) -> u64 {
    let registers = unsafe { &mut *registers };
    let result = Guard::acquire().and_then(|_guard| handle(registers));
    let (status, value) = match result {
        Ok(value) => (Status::Ok as i64 as u64, value),
        Err(status) => (status as i64 as u64, 0),
    };
    registers.fill(0);
    registers[0] = status;
    registers[1] = value;
    unsafe {
        let action = ACTIVATION_ACTION;
        ACTIVATION_ACTION = 0;
        action
    }
}

/// Called only after the newly selected bank reaches its first architectural
/// return to the normal world. At that point its kernel and built-in TAs have
/// completed startup and the transport can be rebound to a fresh generation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bexos_arm_owner_candidate_ready() -> u64 {
    unsafe {
        if REPORT[0] != abi::APPLYING || STAGED_SLOT == 0 {
            return 0;
        }
        CANDIDATE_READY = true;
        ACTIVE_GENERATION = STAGED_GENERATION;
        ACTIVE_SLOT = STAGED_SLOT;
        TRANSPORT_GENERATION = TRANSPORT_GENERATION.wrapping_add(1).max(1);
        REVISION = REVISION.wrapping_add(1);
        ACTIVE_GENERATION
    }
}

unsafe fn candidate_identity() -> bexos_secure_firmware::selection::Identity {
    unsafe {
        bexos_secure_firmware::selection::Identity {
            slot: if STAGED_SLOT == 1 {
                bexos_secure_firmware::selection::Slot::A
            } else {
                bexos_secure_firmware::selection::Slot::B
            },
            generation: STAGED_GENERATION,
            digest: STAGED_DIGEST,
            length: STAGED_IMAGE_LENGTH,
        }
    }
}

unsafe fn commit_candidate() -> Result<u64, Status> {
    unsafe {
        let state = arm_persistent::load_state().map_err(|_| Status::AccessDenied)?;
        let candidate = candidate_identity();
        if state.pending.is_some_and(|pending| pending != candidate) {
            REPORT = [
                abi::RECOVERY_REQUIRED,
                STAGED_GENERATION,
                abi::TRUSTY,
                SOURCE_SLOT,
            ];
            REVISION = REVISION.wrapping_add(1);
            return Err(Status::AccessDenied);
        }
        arm_persistent::save_state(arm_persistent::State {
            revision: state.revision,
            phase: arm_persistent::Phase::Idle,
            active: candidate,
            pending: None,
        })
        .map_err(|_| Status::AccessDenied)?;
        let retired = if SOURCE_SLOT == 1 {
            layout::TRUSTY_A_BASE
        } else {
            layout::TRUSTY_B_BASE
        };
        core::slice::from_raw_parts_mut(retired as *mut u8, TRUSTY_RUNTIME_BYTES).fill(0);
        core::slice::from_raw_parts_mut(layout::UPLOAD_BASE as *mut u8, STAGED_LENGTH as usize)
            .fill(0);
        REPORT = [abi::COMMITTED, ACTIVE_GENERATION, abi::TRUSTY, ACTIVE_SLOT];
        REVISION = REVISION.wrapping_add(1);
        CANDIDATE_READY = false;
        BOOT_TRIAL = false;
        STAGED_LENGTH = 0;
        STAGED_WRITTEN = 0;
        STAGED_SEALED = false;
        STAGED_SLOT = 0;
        ACTIVATION_ACTION = 3;
        Ok(ACTIVE_GENERATION)
    }
}

unsafe fn rollback_candidate() -> Result<u64, Status> {
    unsafe {
        let state = arm_persistent::load_state().map_err(|_| Status::AccessDenied)?;
        if state.phase != arm_persistent::Phase::Idle {
            arm_persistent::save_state(arm_persistent::State {
                revision: state.revision,
                phase: arm_persistent::Phase::Idle,
                active: state.active,
                pending: None,
            })
            .map_err(|_| Status::AccessDenied)?;
        }
        let candidate_slot = STAGED_SLOT;
        let candidate = if candidate_slot == 1 {
            layout::TRUSTY_A_BASE
        } else {
            layout::TRUSTY_B_BASE
        };
        core::slice::from_raw_parts_mut(candidate as *mut u8, TRUSTY_RUNTIME_BYTES).fill(0);
        core::slice::from_raw_parts_mut(layout::UPLOAD_BASE as *mut u8, STAGED_LENGTH as usize)
            .fill(0);
        ACTIVE_GENERATION = SOURCE_GENERATION;
        ACTIVE_SLOT = SOURCE_SLOT;
        TRANSPORT_GENERATION = TRANSPORT_GENERATION.wrapping_add(1).max(1);
        REPORT = [
            abi::ROLLED_BACK,
            STAGED_GENERATION,
            abi::TRUSTY,
            candidate_slot,
        ];
        REVISION = REVISION.wrapping_add(1);
        CANDIDATE_READY = false;
        STAGED_LENGTH = 0;
        STAGED_WRITTEN = 0;
        STAGED_SEALED = false;
        STAGED_SLOT = 0;
        ACTIVATION_ACTION = 3 + SOURCE_SLOT;
        Ok(ACTIVE_GENERATION)
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn bexos_arm_owner_candidate_failed() -> u64 {
    unsafe {
        if !CANDIDATE_READY && REPORT[0] != abi::APPLYING {
            return 0;
        }
        if rollback_candidate().is_err() {
            REPORT = [
                abi::RECOVERY_REQUIRED,
                STAGED_GENERATION,
                abi::TRUSTY,
                SOURCE_SLOT,
            ];
            REVISION = REVISION.wrapping_add(1);
            return 0;
        }
        let action = ACTIVATION_ACTION;
        ACTIVATION_ACTION = 0;
        action
    }
}

/// Restore the authenticated committed slot before S-EL1 starts. A pending
/// image becomes a durable trial; a trial left by an interrupted boot is
/// rolled back before any candidate byte executes again. Bit 8 tells the
/// assembly owner that the selected image must commit at its first NS return.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bexos_arm_owner_boot_select() -> u64 {
    unsafe {
        let mut state = match arm_persistent::load_state() {
            Ok(state) => state,
            Err(_) => return 0,
        };
        if state.phase == arm_persistent::Phase::Trial {
            let abandoned = state.pending;
            state = match arm_persistent::save_state(arm_persistent::State {
                revision: state.revision,
                phase: arm_persistent::Phase::Idle,
                active: state.active,
                pending: None,
            }) {
                Ok(state) => state,
                Err(_) => return 0,
            };
            if let Some(abandoned) = abandoned {
                REPORT = [
                    abi::ROLLED_BACK,
                    abandoned.generation,
                    abi::TRUSTY,
                    arm_persistent::slot_number(abandoned.slot),
                ];
            }
        }
        let (selected, trial) = match (state.phase, state.pending) {
            (arm_persistent::Phase::Pending, Some(candidate)) => {
                state = match arm_persistent::save_state(arm_persistent::State {
                    revision: state.revision,
                    phase: arm_persistent::Phase::Trial,
                    active: state.active,
                    pending: Some(candidate),
                }) {
                    Ok(state) => state,
                    Err(_) => return 0,
                };
                REPORT = [
                    abi::APPLYING,
                    candidate.generation,
                    abi::TRUSTY,
                    arm_persistent::slot_number(candidate.slot),
                ];
                SOURCE_GENERATION = state.active.generation;
                SOURCE_SLOT = arm_persistent::slot_number(state.active.slot);
                (candidate, true)
            }
            (arm_persistent::Phase::Idle, None) => (state.active, false),
            _ => return 0,
        };
        let slot = arm_persistent::slot_number(selected.slot);
        if selected != bexos_secure_firmware::selection::Identity::INITIAL {
            let scratch = core::slice::from_raw_parts_mut(
                layout::UPLOAD_BASE as *mut u8,
                layout::UPLOAD_BYTES as usize,
            );
            let base = if slot == 1 {
                layout::TRUSTY_A_BASE
            } else {
                layout::TRUSTY_B_BASE
            };
            {
                let image = match arm_persistent::load_image(selected, ROOT, scratch) {
                    Ok(image) => image,
                    Err(_) => return 0,
                };
                if load_candidate(image, base).is_err() {
                    return 0;
                }
            }
            scratch.fill(0);
        }
        ACTIVE_GENERATION = state.active.generation;
        ACTIVE_SLOT = arm_persistent::slot_number(state.active.slot);
        if trial {
            STAGED_GENERATION = selected.generation;
            STAGED_SLOT = slot;
            STAGED_DIGEST = selected.digest;
            STAGED_IMAGE_LENGTH = selected.length;
            STAGED_SEALED = true;
            BOOT_TRIAL = true;
        }
        slot | if trial { 1 << 8 } else { 0 }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn memcpy(
    destination: *mut c_void,
    source: *const c_void,
    count: usize,
) -> *mut c_void {
    for index in 0..count {
        unsafe {
            core::ptr::write_volatile(
                destination.cast::<u8>().add(index),
                core::ptr::read_volatile(source.cast::<u8>().add(index)),
            );
        }
    }
    destination
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn memset(destination: *mut c_void, value: i32, count: usize) -> *mut c_void {
    for index in 0..count {
        unsafe {
            core::ptr::write_volatile(destination.cast::<u8>().add(index), value as u8);
        }
    }
    destination
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn memcmp(left: *const c_void, right: *const c_void, count: usize) -> i32 {
    for index in 0..count {
        let (a, b) = unsafe {
            (
                *left.cast::<u8>().add(index),
                *right.cast::<u8>().add(index),
            )
        };
        if a != b {
            return i32::from(a) - i32::from(b);
        }
    }
    0
}
