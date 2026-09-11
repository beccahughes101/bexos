use crate::{Memory, yield_now};
use alloc::vec::Vec;
pub use bexos_boot::ARCHITECTURE_ID;
pub use bexos_elf::runtime::{EncodedSymbol, TlsModule, encode_linker_data, encode_linker_data_v3};
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};
use kernel_fidl::Status;

const LEGACY_MAGIC: &[u8; 8] = b"BXLINK01";
const LINKER_MAGIC: &[u8; 8] = b"BXLINK02";
const LINKER_VERSION: u32 = 2;

#[derive(Clone, Debug, Eq, PartialEq)]
struct RuntimeSymbol {
    name: Vec<u8>,
    address: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct RuntimeState {
    tls_modules: Vec<TlsModule>,
    symbols: Vec<RuntimeSymbol>,
    constructors: Vec<u64>,
    tls_template: Vec<u8>,
    tls_mem_size: u64,
    tls_align: u64,
}

struct Locked<T> {
    locked: AtomicBool,
    value: UnsafeCell<T>,
}

unsafe impl<T> Sync for Locked<T> {}

impl<T> Locked<T> {
    const fn new(value: T) -> Self {
        Self {
            locked: AtomicBool::new(false),
            value: UnsafeCell::new(value),
        }
    }

    fn with<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        while self.locked.swap(true, Ordering::Acquire) {
            yield_now();
        }
        let result = f(unsafe { &mut *self.value.get() });
        self.locked.store(false, Ordering::Release);
        result
    }
}

static RUNTIME: Locked<RuntimeState> = Locked::new(RuntimeState {
    tls_modules: Vec::new(),
    symbols: Vec::new(),
    constructors: Vec::new(),
    tls_template: Vec::new(),
    tls_mem_size: 0,
    tls_align: 1,
});

pub fn install_from_vmo(handle: u64, len: u64) -> Result<(), Status> {
    if len == 0 {
        return Ok(());
    }
    let rounded = bexos_boot::page_round(len).ok_or(Status::ErrInvalidArgs)?;
    let va = Memory::map(handle, rounded, 2)?;
    let bytes = unsafe { core::slice::from_raw_parts(va as *const u8, len as usize) };
    let result = install(bytes);
    let _ = Memory::unmap(va, rounded);
    result
}

pub fn install(bytes: &[u8]) -> Result<(), Status> {
    let parsed = parse(bytes)?;
    let constructors = parsed.constructors.clone();
    RUNTIME.with(|runtime| *runtime = parsed);
    for address in constructors {
        if address != 0 {
            let init: extern "C" fn() = unsafe { core::mem::transmute(address as usize) };
            init();
        }
    }
    Ok(())
}

pub fn symbol_address(name: &[u8]) -> Option<u64> {
    RUNTIME.with(|runtime| {
        runtime
            .symbols
            .iter()
            .find(|symbol| symbol.name == name)
            .map(|symbol| symbol.address)
    })
}

pub fn copy_tls_template(out: &mut [u8]) -> Option<(usize, usize, u64)> {
    RUNTIME.with(|runtime| {
        if runtime.tls_mem_size == 0 {
            return None;
        }
        let mem_size = runtime.tls_mem_size as usize;
        if out.len() < mem_size {
            return None;
        }
        let file_size = runtime.tls_template.len();
        out[..file_size].copy_from_slice(&runtime.tls_template);
        out[file_size..mem_size].fill(0);
        Some((file_size, mem_size, runtime.tls_align))
    })
}

pub fn tls_layout() -> Option<(usize, u64)> {
    RUNTIME.with(|runtime| {
        if runtime.tls_mem_size == 0 {
            None
        } else {
            Some((runtime.tls_mem_size as usize, runtime.tls_align))
        }
    })
}

