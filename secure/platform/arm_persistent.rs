//! Root-only persistent A/B storage for the QEMU S-EL2 execution owner.
//!
//! QEMU maps this file-backed region only into the secure address space.  The
//! keyed records still authenticate every state transition so corruption and
//! host-side fixture tampering fail closed before a slot can be selected.

use bexos_secure_firmware::selection::{Identity, Slot};
use bexos_secure_firmware::store::{self, BlockDevice, Error as StoreError, SECTOR_BYTES};
use bexos_secure_firmware::{Architecture, Component};
use core::sync::atomic::{Ordering, compiler_fence};

use crate::layout;

const MAGIC: &[u8; 8] = b"BEXAP001";
const VERSION: u32 = 1;
const RECORD_BYTES: usize = SECTOR_BYTES;
const RECORD_COPIES: usize = 2;
const AUTH_OFFSET: usize = RECORD_BYTES - 32;
const STATE_KEY: [u8; 32] = [
    0x42, 0x65, 0x78, 0x4f, 0x53, 0x2d, 0x41, 0x52, 0x4d, 0x2d, 0x6f, 0x77, 0x6e, 0x65, 0x72, 0x2d,
    0x73, 0x74, 0x61, 0x74, 0x65, 0x2d, 0x76, 0x31, 0x91, 0x6d, 0x27, 0xa8, 0x53, 0xcf, 0x10, 0x7b,
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Phase {
    Idle,
    Pending,
    Trial,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct State {
    pub revision: u64,
    pub phase: Phase,
    pub active: Identity,
    pub pending: Option<Identity>,
}

impl State {
    const INITIAL: Self = Self {
        revision: 1,
        phase: Phase::Idle,
        active: Identity::INITIAL,
        pending: None,
    };

    fn valid(self) -> bool {
        if self.revision == 0 || !self.active.valid(true) {
            return false;
        }
        match (self.phase, self.pending) {
            (Phase::Idle, None) => true,
            (Phase::Pending | Phase::Trial, Some(candidate)) => {
                candidate.valid(false)
                    && candidate.slot != self.active.slot
                    && candidate.generation > self.active.generation
            }
            _ => false,
        }
    }

    fn encode(self) -> [u8; RECORD_BYTES] {
        let mut bytes = [0; RECORD_BYTES];
        bytes[..8].copy_from_slice(MAGIC);
        bytes[8..12].copy_from_slice(&VERSION.to_le_bytes());
        bytes[12..16].copy_from_slice(&Architecture::Aarch64.number().to_le_bytes());
        bytes[16..24].copy_from_slice(&self.revision.to_le_bytes());
        bytes[24..28].copy_from_slice(
            &(match self.phase {
                Phase::Idle => 0u32,
                Phase::Pending => 1,
                Phase::Trial => 2,
            })
            .to_le_bytes(),
        );
        bytes[32..88].copy_from_slice(&self.active.encode());
        if let Some(pending) = self.pending {
            bytes[88..144].copy_from_slice(&pending.encode());
        }
        let tag = blake3::keyed_hash(&STATE_KEY, &bytes[..AUTH_OFFSET]);
        bytes[AUTH_OFFSET..].copy_from_slice(tag.as_bytes());
        bytes
    }

    fn decode(bytes: &[u8; RECORD_BYTES]) -> Option<Self> {
        let expected = blake3::keyed_hash(&STATE_KEY, &bytes[..AUTH_OFFSET]);
        if &bytes[..8] != MAGIC
            || u32::from_le_bytes(bytes[8..12].try_into().ok()?) != VERSION
            || u32::from_le_bytes(bytes[12..16].try_into().ok()?) != Architecture::Aarch64.number()
            || expected.as_bytes() != &bytes[AUTH_OFFSET..]
            || bytes[28..32] != [0; 4]
            || bytes[144..AUTH_OFFSET] != [0; AUTH_OFFSET - 144]
        {
            return None;
        }
        let phase = match u32::from_le_bytes(bytes[24..28].try_into().ok()?) {
            0 => Phase::Idle,
            1 => Phase::Pending,
            2 => Phase::Trial,
            _ => return None,
        };
        let pending = if phase == Phase::Idle {
            if bytes[88..144] != [0; 56] {
                return None;
            }
            None
        } else {
            Some(Identity::decode(&bytes[88..144], false).ok()?)
        };
        let state = Self {
            revision: u64::from_le_bytes(bytes[16..24].try_into().ok()?),
            phase,
            active: Identity::decode(&bytes[32..88], true).ok()?,
            pending,
        };
        state.valid().then_some(state)
    }
}

struct Disk;
impl BlockDevice for Disk {
    fn architecture(&self) -> Architecture {
        Architecture::Aarch64
    }

    fn sectors(&self) -> u64 {
        layout::PERSISTENT_BYTES / SECTOR_BYTES as u64
    }

    fn read(&mut self, sector: u64, output: &mut [u8; SECTOR_BYTES]) -> Result<(), StoreError> {
        let offset = sector
            .checked_mul(SECTOR_BYTES as u64)
            .filter(|offset| offset + SECTOR_BYTES as u64 <= layout::PERSISTENT_BYTES)
            .ok_or(StoreError::Device)?;
        for (index, byte) in output.iter_mut().enumerate() {
            *byte = unsafe {
                core::ptr::read_volatile(
                    (layout::PERSISTENT_BASE + offset + index as u64) as *const u8,
                )
            };
        }
        Ok(())
    }

    fn write(&mut self, sector: u64, input: &[u8; SECTOR_BYTES]) -> Result<(), StoreError> {
        let offset = sector
            .checked_mul(SECTOR_BYTES as u64)
            .filter(|offset| offset + SECTOR_BYTES as u64 <= layout::PERSISTENT_BYTES)
            .ok_or(StoreError::Device)?;
        for (index, byte) in input.iter().enumerate() {
            unsafe {
                core::ptr::write_volatile(
                    (layout::PERSISTENT_BASE + offset + index as u64) as *mut u8,
                    *byte,
                );
            }
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), StoreError> {
        compiler_fence(Ordering::SeqCst);
        unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)) };
        Ok(())
    }
}

