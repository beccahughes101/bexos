use crate::{
    abi,
    memory::{AddressSpace, MappingKind},
    signals::{AltStack, SA_RESTORER, SignalAction, SignalState},
    vfs::{AT_FDCWD, Vfs},
};
use bexos_migration::{
    Error as MigrationError,
    codec::{Decoder, Encoder},
};
use bexos_userspace::Memory;
use bexos_zircon::{Clock, ClockId, Futex as ZirconFutex};
use starnix_kernel::{Architecture, EBADF, EINTR, EINVAL, ENOMEM, ENOSYS, ENOTSUP, Syscall, error};

pub enum Outcome {
    Return(u64),
    Exit(i32),
    Sigreturn,
    SetArchBase { fs: bool, value: u64 },
}

pub struct Dispatcher {
    pub architecture: Architecture,
    random: u64,
    directory_offsets: [usize; 256],
    terminal_mode: u32,
    window: [u16; 2],
    fs_base: u64,
    gs_base: u64,
}

impl Dispatcher {
    pub fn new(architecture: Architecture) -> Self {
        Self {
            architecture,
            random: bexos_userspace::syscall::ticks() ^ 0x9e37_79b9_7f4a_7c15,
            directory_offsets: [0; 256],
            terminal_mode: 0x0000_0002 | 0x0000_0008 | 0x0000_0100 | 0x0000_0800,
            window: [24, 80],
            fs_base: 0,
            gs_base: 0,
        }
    }

    pub fn checkpoint(&self) -> Vec<u8> {
        let mut out = Encoder::new();
        out.word(1);
        out.word(self.random);
        out.word(u64::from(self.terminal_mode));
        out.word(u64::from(self.window[0]));
        out.word(u64::from(self.window[1]));
        out.word(self.fs_base);
        out.word(self.gs_base);
        for offset in self.directory_offsets {
            out.word(offset as u64);
        }
        out.finish()
    }

    pub fn restore(architecture: Architecture, bytes: &[u8]) -> Result<Self, MigrationError> {
        let mut input = Decoder::new(bytes);
        if input.word()? != 1 {
            return Err(MigrationError::UnsupportedVersion);
        }
        let random = input.word()?;
        let terminal_mode =
            u32::try_from(input.word()?).map_err(|_| MigrationError::InvalidData)?;
        let rows = u16::try_from(input.word()?).map_err(|_| MigrationError::InvalidData)?;
        let columns = u16::try_from(input.word()?).map_err(|_| MigrationError::InvalidData)?;
        let fs_base = input.word()?;
        let gs_base = input.word()?;
        let mut directory_offsets = [0; 256];
        for offset in &mut directory_offsets {
            *offset = usize::try_from(input.word()?).map_err(|_| MigrationError::InvalidData)?;
        }
        input.finish()?;
        Ok(Self {
            architecture,
            random,
            directory_offsets,
            terminal_mode,
            window: [rows, columns],
            fs_base,
            gs_base,
        })
    }

    pub fn call(
        &mut self,
        syscall: Syscall,
        args: [u64; 6],
        memory: &mut AddressSpace,
        vfs: &mut Vfs,
        signals: &mut SignalState,
    ) -> Outcome {
        match self.call_inner(syscall, args, memory, vfs, signals) {
            Ok(outcome) => outcome,
            Err(errno) => Outcome::Return(error(errno)),
        }
    }

