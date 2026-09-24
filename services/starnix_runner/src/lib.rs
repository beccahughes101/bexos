mod migration;

use bexos_starnix_abi::Launch;
use bexos_userspace::{Channel, Memory, Startup, live_migration::Source};
use bexos_zircon::{AsHandleRef, RestrictedState, Vmar, VmarFlags, Vmo};
use starnix_kernel::{Architecture, EBADF, EFAULT, ENOSYS, Syscall, Task, error};
use std::{boxed::Box, string::String, sync::Arc, vec::Vec};

#[derive(Debug, Eq, PartialEq)]
pub enum Error {
    Prelude,
    Startup,
    Image,
    Mapping,
    Restricted,
}

struct PayloadMapping {
    handle: u64,
    address: u64,
    size: u64,
}

impl Drop for PayloadMapping {
    fn drop(&mut self) {
        let _ = Memory::unmap(self.address, self.size);
        let _ = Memory::close(self.handle);
    }
}

fn copy_payload(handle: u64, len: u64) -> Result<Arc<[u8]>, Error> {
    let rounded = match bexos_boot::page_round(len) {
        Some(rounded) => rounded,
        None => {
            let _ = Memory::close(handle);
            return Err(Error::Image);
        }
    };
    let address = match Memory::map(handle, rounded, 2) {
        Ok(address) => address,
        Err(_) => {
            let _ = Memory::close(handle);
            return Err(Error::Mapping);
        }
    };
    let mapping = PayloadMapping {
        handle,
        address,
        size: rounded,
    };
    let bytes = unsafe {
        std::slice::from_raw_parts(
            address as *const u8,
            usize::try_from(len).map_err(|_| Error::Image)?,
        )
    };
    let owned: Arc<[u8]> = bytes.into();
    drop(mapping);
    Ok(owned)
}

struct GuestMapping {
    vmo: Vmo,
    address: u64,
    size: u64,
    readable: bool,
    rights: u32,
}

impl Drop for GuestMapping {
    fn drop(&mut self) {
        let _ = Vmar::root_self().unmap(self.address, self.size);
    }
}

struct Runtime {
    restricted: RestrictedState,
    mappings: Vec<GuestMapping>,
    task: Task,
    migration: migration::Snapshot,
    source: Option<Source>,
}

impl Runtime {
    fn readable(&self, address: u64, length: u64) -> Option<&[u8]> {
        let end = address.checked_add(length)?;
        let mapping = self.mappings.iter().find(|mapping| {
            mapping.readable
                && address >= mapping.address
                && end <= mapping.address.saturating_add(mapping.size)
        })?;
        let _keep_alive = mapping.vmo.as_handle_ref();
        Some(unsafe {
            std::slice::from_raw_parts(address as *const u8, usize::try_from(length).ok()?)
        })
    }
}

fn architecture() -> Architecture {
    if cfg!(bexos_arch_x86_64) {
        Architecture::X86_64
    } else {
        Architecture::Aarch64
    }
}

fn flags(rights: u32) -> VmarFlags {
    let mut flags = VmarFlags::SPECIFIC.0;
    if rights & 2 != 0 {
        flags |= VmarFlags::PERM_READ.0;
    }
    if rights & 4 != 0 {
        flags |= VmarFlags::PERM_WRITE.0;
    }
    if rights & 8 != 0 {
        flags |= VmarFlags::PERM_EXECUTE.0;
    }
    VmarFlags(flags)
}

fn map_bytes(address: u64, bytes: &[u8], size: u64, rights: u32) -> Result<GuestMapping, Error> {
    let vmo = Vmo::create(size).map_err(|_| Error::Mapping)?;
    let writable =
        VmarFlags(VmarFlags::SPECIFIC.0 | VmarFlags::PERM_READ.0 | VmarFlags::PERM_WRITE.0);
    let staging = Vmar::root_self()
        .map(address, &vmo, 0, size, writable)
        .map_err(|_| Error::Mapping)?;
    Memory::commit_range(staging.address(), size).map_err(|_| Error::Mapping)?;
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), staging.address() as *mut u8, bytes.len())
    };
    drop(staging);
    let mapped = Vmar::root_self()
        .map(address, &vmo, 0, size, flags(rights))
        .map_err(|_| Error::Mapping)?;
    let address = mapped.address();
    core::mem::forget(mapped);
    Ok(GuestMapping {
        vmo,
        address,
        size,
        readable: rights & 2 != 0,
        rights,
    })
}

fn map_zero(address: u64, size: u64, rights: u32) -> Result<GuestMapping, Error> {
    map_bytes(address, &[], size, rights)
}

