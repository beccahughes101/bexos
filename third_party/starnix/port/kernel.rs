//! BexOS Phase 2 Starnix kernel bootstrap and Linux ELF validation.
extern crate alloc;

use alloc::vec::Vec;
use bexos_elf::{
    arch::Machine,
    load::{ElfLoadError, LoadPlan},
};

pub use starnix_core::{
    Architecture, EACCES, EADDRINUSE, EAFNOSUPPORT, EAGAIN, EBADF, EBUSY, ECHILD, ECONNREFUSED,
    EDEADLK, EEXIST, EFAULT, EINTR, EINVAL, EIO, EISCONN, EISDIR, ELOOP, EMFILE, ENODATA, ENOENT,
    ENOMEM, ENOSPC, ENOSYS, ENOTCONN, ENOTDIR, ENOTEMPTY, ENOTSOCK, ENOTSUP, EOWNERDEAD, EPERM,
    EPIPE, ERANGE, EROFS, ESPIPE, ESRCH, ETIMEDOUT, EXDEV, Syscall, Task, error,
};

pub const PAGE_SIZE: u64 = 4096;
pub const GUEST_STACK_SIZE: u64 = 256 * 1024;
pub const GUEST_STACK_TOP: u64 = 0x60_0000_0000;
pub const GUEST_PIE_LOAD_BIAS: u64 = 0x10_0000_0000;
pub const GUEST_INTERPRETER_LOAD_BIAS: u64 = 0x20_0000_0000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Elf(ElfLoadError),
    DynamicInterpreter,
    InvalidAddress,
    TooManyArguments,
    ArgumentTooLarge,
    Overflow,
    InvalidInterpreter,
}