    fn call_inner(
        &mut self,
        syscall: Syscall,
        a: [u64; 6],
        memory: &mut AddressSpace,
        vfs: &mut Vfs,
        signals: &mut SignalState,
    ) -> Result<Outcome, i64> {
        use Syscall::*;
        let value = match syscall {
            Read => {
                let bytes = vfs.read(a[0] as i32, bounded(a[2])?)?;
                memory.write(a[1], &bytes)?;
                bytes.len() as u64
            }
            Write => {
                let bytes = memory.read(a[1], bounded(a[2])?)?;
                vfs.write(a[0] as i32, bytes)? as u64
            }
            Readv | Writev => self.vectored(
                matches!(syscall, Readv),
                a[0] as i32,
                a[1],
                a[2],
                memory,
                vfs,
            )?,
            Pread64 => {
                let bytes = vfs.read_at(a[0] as i32, a[3], bounded(a[2])?)?;
                memory.write(a[1], &bytes)?;
                bytes.len() as u64
            }
            Pwrite64 => {
                let bytes = memory.read(a[1], bounded(a[2])?)?;
                vfs.write_at(a[0] as i32, a[3], bytes)? as u64
            }
            Open => self.open(AT_FDCWD, a[0], a[1], memory, vfs)? as u64,
            OpenAt => self.open(a[0] as i32, a[1], a[2], memory, vfs)? as u64,
            OpenAt2 => {
                if a[3] < 24 {
                    return Err(EINVAL);
                }
                let how = memory.read(a[2], 24)?;
                let flags = abi::read_u64(how, 0)?;
                let resolve = abi::read_u64(how, 16)?;
                if resolve & !0x3f != 0 {
                    return Err(EINVAL);
                }
                self.open(a[0] as i32, a[1], flags, memory, vfs)? as u64
            }
            Close => {
                vfs.close(a[0] as i32)?;
                0
            }
            CloseRange => {
                vfs.close_range(a[0] as u32, a[1] as u32)?;
                0
            }
            Dup => vfs.duplicate(a[0] as i32, 0, None)? as u64,
            Dup2 | Dup3 => vfs.duplicate(a[0] as i32, 0, Some(a[1] as usize))? as u64,
            Fcntl => match a[1] {
                0 => vfs.duplicate(a[0] as i32, a[2] as usize, None)? as u64,
                1 => 0,
                2 => 0,
                3 => u64::from(vfs.flags(a[0] as i32)?),
                4 => {
                    vfs.set_flags(a[0] as i32, a[2] as u32)?;
                    0
                }
                _ => return Err(ENOTSUP),
            },
            Fstat => {
                self.write_stat(vfs.stat_fd(a[0] as i32)?, a[1], memory)?;
                0
            }
            Stat | Lstat => {
                let path = memory.cstring(a[0], 4096)?;
                self.write_stat(vfs.stat_path(AT_FDCWD, &path)?, a[1], memory)?;
                0
            }
            NewFstatAt => {
                let path = memory.cstring(a[1], 4096)?;
                self.write_stat(vfs.stat_path(a[0] as i32, &path)?, a[2], memory)?;
                0
            }
            Statx => {
                let path = memory.cstring(a[1], 4096)?;
                let stat = vfs.stat_path(a[0] as i32, &path)?;
                let mut out = vec![0; 256];
                abi::put_u32(&mut out, 0, 0x7ff);
                abi::put_u32(&mut out, 4, 4096);
                abi::put_u64(&mut out, 16, stat.links);
                abi::put_u32(&mut out, 28, stat.mode);
                abi::put_u64(&mut out, 40, stat.inode);
                abi::put_u64(&mut out, 48, stat.size);
                abi::put_u64(&mut out, 56, stat.blocks);
                memory.write(a[4], &out)?;
                0
            }
            Getdents64 => {
                let fd = a[0] as usize;
                if fd >= self.directory_offsets.len() {
                    return Err(EBADF);
                }
                let entries = vfs.entries(fd as i32)?;
                let (bytes, encoded) = abi::encode_dirents(
                    &entries[self.directory_offsets[fd].min(entries.len())..],
                    bounded(a[2])?,
                    self.directory_offsets[fd],
                );
                self.directory_offsets[fd] = self.directory_offsets[fd].saturating_add(encoded);
                memory.write(a[1], &bytes)?;
                bytes.len() as u64
            }
            Lseek => vfs.seek(a[0] as i32, a[1] as i64, a[2] as u8)?,
            Access | FaccessAt => {
                let (dirfd, pointer) = if matches!(syscall, Access) {
                    (AT_FDCWD, a[0])
                } else {
                    (a[0] as i32, a[1])
                };
                let path = memory.cstring(pointer, 4096)?;
                vfs.stat_path(dirfd, &path)?;
                0
            }
            Getcwd => {
                let bytes = vfs.cwd().as_bytes();
                if a[1] as usize <= bytes.len() {
                    return Err(starnix_kernel::ERANGE);
                }
                memory.write(a[0], bytes)?;
                memory.write(a[0] + bytes.len() as u64, &[0])?;
                a[0]
            }
            Chdir => {
                let path = memory.cstring(a[0], 4096)?;
                vfs.chdir(&path)?;
                0
            }
            Mkdir => {
                let path = memory.cstring(a[0], 4096)?;
                vfs.mkdir(AT_FDCWD, &path)?;
                0
            }
            MkdirAt => {
                let path = memory.cstring(a[1], 4096)?;
                vfs.mkdir(a[0] as i32, &path)?;
                0
            }
            Unlink => {
                let path = memory.cstring(a[0], 4096)?;
                vfs.unlink(AT_FDCWD, &path, false)?;
                0
            }
            Rmdir => {
                let path = memory.cstring(a[0], 4096)?;
                vfs.unlink(AT_FDCWD, &path, true)?;
                0
            }
            UnlinkAt => {
                let path = memory.cstring(a[1], 4096)?;
                vfs.unlink(a[0] as i32, &path, a[2] & 0x200 != 0)?;
                0
            }
            Fsync | Fdatasync => {
                vfs.sync(a[0] as i32)?;
                0
            }
            Truncate => {
                let path = memory.cstring(a[0], 4096)?;
                vfs.truncate_path(&path, a[1])?;
                0
            }
            Ftruncate => {
                vfs.truncate_fd(a[0] as i32, a[1])?;
                0
            }
            Rename | RenameAt | Link | LinkAt | Symlink | SymlinkAt | Readlink | ReadlinkAt => {
                return Err(ENOTSUP);
            }
            Mmap => {
                let flags = a[3];
                if flags & 3 == 0 || flags & 3 == 3 {
                    return Err(EINVAL);
                }
                let anonymous = flags & 0x20 != 0;
                let fixed = flags & (0x10 | 0x10_0000) != 0;
                if flags & 0x10 != 0 && flags & 0x10_0000 == 0 {
                    let _ = memory.unmap(a[0], a[1]);
                }
                if anonymous {
                    memory.map_anonymous(a[0], a[1], a[2], MappingKind::Anonymous, fixed)?
                } else {
                    let (handle, size) = vfs.backing(a[4] as i32)?;
                    let result = memory.map_file_copy(
                        a[0],
                        a[1],
                        a[2],
                        a[4] as i32,
                        a[5],
                        handle,
                        size,
                        flags & 1 != 0,
                        fixed,
                    );
                    let _ = Memory::close(handle);
                    result?
                }
            }
            Munmap => {
                self.flush(a[0], a[1], memory, vfs)?;
                memory.unmap(a[0], a[1])?;
                0
            }
            Mprotect => {
                memory.protect(a[0], a[1], a[2])?;
                0
            }
            Msync => {
                self.flush(a[0], a[1], memory, vfs)?;
                0
            }
            Madvise => {
                if a[2] > 25 {
                    return Err(EINVAL);
                }
                0
            }
            Mremap => return Err(ENOTSUP),
            Brk => memory.brk(a[0]),
            Futex => match a[1] & 0x7f {
                0 => {
                    let timeout = if a[3] == 0 {
                        -1
                    } else {
                        abi::decode_timespec(memory.read(a[3], 16)?)? as i64
                    };
                    ZirconFutex::wait(a[0], a[2] as u32, timeout).map_err(|_| EINTR)?;
                    0
                }
                1 => u64::from(ZirconFutex::wake(a[0], a[2] as u32).map_err(|_| EINVAL)?),
                _ => return Err(ENOTSUP),
            },
            RtSigaction => {
                let signal = a[0] as u32;
                if a[3] != 8 {
                    return Err(EINVAL);
                }
                let old = signals.action(signal)?;
                if a[2] != 0 {
                    memory.write(a[2], &encode_action(old))?;
                }
                if a[1] != 0 {
                    let action = decode_action(memory.read(a[1], 32)?)?;
                    if action.flags & SA_RESTORER != 0 && action.restorer == 0 {
                        return Err(EINVAL);
                    }
                    signals.set_action(signal, action)?;
                }
                0
            }
            RtSigprocmask => {
                if a[3] != 8 {
                    return Err(EINVAL);
                }
                let old = signals.mask();
                if a[2] != 0 {
                    memory.write(a[2], &old.to_ne_bytes())?;
                }
                if a[1] != 0 {
                    signals.update_mask(a[0] as u32, abi::read_u64(memory.read(a[1], 8)?, 0)?)?;
                }
                0
            }
            RtSigreturn => return Ok(Outcome::Sigreturn),
            RtSigsuspend => {
                if a[1] != 8 {
                    return Err(EINVAL);
                }
                signals.suspend(abi::read_u64(memory.read(a[0], 8)?, 0)?);
                return Err(EINTR);
            }
            Sigaltstack => {
                let old = signals.alt_stack();
                if a[1] != 0 {
                    memory.write(a[1], &encode_altstack(old))?;
                }
                if a[0] != 0 {
                    signals.set_alt_stack(decode_altstack(memory.read(a[0], 24)?)?)?;
                }
                0
            }
            Kill => {
                signals.validate_target(a[0] as i64, 1)?;
                if a[1] != 0 {
                    signals.queue(a[1] as u32)?;
                }
                0
            }
            Tgkill => {
                signals.validate_target(a[0] as i64, a[1] as i64)?;
                if a[2] != 0 {
                    signals.queue(a[2] as u32)?;
                }
                0
            }
            ClockGettime => {
                let id = match a[0] as i32 {
                    0 => ClockId::Realtime,
                    1 => ClockId::Monotonic,
                    7 => ClockId::Boot,
                    _ => return Err(EINVAL),
                };
                let nanos = Clock::get(id).map_err(|_| EINVAL)?;
                memory.write(a[1], &abi::encode_timespec(nanos))?;
                0
            }
            Gettimeofday => {
                let nanos = Clock::get(ClockId::Realtime).map_err(|_| EINVAL)?;
                let mut out = [0; 16];
                abi::put_u64(&mut out, 0, nanos / 1_000_000_000);
                abi::put_u64(&mut out, 8, (nanos % 1_000_000_000) / 1000);
                memory.write(a[0], &out)?;
                0
            }
            Nanosleep => {
                let duration = abi::decode_timespec(memory.read(a[0], 16)?)?;
                let start = bexos_userspace::syscall::ticks();
                let target = start.saturating_add(
                    (duration as u128 * bexos_userspace::syscall::frequency() as u128
                        / 1_000_000_000) as u64,
                );
                while bexos_userspace::syscall::ticks() < target {
                    bexos_userspace::yield_now();
                }
                0
            }
            SchedYield => {
                bexos_userspace::yield_now();
                0
            }
            Getrandom => {
                let length = bounded(a[1])?;
                let mut bytes = vec![0; length];
                for chunk in bytes.chunks_mut(8) {
                    self.random ^= self.random << 13;
                    self.random ^= self.random >> 7;
                    self.random ^= self.random << 17;
                    let value = self.random.to_ne_bytes();
                    let count = chunk.len();
                    chunk.copy_from_slice(&value[..count]);
                }
                memory.write(a[0], &bytes)?;
                length as u64
            }
            Getpid | Gettid => 1,
            Getuid | Geteuid | Getgid | Getegid => 0,
            Uname => {
                memory.write(a[0], &abi::encode_uts())?;
                0
            }
            Prlimit64 => {
                if a[0] != 0 && a[0] != 1 {
                    return Err(EINVAL);
                }
                if a[3] != 0 {
                    let infinity = u64::MAX;
                    let mut out = [0; 16];
                    abi::put_u64(&mut out, 0, if a[1] == 7 { 256 } else { infinity });
                    abi::put_u64(&mut out, 8, if a[1] == 7 { 256 } else { infinity });
                    memory.write(a[3], &out)?;
                }
                0
            }
            SetTidAddress => 1,
            SetRobustList | Rseq => 0,
            ArchPrctl => {
                if self.architecture != Architecture::X86_64 {
                    return Err(ENOSYS);
                }
                match a[0] {
                    0x1001 => {
                        self.gs_base = a[1];
                        return Ok(Outcome::SetArchBase {
                            fs: false,
                            value: a[1],
                        });
                    }
                    0x1002 => {
                        self.fs_base = a[1];
                        return Ok(Outcome::SetArchBase {
                            fs: true,
                            value: a[1],
                        });
                    }
                    0x1003 => {
                        memory.write(a[1], &self.fs_base.to_ne_bytes())?;
                        0
                    }
                    0x1004 => {
                        memory.write(a[1], &self.gs_base.to_ne_bytes())?;
                        0
                    }
                    _ => return Err(EINVAL),
                }
            }
            Ioctl => self.ioctl(a, memory)?,
            Pipe | Pipe2 | Poll | Ppoll | Select | Pselect6 => return Err(ENOTSUP),
            Exit | ExitGroup => return Ok(Outcome::Exit(a[0] as u8 as i32)),
            Unsupported(_) => return Err(ENOSYS),
        };
        Ok(Outcome::Return(value))
    }

