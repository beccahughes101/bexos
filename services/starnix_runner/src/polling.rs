use crate::{abi, memory::AddressSpace, vfs::Vfs};
use starnix_kernel::EINVAL;

const POLLNVAL: i16 = 0x20;
const MAX_FDS: usize = 256;

pub fn poll(
    memory: &mut AddressSpace,
    vfs: &Vfs,
    address: u64,
    count: u64,
    timeout_nanos: Option<u64>,
) -> Result<u64, i64> {
    let count = usize::try_from(count).map_err(|_| EINVAL)?;
    if count > MAX_FDS {
        return Err(EINVAL);
    }
    let table = memory
        .read(address, count.checked_mul(8).ok_or(EINVAL)?)?
        .to_vec();
    let deadline = deadline(timeout_nanos);
    loop {
        let mut ready = 0u64;
        for index in 0..count {
            let offset = index * 8;
            let fd = i32::from_ne_bytes(table[offset..offset + 4].try_into().unwrap());
            let events = i16::from_ne_bytes(table[offset + 4..offset + 6].try_into().unwrap());
            let revents = if fd < 0 {
                0
            } else {
                vfs.readiness(fd, events).unwrap_or(POLLNVAL)
            };
            memory.write(address + offset as u64 + 6, &revents.to_ne_bytes())?;
            ready += u64::from(revents != 0);
        }
        if ready != 0 || expired(deadline) {
            return Ok(ready);
        }
        bexos_userspace::yield_now();
    }
}

pub fn select(
    memory: &mut AddressSpace,
    vfs: &Vfs,
    nfds: u64,
    read_address: u64,
    write_address: u64,
    except_address: u64,
    timeout_nanos: Option<u64>,
) -> Result<u64, i64> {
    let nfds = usize::try_from(nfds).map_err(|_| EINVAL)?;
    if nfds > MAX_FDS {
        return Err(EINVAL);
    }
    let bytes = nfds.div_ceil(64) * 8;
    let requested_read = read_set(memory, read_address, bytes)?;
    let requested_write = read_set(memory, write_address, bytes)?;
    let deadline = deadline(timeout_nanos);
    loop {
        let mut read = vec![0; bytes];
        let mut write = vec![0; bytes];
        let mut ready = 0u64;
        for fd in 0..nfds {
            let wants_read = is_set(&requested_read, fd);
            let wants_write = is_set(&requested_write, fd);
            if !wants_read && !wants_write {
                continue;
            }
            let mut events = 0;
            if wants_read {
                events |= 0x001;
            }
            if wants_write {
                events |= 0x004;
            }
            let events = vfs.readiness(fd as i32, events)?;
            let readable = wants_read && events & (0x001 | 0x010) != 0;
            let writable = wants_write && events & 0x004 != 0;
            if readable {
                set(&mut read, fd);
            }
            if writable {
                set(&mut write, fd);
            }
            ready += u64::from(readable || writable);
        }
        if ready != 0 || expired(deadline) {
            write_set(memory, read_address, &read)?;
            write_set(memory, write_address, &write)?;
            if except_address != 0 {
                memory.write(except_address, &vec![0; bytes])?;
            }
            return Ok(ready);
        }
        bexos_userspace::yield_now();
    }
}

pub fn timespec(memory: &AddressSpace, address: u64) -> Result<Option<u64>, i64> {
    if address == 0 {
        return Ok(None);
    }
    Ok(Some(abi::decode_timespec(memory.read(address, 16)?)?))
}

pub fn timeval(memory: &AddressSpace, address: u64) -> Result<Option<u64>, i64> {
    if address == 0 {
        return Ok(None);
    }
    let bytes = memory.read(address, 16)?;
    let seconds = abi::read_u64(bytes, 0)?;
    let micros = abi::read_u64(bytes, 8)?;
    if micros >= 1_000_000 {
        return Err(EINVAL);
    }
    Ok(Some(
        seconds
            .checked_mul(1_000_000_000)
            .and_then(|value| value.checked_add(micros * 1000))
            .ok_or(EINVAL)?,
    ))
}

pub fn milliseconds(value: u64) -> Result<Option<u64>, i64> {
    let signed = value as i32;
    if signed < 0 {
        Ok(None)
    } else {
        Ok(Some(u64::try_from(signed).unwrap() * 1_000_000))
    }
}

fn read_set(memory: &AddressSpace, address: u64, bytes: usize) -> Result<Vec<u8>, i64> {
    if address == 0 {
        Ok(vec![0; bytes])
    } else {
        Ok(memory.read(address, bytes)?.to_vec())
    }
}

fn write_set(memory: &mut AddressSpace, address: u64, bytes: &[u8]) -> Result<(), i64> {
    if address != 0 {
        memory.write(address, bytes)?;
    }
    Ok(())
}

fn is_set(bytes: &[u8], fd: usize) -> bool {
    bytes
        .get(fd / 8)
        .is_some_and(|byte| byte & (1 << (fd % 8)) != 0)
}

fn set(bytes: &mut [u8], fd: usize) {
    bytes[fd / 8] |= 1 << (fd % 8);
}

fn deadline(timeout_nanos: Option<u64>) -> Option<u64> {
    timeout_nanos.map(|nanos| {
        bexos_userspace::syscall::ticks().saturating_add(
            (nanos as u128 * bexos_userspace::syscall::frequency() as u128 / 1_000_000_000) as u64,
        )
    })
}

fn expired(deadline: Option<u64>) -> bool {
    deadline.is_some_and(|deadline| bexos_userspace::syscall::ticks() >= deadline)
}