unsafe fn initialize_state(state: u64, pc: u64, sp: u64) {
    #[cfg(all(bexos_guest, target_arch = "x86_64"))]
    unsafe {
        let mut registers = bexos_userspace::restricted::X86_64StateV1::zeroed();
        registers.rip = pc;
        registers.rsp = sp;
        registers.rflags = 0x202;
        (state as *mut bexos_userspace::restricted::X86_64StateV1).write(registers);
    }
    #[cfg(all(bexos_guest, target_arch = "aarch64"))]
    unsafe {
        let mut registers = bexos_userspace::restricted::Aarch64StateV1::zeroed();
        registers.pc = pc;
        registers.sp = sp;
        (state as *mut bexos_userspace::restricted::Aarch64StateV1).write(registers);
    }
    #[cfg(not(bexos_guest))]
    let _ = (state, pc, sp);
}

fn register_size() -> usize {
    #[cfg(all(bexos_guest, target_arch = "x86_64"))]
    {
        core::mem::size_of::<bexos_userspace::restricted::X86_64StateV1>()
    }
    #[cfg(all(bexos_guest, target_arch = "aarch64"))]
    {
        core::mem::size_of::<bexos_userspace::restricted::Aarch64StateV1>()
    }
    #[cfg(not(bexos_guest))]
    {
        0
    }
}

fn architecture_id() -> u64 {
    match architecture() {
        Architecture::Aarch64 => 1,
        Architecture::X86_64 => 2,
    }
}

fn capture_registers(restricted: &RestrictedState) -> &[u8] {
    unsafe { std::slice::from_raw_parts(restricted.address() as *const u8, register_size()) }
}

fn restore_registers(restricted: &RestrictedState, registers: &[u8]) -> Result<(), Error> {
    if registers.len() != register_size() {
        return Err(Error::Restricted);
    }
    unsafe {
        core::ptr::copy_nonoverlapping(
            registers.as_ptr(),
            restricted.address() as *mut u8,
            registers.len(),
        );
    }
    Ok(())
}

fn poll_migration(runtime: &mut Runtime) {
    let Some(mut source) = runtime.source.take() else {
        return;
    };
    if runtime
        .migration
        .capture_registers(capture_registers(&runtime.restricted))
        .is_err()
        || source.poll(&runtime.migration).is_err()
    {
        bexos_userspace::log("starnix_runner: migration aborted at restricted safe point\n");
    }
    runtime.source = Some(source);
}

unsafe extern "C" fn vector(context: u64, reason: bexos_userspace::restricted::Reason) -> ! {
    let runtime = unsafe { &mut *(context as *mut Runtime) };
    if matches!(
        reason,
        bexos_userspace::restricted::Reason::Syscall | bexos_userspace::restricted::Reason::Kick
    ) {
        poll_migration(runtime);
    }
    if reason == bexos_userspace::restricted::Reason::Kick {
        unsafe { runtime.restricted.enter(vector, context) }.unwrap();
        unreachable!()
    }
    if reason != bexos_userspace::restricted::Reason::Syscall {
        let _ = runtime.restricted.unbind();
        bexos_userspace::log("starnix_runner: Linux exception exit\n");
        unsafe { drop(Box::from_raw(context as *mut Runtime)) };
        bexos_userspace::syscall::exit_with_status(126)
    }

    #[cfg(all(bexos_guest, target_arch = "x86_64"))]
    let (number, args) = unsafe {
        let state =
            &mut *(runtime.restricted.address() as *mut bexos_userspace::restricted::X86_64StateV1);
        (
            state.rax,
            [
                state.rdi, state.rsi, state.rdx, state.r10, state.r8, state.r9,
            ],
        )
    };
    #[cfg(all(bexos_guest, target_arch = "aarch64"))]
    let (number, args) = unsafe {
        let state = &mut *(runtime.restricted.address()
            as *mut bexos_userspace::restricted::Aarch64StateV1);
        (
            state.x[8],
            [
                state.x[0], state.x[1], state.x[2], state.x[3], state.x[4], state.x[5],
            ],
        )
    };
    #[cfg(not(bexos_guest))]
    let (number, args) = (u64::MAX, [0; 6]);

    let result = match Syscall::decode(architecture(), number) {
        Syscall::Write => {
            if !matches!(args[0], 1 | 2) {
                error(EBADF)
            } else if let Some(bytes) = runtime.readable(args[1], args[2]) {
                bexos_userspace::log(&String::from_utf8_lossy(bytes));
                args[2]
            } else {
                error(EFAULT)
            }
        }
        Syscall::Exit | Syscall::ExitGroup => {
            runtime.task.exit(args[0]);
            let status = runtime.task.exit_code().unwrap_or(126);
            let _ = runtime.restricted.unbind();
            unsafe { drop(Box::from_raw(context as *mut Runtime)) };
            bexos_userspace::log(&format!("starnix_runner: guest exited {status}\n"));
            bexos_userspace::syscall::exit_with_status(status)
        }
        Syscall::Unsupported(_) => error(ENOSYS),
    };
    #[cfg(not(bexos_guest))]
    let _ = result;

    #[cfg(all(bexos_guest, target_arch = "x86_64"))]
    unsafe {
        (*(runtime.restricted.address() as *mut bexos_userspace::restricted::X86_64StateV1)).rax =
            result;
    }
    #[cfg(all(bexos_guest, target_arch = "aarch64"))]
    unsafe {
        (*(runtime.restricted.address() as *mut bexos_userspace::restricted::Aarch64StateV1)).x
            [0] = result;
    }
    unsafe { runtime.restricted.enter(vector, context) }.unwrap();
    unreachable!()
}