    fn open(
        &self,
        dirfd: i32,
        pointer: u64,
        flags: u64,
        memory: &AddressSpace,
        vfs: &mut Vfs,
    ) -> Result<i32, i64> {
        let path = memory.cstring(pointer, 4096)?;
        vfs.open(dirfd, &path, flags as u32)
    }

    fn write_stat(
        &self,
        stat: crate::vfs::Stat,
        address: u64,
        memory: &mut AddressSpace,
    ) -> Result<(), i64> {
        memory.write(address, &abi::encode_stat(self.architecture, &stat))
    }

    fn vectored(
        &self,
        read: bool,
        fd: i32,
        address: u64,
        count: u64,
        memory: &mut AddressSpace,
        vfs: &mut Vfs,
    ) -> Result<u64, i64> {
        if count > 64 {
            return Err(EINVAL);
        }
        let table = memory.read(address, count as usize * 16)?.to_vec();
        let mut total = 0u64;
        for index in 0..count as usize {
            let pointer = abi::read_u64(&table, index * 16)?;
            let length = bounded(abi::read_u64(&table, index * 16 + 8)?)?;
            let n = if read {
                let bytes = vfs.read(fd, length)?;
                memory.write(pointer, &bytes)?;
                bytes.len()
            } else {
                vfs.write(fd, memory.read(pointer, length)?)?
            };
            total = total.saturating_add(n as u64);
            if n != length {
                break;
            }
        }
        Ok(total)
    }

