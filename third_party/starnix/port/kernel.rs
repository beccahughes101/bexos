//! BexOS Phase 2 Starnix kernel bootstrap and Linux ELF validation.
extern crate alloc;

use alloc::vec::Vec;
use bexos_elf::{
    arch::Machine,
    load::{ElfLoadError, LoadPlan},
};

pub use starnix_core::{Architecture, EBADF, EFAULT, ENOSYS, Syscall, Task, error};

pub const PAGE_SIZE: u64 = 4096;
pub const GUEST_STACK_SIZE: u64 = 256 * 1024;
pub const GUEST_STACK_TOP: u64 = 0x60_0000_0000;
pub const GUEST_PIE_LOAD_BIAS: u64 = 0x10_0000_0000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Elf(ElfLoadError),
    DynamicInterpreter,
    InvalidAddress,
    TooManyArguments,
    ArgumentTooLarge,
    Overflow,
}

impl From<ElfLoadError> for Error {
    fn from(value: ElfLoadError) -> Self {
        Self::Elf(value)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Image {
    pub plan: LoadPlan,
    pub stack: Vec<u8>,
    pub stack_pointer: u64,
}

pub fn prepare_image(
    bytes: &[u8],
    architecture: Architecture,
    arguments: &[alloc::string::String],
    environment: &[(alloc::string::String, alloc::string::String)],
) -> Result<Image, Error> {
    if contains_interpreter(bytes)? {
        return Err(Error::DynamicInterpreter);
    }
    let machine = match architecture {
        Architecture::Aarch64 => Machine::Aarch64,
        Architecture::X86_64 => Machine::X86_64,
    };
    let plan = LoadPlan::parse_with_bias(bytes, machine, GUEST_PIE_LOAD_BIAS)?;
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
    let (stack, stack_pointer) = build_initial_stack(arguments, environment)?;
    Ok(Image {
        plan,
        stack,
        stack_pointer,
    })
}

fn contains_interpreter(bytes: &[u8]) -> Result<bool, Error> {
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
        if read_u32(bytes, usize::try_from(offset).map_err(|_| Error::Overflow)?)? == 3 {
            return Ok(true);
        }
    }
    Ok(false)
}

fn build_initial_stack(
    arguments: &[alloc::string::String],
    environment: &[(alloc::string::String, alloc::string::String)],
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
    cursor &= !15;
    let words = 1 + arg_pointers.len() + 1 + env_pointers.len() + 1 + 2;
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
    put_word(&mut stack, &mut word, 0); // AT_NULL
    put_word(&mut stack, &mut word, 0);
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
    use alloc::{string::ToString, vec};

    #[test]
    fn initial_stack_contains_linux_argv_and_environment_layout() {
        let (stack, pointer) = build_initial_stack(
            &["hello".to_string(), "world".to_string()],
            &[("LANG".to_string(), "C".to_string())],
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
        assert_eq!(
            build_initial_stack(&["x".repeat(4097)], &[]),
            Err(Error::ArgumentTooLarge)
        );
    }
}