fn read_record(index: usize) -> Option<State> {
    let mut bytes = [0; RECORD_BYTES];
    Disk.read(index as u64, &mut bytes).ok()?;
    State::decode(&bytes)
}

pub fn load_state() -> Result<State, StoreError> {
    let selected = (0..RECORD_COPIES)
        .filter_map(read_record)
        .max_by_key(|state| state.revision)
        .map(Ok);
    if let Some(selected) = selected {
        return selected;
    }
    let mut record = [0; RECORD_BYTES];
    let mut disk = Disk;
    for index in 0..RECORD_COPIES {
        disk.read(index as u64, &mut record)?;
        if record != [0; RECORD_BYTES] {
            return Err(StoreError::Header);
        }
    }
    Ok(State::INITIAL)
}

pub fn save_state(mut state: State) -> Result<State, StoreError> {
    if !state.valid() {
        return Err(StoreError::Header);
    }
    state.revision = state.revision.checked_add(1).ok_or(StoreError::Header)?;
    let index = state.revision as usize % RECORD_COPIES;
    let bytes = state.encode();
    let mut disk = Disk;
    disk.write(index as u64, &[0; RECORD_BYTES])?;
    disk.flush()?;
    disk.write(index as u64, &bytes)?;
    disk.flush()?;
    read_record(index)
        .filter(|readback| *readback == state)
        .ok_or(StoreError::Device)
}

pub fn install(
    state: State,
    bundle: &mut [u8],
    root: &[u8],
) -> Result<(State, Identity), StoreError> {
    if state.phase != Phase::Idle || state.pending.is_some() {
        return Err(StoreError::Header);
    }
    let identity = store::install_in_place(
        &mut Disk,
        Component::Trusty,
        state.active,
        bundle,
        root,
        state.active.generation,
    )?;
    Ok((state, identity))
}

pub fn load_image<'a>(
    identity: Identity,
    root: &[u8],
    scratch: &'a mut [u8],
) -> Result<&'a [u8], StoreError> {
    store::load(
        &mut Disk,
        Component::Trusty,
        identity,
        root,
        identity.generation,
        scratch,
    )
    .map(|verified| verified.image)
}

pub fn slot_number(slot: Slot) -> u64 {
    u64::from(slot.number())
}