    fn flush(
        &self,
        address: u64,
        length: u64,
        memory: &AddressSpace,
        vfs: &mut Vfs,
    ) -> Result<(), i64> {
        for (fd, offset, bytes) in memory.shared_dirty_ranges(address, length) {
            vfs.write_at(fd, offset, bytes)?;
            vfs.sync(fd)?;
        }
        Ok(())
    }

    fn ioctl(&mut self, a: [u64; 6], memory: &mut AddressSpace) -> Result<u64, i64> {
        match a[1] {
            0x5413 => {
                let mut out = [0; 8];
                abi::put_u16(&mut out, 0, self.window[0]);
                abi::put_u16(&mut out, 2, self.window[1]);
                memory.write(a[2], &out)?;
                Ok(0)
            }
            0x5414 => {
                let bytes = memory.read(a[2], 8)?;
                self.window = [
                    u16::from_ne_bytes(bytes[0..2].try_into().unwrap()),
                    u16::from_ne_bytes(bytes[2..4].try_into().unwrap()),
                ];
                Ok(0)
            }
            0x5401 => {
                let mut out = [0; 36];
                abi::put_u32(&mut out, 12, self.terminal_mode);
                memory.write(a[2], &out)?;
                Ok(0)
            }
            0x5402 | 0x5403 | 0x5404 => {
                self.terminal_mode = abi::read_u32(memory.read(a[2], 36)?, 12)?;
                Ok(0)
            }
            0x5421 => Ok(0),
            _ => Err(ENOTSUP),
        }
    }
}