fn parse(bytes: &[u8]) -> Result<RuntimeState, Status> {
    if bytes.len() < 16 {
        return Err(Status::ErrInvalidArgs);
    }
    if &bytes[..8] == LEGACY_MAGIC {
        if bexos_boot::ARCHITECTURE_ID != 1 {
            return Err(Status::ErrInvalidArgs);
        }
        return parse_legacy(bytes);
    }
    let version3 = &bytes[..8] == b"BXLINK03";
    if bytes.len() < 36
        || (!version3 && (&bytes[..8] != LINKER_MAGIC || bexos_boot::ARCHITECTURE_ID != 1))
    {
        return Err(Status::ErrInvalidArgs);
    }
    if read_u32(bytes, 8)? != if version3 { 3 } else { LINKER_VERSION } {
        return Err(Status::ErrInvalidArgs);
    }
    let symbol_count = read_u32(bytes, 12)? as usize;
    let constructor_count = read_u32(bytes, 16)? as usize;
    let tls_template_size = read_u32(bytes, 20)? as usize;
    let tls_mem_size = read_u32(bytes, 24)? as u64;
    let tls_align = read_u32(bytes, 28)? as u64;
    if tls_template_size as u64 > tls_mem_size || tls_align == 0 || !tls_align.is_power_of_two() {
        return Err(Status::ErrInvalidArgs);
    }

    let mut offset = 36usize;
    let mut tls_modules = Vec::new();
    if version3 {
        if read_u64(bytes, offset)? != bexos_boot::ARCHITECTURE_ID {
            return Err(Status::ErrInvalidArgs);
        }
        let count = read_u64(bytes, offset + 8)?;
        if count > 65 {
            return Err(Status::ErrInvalidArgs);
        }
        offset += 16;
        for _ in 0..count {
            let module = TlsModule {
                thread_offset: read_u64(bytes, offset)? as i64,
                mem_size: read_u64(bytes, offset + 8)?,
            };
            let start = module.thread_offset as i128;
            let end = start + module.mem_size as i128;
            let valid = if bexos_boot::ARCHITECTURE_ID == 1 {
                start >= 16 && end <= 16 + tls_mem_size as i128
            } else {
                start >= -(tls_mem_size as i128) && end <= 0
            };
            if module.mem_size != 0 && !valid {
                return Err(Status::ErrInvalidArgs);
            }
            tls_modules.push(module);
            offset += 16;
        }
    }

    let mut symbols = Vec::new();
    for _ in 0..symbol_count {
        let name_len = read_u16(bytes, offset)? as usize;
        let address = read_u64(bytes, offset + 4)?;
        offset = offset.checked_add(12).ok_or(Status::ErrInvalidArgs)?;
        let end = offset.checked_add(name_len).ok_or(Status::ErrInvalidArgs)?;
        let name = bytes.get(offset..end).ok_or(Status::ErrInvalidArgs)?;
        symbols.push(RuntimeSymbol {
            name: name.to_vec(),
            address,
        });
        offset = end;
    }

    let mut constructors = Vec::new();
    for _ in 0..constructor_count {
        constructors.push(read_u64(bytes, offset)?);
        offset = offset.checked_add(8).ok_or(Status::ErrInvalidArgs)?;
    }

    let tls_end = offset
        .checked_add(tls_template_size)
        .ok_or(Status::ErrInvalidArgs)?;
    let tls_template = bytes
        .get(offset..tls_end)
        .ok_or(Status::ErrInvalidArgs)?
        .to_vec();
    if tls_end != bytes.len() {
        return Err(Status::ErrInvalidArgs);
    }

    Ok(RuntimeState {
        tls_modules,
        symbols,
        constructors,
        tls_template,
        tls_mem_size,
        tls_align,
    })
}

fn parse_legacy(bytes: &[u8]) -> Result<RuntimeState, Status> {
    if read_u32(bytes, 8)? != 1 {
        return Err(Status::ErrInvalidArgs);
    }
    let count = read_u32(bytes, 12)? as usize;
    let mut offset = 16usize;
    let mut symbols = Vec::new();
    for _ in 0..count {
        let name_len = read_u16(bytes, offset)? as usize;
        let address = read_u64(bytes, offset + 4)?;
        offset = offset.checked_add(12).ok_or(Status::ErrInvalidArgs)?;
        let end = offset.checked_add(name_len).ok_or(Status::ErrInvalidArgs)?;
        let name = bytes.get(offset..end).ok_or(Status::ErrInvalidArgs)?;
        symbols.push(RuntimeSymbol {
            name: name.to_vec(),
            address,
        });
        offset = end;
    }
    if offset != bytes.len() {
        return Err(Status::ErrInvalidArgs);
    }
    Ok(RuntimeState {
        tls_modules: Vec::new(),
        symbols,
        constructors: Vec::new(),
        tls_template: Vec::new(),
        tls_mem_size: 0,
        tls_align: 1,
    })
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, Status> {
    let raw = bytes
        .get(offset..offset + 2)
        .ok_or(Status::ErrInvalidArgs)?;
    Ok(u16::from_le_bytes([raw[0], raw[1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, Status> {
    let raw = bytes
        .get(offset..offset + 4)
        .ok_or(Status::ErrInvalidArgs)?;
    Ok(u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, Status> {
    let raw = bytes
        .get(offset..offset + 8)
        .ok_or(Status::ErrInvalidArgs)?;
    Ok(u64::from_le_bytes([
        raw[0], raw[1], raw[2], raw[3], raw[4], raw[5], raw[6], raw[7],
    ]))
}

pub fn tls_address(thread_pointer: u64, module: u64, offset: u64) -> Option<u64> {
    RUNTIME.with(|state| {
        let module = state
            .tls_modules
            .get(usize::try_from(module.checked_sub(1)?).ok()?)?;
        if offset >= module.mem_size {
            return None;
        }
        thread_pointer
            .checked_add_signed(module.thread_offset)?
            .checked_add(offset)
    })
}

/// ELF general-dynamic TLS entry. BexOS resolves library calls to this
/// executable-owned implementation, which owns the process link map.
#[cfg_attr(bexos_guest, unsafe(no_mangle))]
pub unsafe extern "C" fn __tls_get_addr(index: *const u64) -> *mut u8 {
    if index.is_null() {
        return core::ptr::null_mut();
    }
    let module = unsafe { index.read() };
    let offset = unsafe { index.add(1).read() };
    tls_address(crate::syscall::thread_pointer(), module, offset)
        .map_or(core::ptr::null_mut(), |address| address as *mut u8)
}