pub fn run(channel: Channel) -> Result<u8, Error> {
    let prelude = channel.recv_blocking().map_err(|_| Error::Prelude)?;
    let launch = Launch::decode(&prelude.bytes).map_err(|_| Error::Prelude)?;
    if prelude.handles.len() != 1 {
        for handle in prelude.handles {
            let _ = Memory::close(handle);
        }
        return Err(Error::Prelude);
    }
    let image_bytes = copy_payload(prelude.handles[0], launch.image_len)?;
    let startup = Startup::receive(channel).map_err(|_| Error::Startup)?;
    bexos_libc::install_startup(&startup);
    if launch.service != launch.migratable
        || (launch.migratable && startup.migration.is_none() && !startup.migration_target)
    {
        return Err(Error::Startup);
    }
    let arguments = if launch.options.arguments.is_empty() {
        vec![launch.options.path.clone()]
    } else {
        launch.options.arguments.clone()
    };
    let environment: Vec<_> = launch
        .options
        .environment
        .iter()
        .map(|entry| (entry.name.clone(), entry.value.clone()))
        .collect();
    let image =
        starnix_kernel::prepare_image(&image_bytes, architecture(), &arguments, &environment)
            .map_err(|_| Error::Image)?;
    let (mappings, migration, candidate) = if startup.migration_target {
        let snapshot = bexos_userspace::live_migration::receive_with_state(
            channel,
            startup.migration_generation,
            migration::Snapshot::candidate(architecture_id(), &launch.options)
                .map_err(|_| Error::Startup)?,
        )
        .map_err(|_| Error::Startup)?;
        let mappings = snapshot
            .mappings
            .iter()
            .map(|mapping| GuestMapping {
                vmo: unsafe { Vmo::from_raw(mapping.handle) },
                address: mapping.address,
                size: mapping.size,
                readable: mapping.rights & 2 != 0,
                rights: mapping.rights,
            })
            .collect();
        (mappings, snapshot, true)
    } else {
        let mut mappings = Vec::new();
        for segment in &image.plan.segments {
            let size = bexos_boot::page_round(segment.file_size).ok_or(Error::Image)?;
            if size != 0 {
                let start = usize::try_from(segment.file_offset).map_err(|_| Error::Image)?;
                let end = start
                    .checked_add(usize::try_from(segment.file_size).map_err(|_| Error::Image)?)
                    .ok_or(Error::Image)?;
                mappings.push(map_bytes(
                    segment.vaddr,
                    image_bytes.get(start..end).ok_or(Error::Image)?,
                    size,
                    segment.rights,
                )?);
            }
            if let Some(zero) = segment.zero_fill {
                mappings.push(map_zero(zero.vaddr, zero.size_bytes, segment.rights)?);
            }
        }
        mappings.push(map_bytes(
            starnix_kernel::GUEST_STACK_TOP - starnix_kernel::GUEST_STACK_SIZE,
            &image.stack,
            starnix_kernel::GUEST_STACK_SIZE,
            2 | 4,
        )?);
        let descriptors = mappings
            .iter()
            .map(|mapping| migration::Mapping {
                handle: mapping.vmo.as_handle_ref().raw_handle(),
                address: mapping.address,
                size: mapping.size,
                rights: mapping.rights,
            })
            .collect();
        let restricted = RestrictedState::create().map_err(|_| Error::Restricted)?;
        unsafe {
            initialize_state(
                restricted.address(),
                image.plan.entry_vaddr,
                image.stack_pointer,
            )
        };
        let snapshot = migration::Snapshot::source(
            architecture_id(),
            &launch.options,
            capture_registers(&restricted),
            descriptors,
            startup.migration,
        )
        .map_err(|_| Error::Startup)?;
        drop(restricted);
        (mappings, snapshot, false)
    };
    let restricted = RestrictedState::create().map_err(|_| Error::Restricted)?;
    if candidate {
        restore_registers(&restricted, migration.registers())?;
    } else {
        unsafe {
            initialize_state(
                restricted.address(),
                image.plan.entry_vaddr,
                image.stack_pointer,
            )
        };
    }
    restricted.bind().map_err(|_| Error::Restricted)?;
    if !candidate {
        Startup::ready(channel).map_err(|_| Error::Startup)?;
    }
    let source = Some(Source::new(migration.migration()));
    let runtime = Box::new(Runtime {
        restricted,
        mappings,
        task: Task::new(),
        migration,
        source,
    });
    let context = Box::into_raw(runtime) as u64;
    let runtime = unsafe { &*(context as *const Runtime) };
    unsafe { runtime.restricted.enter(vector, context) }.map_err(|_| Error::Restricted)?;
    unreachable!()
}