impl From<ElfLoadError> for Error {
    fn from(value: ElfLoadError) -> Self {
        Self::Elf(value)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Image {
    pub plan: LoadPlan,
    pub interpreter: Option<LoadPlan>,
    pub entry_vaddr: u64,
    pub stack: Vec<u8>,
    pub stack_pointer: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutableMetadata {
    pub interpreter: Option<alloc::string::String>,
    pub program_header_address: u64,
    pub program_header_count: u16,
    pub entry_vaddr: u64,
}

pub fn prepare_image(
    bytes: &[u8],
    architecture: Architecture,
    arguments: &[alloc::string::String],
    environment: &[(alloc::string::String, alloc::string::String)],
) -> Result<Image, Error> {
    if interpreter(bytes)?.is_some() {
        return Err(Error::DynamicInterpreter);
    }
    prepare_image_with_interpreter(bytes, None, architecture, arguments, environment, 0, 0)
}

pub fn validate_executable(
    bytes: &[u8],
    architecture: Architecture,
) -> Result<ExecutableMetadata, Error> {
    let machine = match architecture {
        Architecture::Aarch64 => Machine::Aarch64,
        Architecture::X86_64 => Machine::X86_64,
    };
    let plan = LoadPlan::parse_with_bias(bytes, machine, GUEST_PIE_LOAD_BIAS)?;
    validate_plan(&plan)?;
    metadata(bytes, &plan)
}

pub fn prepare_image_with_interpreter(
    bytes: &[u8],
    interpreter_bytes: Option<&[u8]>,
    architecture: Architecture,
    arguments: &[alloc::string::String],
    environment: &[(alloc::string::String, alloc::string::String)],
    uid: u32,
    gid: u32,
) -> Result<Image, Error> {
    let machine = match architecture {
        Architecture::Aarch64 => Machine::Aarch64,
        Architecture::X86_64 => Machine::X86_64,
    };
    let plan = LoadPlan::parse_with_bias(bytes, machine, GUEST_PIE_LOAD_BIAS)?;
    validate_plan(&plan)?;
    let main = metadata(bytes, &plan)?;
    let interpreter_plan = match (&main.interpreter, interpreter_bytes) {
        (None, None) => None,
        (Some(_), Some(interpreter_bytes)) => {
            if interpreter(interpreter_bytes)?.is_some() {
                return Err(Error::InvalidInterpreter);
            }
            let plan =
                LoadPlan::parse_with_bias(interpreter_bytes, machine, GUEST_INTERPRETER_LOAD_BIAS)?;
            validate_plan(&plan)?;
            Some(plan)
        }
        _ => return Err(Error::InvalidInterpreter),
    };
    let entry_vaddr = interpreter_plan
        .as_ref()
        .map_or(plan.entry_vaddr, |interpreter| interpreter.entry_vaddr);
    let (stack, stack_pointer) = build_initial_stack(
        arguments,
        environment,
        &main,
        interpreter_plan.as_ref(),
        uid,
        gid,
    )?;
    Ok(Image {
        plan,
        interpreter: interpreter_plan,
        entry_vaddr,
        stack,
        stack_pointer,
    })
}

fn validate_plan(plan: &LoadPlan) -> Result<(), Error> {
    if plan.segments.iter().any(|segment| {
        segment.vaddr < bexos_boot::USER_START
            || segment
                .vaddr
                .checked_add(segment.zero_fill.map_or_else(
                    || page_round(segment.file_size).unwrap_or(u64::MAX),
                    |zero| {
                        zero.vaddr
                            .saturating_add(zero.size_bytes)
                            .saturating_sub(segment.vaddr)
                    },
                ))
                .is_none_or(|end| end > bexos_boot::USER_END)
    }) {
        return Err(Error::InvalidAddress);
    }
    Ok(())
}

pub fn interpreter(bytes: &[u8]) -> Result<Option<alloc::string::String>, Error> {
    let phoff = read_u64(bytes, 32)?;
    let phentsize = u64::from(read_u16(bytes, 54)?);
    let phnum = u64::from(read_u16(bytes, 56)?);
    if phentsize != 56 || phnum > 32 {
        return Err(Error::InvalidAddress);
    }
    for index in 0..phnum {
        let offset = phoff
            .checked_add(index.checked_mul(phentsize).ok_or(Error::Overflow)?)
            .ok_or(Error::Overflow)?;
        let offset = usize::try_from(offset).map_err(|_| Error::Overflow)?;
        if read_u32(bytes, offset)? == 3 {
            let file_offset =
                usize::try_from(read_u64(bytes, offset + 8)?).map_err(|_| Error::Overflow)?;
            let file_size =
                usize::try_from(read_u64(bytes, offset + 32)?).map_err(|_| Error::Overflow)?;
            if file_size < 2 || file_size > 4096 {
                return Err(Error::InvalidInterpreter);
            }
            let raw = bytes
                .get(file_offset..file_offset.checked_add(file_size).ok_or(Error::Overflow)?)
                .ok_or(Error::InvalidAddress)?;
            let path = raw.strip_suffix(&[0]).ok_or(Error::InvalidInterpreter)?;
            if !path.starts_with(b"/") || path.contains(&0) {
                return Err(Error::InvalidInterpreter);
            }
            return alloc::string::String::from_utf8(path.to_vec())
                .map(Some)
                .map_err(|_| Error::InvalidInterpreter);
        }
    }
    Ok(None)
}

fn metadata(bytes: &[u8], plan: &LoadPlan) -> Result<ExecutableMetadata, Error> {
    let phoff = read_u64(bytes, 32)?;
    let phentsize = u64::from(read_u16(bytes, 54)?);
    let phnum = read_u16(bytes, 56)?;
    let load_bias = if read_u16(bytes, 16)? == 3 {
        GUEST_PIE_LOAD_BIAS
    } else {
        0
    };
    let mut phdr = None;
    for index in 0..u64::from(phnum) {
        let offset = usize::try_from(
            phoff
                .checked_add(index.checked_mul(phentsize).ok_or(Error::Overflow)?)
                .ok_or(Error::Overflow)?,
        )
        .map_err(|_| Error::Overflow)?;
        if read_u32(bytes, offset)? == 6 {
            let vaddr = read_u64(bytes, offset + 16)?;
            phdr = Some(vaddr.checked_add(load_bias).ok_or(Error::Overflow)?);
            break;
        }
    }
    let program_header_address = phdr.or_else(|| {
        plan.segments.iter().find_map(|segment| {
            let end = segment.file_offset.checked_add(segment.file_size)?;
            (phoff >= segment.file_offset && phoff < end)
                .then(|| segment.vaddr.checked_add(phoff - segment.file_offset))?
        })
    });
    Ok(ExecutableMetadata {
        interpreter: interpreter(bytes)?,
        program_header_address: program_header_address.ok_or(Error::InvalidAddress)?,
        program_header_count: phnum,
        entry_vaddr: plan.entry_vaddr,
    })
}

fn build_initial_stack(
    arguments: &[alloc::string::String],
    environment: &[(alloc::string::String, alloc::string::String)],
    executable: &ExecutableMetadata,
    interpreter: Option<&LoadPlan>,
    uid: u32,
    gid: u32,
) -> Result<(Vec<u8>, u64), Error> {
    if arguments.len() > 64 || environment.len() > 64 {
        return Err(Error::TooManyArguments);
    }
    let mut stack = alloc::vec![0u8; GUEST_STACK_SIZE as usize];
    let base = GUEST_STACK_TOP - GUEST_STACK_SIZE;
    let mut cursor = stack.len();
    let mut env_pointers = Vec::new();
    for (name, value) in environment.iter().rev() {
        let length = name
            .len()
            .checked_add(value.len())
            .and_then(|value| value.checked_add(2))
            .ok_or(Error::Overflow)?;
        if length > 4353 || cursor < length {
            return Err(Error::ArgumentTooLarge);
        }
        cursor -= length;
        stack[cursor..cursor + name.len()].copy_from_slice(name.as_bytes());
        stack[cursor + name.len()] = b'=';
        stack[cursor + name.len() + 1..cursor + length - 1].copy_from_slice(value.as_bytes());
        env_pointers.push(base + cursor as u64);
    }
    let mut arg_pointers = Vec::new();
    for argument in arguments.iter().rev() {
        let length = argument.len().checked_add(1).ok_or(Error::Overflow)?;
        if length > 4097 || cursor < length {
            return Err(Error::ArgumentTooLarge);
        }
        cursor -= length;
        stack[cursor..cursor + argument.len()].copy_from_slice(argument.as_bytes());
        arg_pointers.push(base + cursor as u64);
    }
    let execfn = arg_pointers.last().copied().unwrap_or(0);
    if cursor < 16 {
        return Err(Error::ArgumentTooLarge);
    }
    cursor -= 16;
    for (index, byte) in stack[cursor..cursor + 16].iter_mut().enumerate() {
        *byte = (0xa5u8)
            .wrapping_add(index as u8)
            .rotate_left((index & 7) as u32);
    }
    let random = base + cursor as u64;
    cursor &= !15;
    let auxv = [
        (3, executable.program_header_address),
        (4, 56),
        (5, u64::from(executable.program_header_count)),
        (6, PAGE_SIZE),
        (
            7,
            interpreter
                .and_then(|plan| plan.segments.iter().map(|segment| segment.vaddr).min())
                .unwrap_or(0),
        ),
        (9, executable.entry_vaddr),
        (11, u64::from(uid)),
        (12, u64::from(uid)),
        (13, u64::from(gid)),
        (14, u64::from(gid)),
        (23, 0),
        (25, random),
        (31, execfn),
        (0, 0),
    ];
    let words = 1 + arg_pointers.len() + 1 + env_pointers.len() + 1 + auxv.len() * 2;
    let table_bytes = words.checked_mul(8).ok_or(Error::Overflow)?;
    if cursor < table_bytes {
        return Err(Error::ArgumentTooLarge);
    }
    cursor -= table_bytes;
    let mut word = cursor;
    put_word(&mut stack, &mut word, arg_pointers.len() as u64);
    for pointer in arg_pointers.iter().rev() {
        put_word(&mut stack, &mut word, *pointer);
    }
    put_word(&mut stack, &mut word, 0);
    for pointer in env_pointers.iter().rev() {
        put_word(&mut stack, &mut word, *pointer);
    }
    put_word(&mut stack, &mut word, 0);
    for (key, value) in auxv {
        put_word(&mut stack, &mut word, key);
        put_word(&mut stack, &mut word, value);
    }
    Ok((stack, base + cursor as u64))
}

fn put_word(stack: &mut [u8], cursor: &mut usize, value: u64) {
    stack[*cursor..*cursor + 8].copy_from_slice(&value.to_le_bytes());
    *cursor += 8;
}

fn page_round(value: u64) -> Option<u64> {
    value
        .checked_add(PAGE_SIZE - 1)
        .map(|value| value & !(PAGE_SIZE - 1))
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, Error> {
    Ok(u16::from_le_bytes(
        bytes
            .get(offset..offset + 2)
            .ok_or(Error::InvalidAddress)?
            .try_into()
            .unwrap(),
    ))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, Error> {
    Ok(u32::from_le_bytes(
        bytes
            .get(offset..offset + 4)
            .ok_or(Error::InvalidAddress)?
            .try_into()
            .unwrap(),
    ))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, Error> {
    Ok(u64::from_le_bytes(
        bytes
            .get(offset..offset + 8)
            .ok_or(Error::InvalidAddress)?
            .try_into()
            .unwrap(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;

    fn elf(interpreter: Option<&str>) -> Vec<u8> {
        let count = if interpreter.is_some() { 3 } else { 2 };
        let mut bytes = alloc::vec![0; 0x300];
        bytes[..4].copy_from_slice(b"\x7fELF");
        bytes[4] = 2;
        bytes[5] = 1;
        bytes[6] = 1;
        bytes[16..18].copy_from_slice(&3u16.to_le_bytes());
        bytes[18..20].copy_from_slice(&62u16.to_le_bytes());
        bytes[24..32].copy_from_slice(&0x180u64.to_le_bytes());
        bytes[32..40].copy_from_slice(&64u64.to_le_bytes());
        bytes[54..56].copy_from_slice(&56u16.to_le_bytes());
        bytes[56..58].copy_from_slice(&(count as u16).to_le_bytes());
        let phdr = 64;
        bytes[phdr..phdr + 4].copy_from_slice(&6u32.to_le_bytes());
        bytes[phdr + 16..phdr + 24].copy_from_slice(&64u64.to_le_bytes());
        bytes[phdr + 32..phdr + 40].copy_from_slice(&(count as u64 * 56).to_le_bytes());
        bytes[phdr + 40..phdr + 48].copy_from_slice(&(count as u64 * 56).to_le_bytes());
        bytes[phdr + 48..phdr + 56].copy_from_slice(&8u64.to_le_bytes());
        let load = phdr + 56;
        bytes[load..load + 4].copy_from_slice(&1u32.to_le_bytes());
        bytes[load + 4..load + 8].copy_from_slice(&5u32.to_le_bytes());
        bytes[load + 32..load + 40].copy_from_slice(&0x300u64.to_le_bytes());
        bytes[load + 40..load + 48].copy_from_slice(&0x300u64.to_le_bytes());
        bytes[load + 48..load + 56].copy_from_slice(&0x1000u64.to_le_bytes());
        if let Some(path) = interpreter {
            let interp = load + 56;
            let length = path.len() + 1;
            bytes[interp..interp + 4].copy_from_slice(&3u32.to_le_bytes());
            bytes[interp + 8..interp + 16].copy_from_slice(&0x280u64.to_le_bytes());
            bytes[interp + 32..interp + 40].copy_from_slice(&(length as u64).to_le_bytes());
            bytes[interp + 40..interp + 48].copy_from_slice(&(length as u64).to_le_bytes());
            bytes[0x280..0x280 + path.len()].copy_from_slice(path.as_bytes());
        }
        bytes
    }

    #[test]
    fn initial_stack_contains_linux_argv_and_environment_layout() {
        let metadata = ExecutableMetadata {
            interpreter: None,
            program_header_address: 0x400040,
            program_header_count: 4,
            entry_vaddr: 0x401000,
        };
        let (stack, pointer) = build_initial_stack(
            &["hello".to_string(), "world".to_string()],
            &[("LANG".to_string(), "C".to_string())],
            &metadata,
            None,
            0,
            0,
        )
        .unwrap();
        let offset = (pointer - (GUEST_STACK_TOP - GUEST_STACK_SIZE)) as usize;
        assert_eq!(
            u64::from_le_bytes(stack[offset..offset + 8].try_into().unwrap()),
            2
        );
        assert!(stack.windows(6).any(|bytes| bytes == b"hello\0"));
        assert!(stack.windows(7).any(|bytes| bytes == b"LANG=C\0"));
        assert_eq!(pointer & 15, 0);
    }

    #[test]
    fn oversized_argument_is_rejected() {
        let metadata = ExecutableMetadata {
            interpreter: None,
            program_header_address: 0x400040,
            program_header_count: 4,
            entry_vaddr: 0x401000,
        };
        assert_eq!(
            build_initial_stack(&["x".repeat(4097)], &[], &metadata, None, 0, 0),
            Err(Error::ArgumentTooLarge)
        );
    }

    #[test]
    fn dynamic_image_starts_in_interpreter_and_keeps_main_entry() {
        let main = elf(Some("/lib/ld.so"));
        let loader = elf(None);
        assert_eq!(interpreter(&main).unwrap().as_deref(), Some("/lib/ld.so"));
        let image = prepare_image_with_interpreter(
            &main,
            Some(&loader),
            Architecture::X86_64,
            &["tool".to_string()],
            &[],
            1000,
            100,
        )
        .unwrap();
        assert_eq!(image.plan.entry_vaddr, GUEST_PIE_LOAD_BIAS + 0x180);
        assert_eq!(image.entry_vaddr, GUEST_INTERPRETER_LOAD_BIAS + 0x180);
        assert!(image.interpreter.is_some());
    }
}