fn bounded(value: u64) -> Result<usize, i64> {
    let value = usize::try_from(value).map_err(|_| EINVAL)?;
    if value > 16 * 1024 * 1024 {
        Err(ENOMEM)
    } else {
        Ok(value)
    }
}

fn encode_action(action: SignalAction) -> [u8; 32] {
    let mut out = [0; 32];
    abi::put_u64(&mut out, 0, action.handler);
    abi::put_u64(&mut out, 8, action.flags);
    abi::put_u64(&mut out, 16, action.restorer);
    abi::put_u64(&mut out, 24, action.mask);
    out
}

fn decode_action(bytes: &[u8]) -> Result<SignalAction, i64> {
    Ok(SignalAction {
        handler: abi::read_u64(bytes, 0)?,
        flags: abi::read_u64(bytes, 8)?,
        restorer: abi::read_u64(bytes, 16)?,
        mask: abi::read_u64(bytes, 24)?,
    })
}

fn encode_altstack(stack: AltStack) -> [u8; 24] {
    let mut out = [0; 24];
    abi::put_u64(&mut out, 0, stack.address);
    abi::put_u32(&mut out, 8, stack.flags);
    abi::put_u64(&mut out, 16, stack.size);
    out
}

fn decode_altstack(bytes: &[u8]) -> Result<AltStack, i64> {
    Ok(AltStack {
        address: abi::read_u64(bytes, 0)?,
        flags: abi::read_u32(bytes, 8)?,
        size: abi::read_u64(bytes, 16)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signal_and_altstack_wire_layouts_round_trip() {
        let action = SignalAction {
            handler: 1,
            flags: 2,
            restorer: 3,
            mask: 4,
        };
        assert_eq!(decode_action(&encode_action(action)).unwrap(), action);
        let stack = AltStack {
            address: 5,
            size: 8192,
            flags: 0,
        };
        assert_eq!(decode_altstack(&encode_altstack(stack)).unwrap(), stack);
    }

    #[test]
    fn transfers_are_bounded() {
        assert_eq!(bounded(4096), Ok(4096));
        assert_eq!(bounded(17 * 1024 * 1024), Err(ENOMEM));
    }
}
