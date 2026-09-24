mod abi;
mod dispatch;
mod memory;
mod migration;
mod signals;
mod vfs;

use bexos_starnix_abi::{Control, Launch, NixRootSource};
use bexos_userspace::{Channel, Memory, Startup, live_migration::Source};
use bexos_zircon::{AsHandleRef, RestrictedState, Vmar, VmarFlags, Vmo};
use dispatch::{Dispatcher, Outcome};
use memory::{AddressSpace, Mapping, MappingKind};
#[cfg(bexos_guest)]
use signals::SA_ONSTACK;
use signals::SignalState;
use starnix_kernel::{Architecture, Syscall};
use std::{boxed::Box, sync::Arc, vec::Vec};
use vfs::{StdioKind, Vfs};

const SIGNAL_TRAMPOLINE: u64 =
    starnix_kernel::GUEST_STACK_TOP - starnix_kernel::GUEST_STACK_SIZE - 4096;

fn signal_trampoline() -> &'static [u8] {
    #[cfg(all(bexos_guest, target_arch = "x86_64"))]
    {
        // mov $SYS_rt_sigreturn,%rax; syscall
        &[0x48, 0xc7, 0xc0, 0x0f, 0, 0, 0, 0x0f, 0x05]
    }
    #[cfg(all(bexos_guest, target_arch = "aarch64"))]
    {
        // mov x8,#SYS_rt_sigreturn; svc #0
        &[0x68, 0x11, 0x80, 0xd2, 0x01, 0x00, 0x00, 0xd4]
    }
    #[cfg(not(bexos_guest))]
    {
        &[]
    }
}

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
    let rounded = bexos_boot::page_round(len).ok_or(Error::Image)?;
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

struct SavedSignalFrame {
    address: u64,
    length: usize,
    old_mask: u64,
}

struct Runtime {
    restricted: RestrictedState,
    memory: AddressSpace,
    vfs: Vfs,
    signals: SignalState,
    dispatcher: Dispatcher,
    signal_frames: Vec<SavedSignalFrame>,
    migration: migration::Snapshot,
    source: Option<Source>,
    control: Channel,
}

fn architecture() -> Architecture {
    Architecture::current()
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

fn map_bytes(
    address: u64,
    bytes: &[u8],
    size: u64,
    rights: u32,
    kind: MappingKind,
) -> Result<Mapping, Error> {
    let vmo = Arc::new(Vmo::create(size).map_err(|_| Error::Mapping)?);
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
    Ok(Mapping {
        vmo,
        address,
        size,
        vmo_offset: 0,
        rights,
        kind,
    })
}

fn map_zero(address: u64, size: u64, rights: u32, kind: MappingKind) -> Result<Mapping, Error> {
    map_bytes(address, &[], size, rights, kind)
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

fn migration_mappings(memory: &AddressSpace) -> Result<Vec<migration::Mapping>, Error> {
    let mut handles = Vec::new();
    let mut out = Vec::new();
    for mapping in memory.mappings() {
        let original = mapping.vmo.as_handle_ref().raw_handle();
        let repeated = handles.contains(&original);
        let handle = if repeated {
            let (_, rights) = Memory::object_info(original).map_err(|_| Error::Mapping)?;
            Memory::duplicate(original, rights).map_err(|_| Error::Mapping)?
        } else {
            handles.push(original);
            original
        };
        out.push(migration::Mapping {
            handle,
            address: mapping.address,
            size: mapping.size,
            offset: mapping.vmo_offset,
            rights: mapping.rights,
            owned: repeated,
        });
    }
    Ok(out)
}

fn poll_control(runtime: &mut Runtime) {
    loop {
        let message = match runtime.control.try_recv() {
            Ok(message) => message,
            Err(kernel_fidl::Status::ErrTimedOut) => return,
            Err(_) => return,
        };
        for handle in message.handles {
            let _ = Memory::close(handle);
        }
        if let Ok(Control::Signal(signal)) = Control::decode(&message.bytes) {
            let _ = runtime.signals.queue(signal);
        }
    }
}

fn poll_migration(runtime: &mut Runtime) {
    let Some(mut source) = runtime.source.take() else {
        return;
    };
    match source.has_pending() {
        Ok(false) => {
            if source.poll(&runtime.migration).is_err() {
                bexos_userspace::log("starnix_runner: migration deadline poll failed\n");
            }
            runtime.source = Some(source);
            return;
        }
        Ok(true) => {}
        Err(_) => {
            bexos_userspace::log("starnix_runner: migration control channel failed\n");
            runtime.source = Some(source);
            return;
        }
    }
    let mappings = migration_mappings(&runtime.memory);
    let runtime_state = runtime.vfs.snapshot().map_err(|_| ()).and_then(|vfs| {
        migration::RuntimeState {
            signals: runtime.signals.checkpoint(),
            dispatcher: runtime.dispatcher.checkpoint(),
            vfs,
            signal_frames: runtime
                .signal_frames
                .iter()
                .map(|frame| (frame.address, frame.length as u64, frame.old_mask))
                .collect(),
        }
        .encode()
        .map_err(|_| ())
    });
    if mappings
        .map(|mappings| runtime.migration.replace_mappings(mappings))
        .is_err()
        || runtime_state
            .and_then(|state| runtime.migration.capture_runtime(state).map_err(|_| ()))
            .is_err()
        || runtime
            .migration
            .capture_registers(capture_registers(&runtime.restricted))
            .is_err()
        || source.poll_pending(&runtime.migration).is_err()
    {
        bexos_userspace::log("starnix_runner: migration aborted at restricted safe point\n");
    }
    runtime.source = Some(source);
}

fn syscall_state(_runtime: &Runtime) -> (u64, [u64; 6]) {
    #[cfg(all(bexos_guest, target_arch = "x86_64"))]
    unsafe {
        let state =
            &*(_runtime.restricted.address() as *const bexos_userspace::restricted::X86_64StateV1);
        return (
            state.rax,
            [
                state.rdi, state.rsi, state.rdx, state.r10, state.r8, state.r9,
            ],
        );
    }
    #[cfg(all(bexos_guest, target_arch = "aarch64"))]
    unsafe {
        let state =
            &*(_runtime.restricted.address() as *const bexos_userspace::restricted::Aarch64StateV1);
        return (
            state.x[8],
            [
                state.x[0], state.x[1], state.x[2], state.x[3], state.x[4], state.x[5],
            ],
        );
    }
    #[cfg(not(bexos_guest))]
    {
        (u64::MAX, [0; 6])
    }
}

fn set_result(runtime: &mut Runtime, value: u64) {
    #[cfg(all(bexos_guest, target_arch = "x86_64"))]
    unsafe {
        (*(runtime.restricted.address() as *mut bexos_userspace::restricted::X86_64StateV1)).rax =
            value;
    }
    #[cfg(all(bexos_guest, target_arch = "aarch64"))]
    unsafe {
        (*(runtime.restricted.address() as *mut bexos_userspace::restricted::Aarch64StateV1)).x
            [0] = value;
    }
    #[cfg(not(bexos_guest))]
    let _ = (runtime, value);
}

fn set_arch_base(runtime: &mut Runtime, fs: bool, value: u64) {
    #[cfg(all(bexos_guest, target_arch = "x86_64"))]
    unsafe {
        let state =
            &mut *(runtime.restricted.address() as *mut bexos_userspace::restricted::X86_64StateV1);
        if fs {
            state.fs_base = value;
        } else {
            state.gs_base = value;
        }
        state.rax = 0;
    }
    #[cfg(not(all(bexos_guest, target_arch = "x86_64")))]
    let _ = (runtime, fs, value);
}

fn restore_signal(runtime: &mut Runtime) -> Result<(), ()> {
    let frame = runtime.signal_frames.pop().ok_or(())?;
    let registers = runtime
        .memory
        .read(frame.address, frame.length)
        .map_err(|_| ())?
        .to_vec();
    restore_registers(&runtime.restricted, &registers).map_err(|_| ())?;
    runtime.signals.restore_mask(frame.old_mask);
    Ok(())
}

#[allow(unreachable_code)]
fn deliver_signal(runtime: &mut Runtime) -> Option<i32> {
    let (signal, action, old_mask) = runtime.signals.next()?;
    if action.handler == 1 {
        runtime.signals.restore_mask(old_mask);
        return None;
    }
    if action.handler == 0 {
        if matches!(signal, 17 | 18 | 20 | 21 | 22 | 23 | 28) {
            runtime.signals.restore_mask(old_mask);
            return None;
        }
        return Some(128 + signal as i32);
    }
    let registers = capture_registers(&runtime.restricted).to_vec();
    let _restorer = if action.restorer == 0 {
        SIGNAL_TRAMPOLINE
    } else {
        action.restorer
    };

    #[cfg(all(bexos_guest, target_arch = "x86_64"))]
    let frame_address = unsafe {
        let state =
            &mut *(runtime.restricted.address() as *mut bexos_userspace::restricted::X86_64StateV1);
        let stack_top = if action.flags & SA_ONSTACK != 0 {
            let alternate = runtime.signals.alt_stack();
            if alternate.flags == 0 && alternate.size != 0 {
                alternate.address.saturating_add(alternate.size)
            } else {
                state.rsp
            }
        } else {
            state.rsp
        };
        let frame = stack_top
            .saturating_sub(registers.len() as u64)
            .saturating_sub(16)
            & !15;
        if runtime.memory.write(frame, &registers).is_err()
            || runtime
                .memory
                .write(frame.saturating_sub(8), &_restorer.to_ne_bytes())
                .is_err()
        {
            return Some(128 + signal as i32);
        }
        state.rsp = frame - 8;
        state.rip = action.handler;
        state.rdi = signal as u64;
        state.rsi = 0;
        state.rdx = frame;
        frame
    };

    #[cfg(all(bexos_guest, target_arch = "aarch64"))]
    let frame_address = unsafe {
        let state = &mut *(runtime.restricted.address()
            as *mut bexos_userspace::restricted::Aarch64StateV1);
        let stack_top = if action.flags & SA_ONSTACK != 0 {
            let alternate = runtime.signals.alt_stack();
            if alternate.flags == 0 && alternate.size != 0 {
                alternate.address.saturating_add(alternate.size)
            } else {
                state.sp
            }
        } else {
            state.sp
        };
        let frame = stack_top.saturating_sub(registers.len() as u64) & !15;
        if runtime.memory.write(frame, &registers).is_err() {
            return Some(128 + signal as i32);
        }
        state.sp = frame;
        state.pc = action.handler;
        state.x[0] = signal as u64;
        state.x[1] = 0;
        state.x[2] = frame;
        state.x[30] = _restorer;
        frame
    };

    #[cfg(not(bexos_guest))]
    let frame_address = {
        let _ = (signal, action, registers.as_slice());
        return Some(126);
    };

    runtime.signal_frames.push(SavedSignalFrame {
        address: frame_address,
        length: registers.len(),
        old_mask,
    });
    None
}

unsafe fn finish(context: u64, status: i32, message: &str) -> ! {
    let runtime = unsafe { &mut *(context as *mut Runtime) };
    let _ = runtime.restricted.unbind();
    bexos_userspace::log(message);
    unsafe { drop(Box::from_raw(context as *mut Runtime)) };
    bexos_userspace::syscall::exit_with_status(status)
}

unsafe extern "C" fn vector(context: u64, reason: bexos_userspace::restricted::Reason) -> ! {
    let runtime = unsafe { &mut *(context as *mut Runtime) };
    if matches!(
        reason,
        bexos_userspace::restricted::Reason::Syscall | bexos_userspace::restricted::Reason::Kick
    ) {
        poll_control(runtime);
        poll_migration(runtime);
    }
    if reason == bexos_userspace::restricted::Reason::Kick {
        if let Some(status) = deliver_signal(runtime) {
            unsafe {
                finish(
                    context,
                    status,
                    "starnix_runner: guest terminated by signal\n",
                )
            }
        }
        unsafe { runtime.restricted.enter(vector, context) }.unwrap();
        unreachable!()
    }
    if reason != bexos_userspace::restricted::Reason::Syscall {
        unsafe { finish(context, 126, "starnix_runner: Linux exception exit\n") }
    }

    let (number, args) = syscall_state(runtime);
    let outcome = runtime.dispatcher.call(
        Syscall::decode(architecture(), number),
        args,
        &mut runtime.memory,
        &mut runtime.vfs,
        &mut runtime.signals,
    );
    match outcome {
        Outcome::Return(value) => set_result(runtime, value),
        Outcome::Exit(status) => unsafe {
            finish(
                context,
                status,
                &format!("starnix_runner: guest exited {status}\n"),
            )
        },
        Outcome::Sigreturn => {
            if restore_signal(runtime).is_err() {
                unsafe { finish(context, 126, "starnix_runner: invalid signal return\n") }
            }
        }
        Outcome::SetArchBase { fs, value } => set_arch_base(runtime, fs, value),
    }
    if let Some(status) = deliver_signal(runtime) {
        unsafe {
            finish(
                context,
                status,
                "starnix_runner: guest terminated by signal\n",
            )
        }
    }
    unsafe { runtime.restricted.enter(vector, context) }.unwrap();
    unreachable!()
}

fn stdio(startup: &Startup, command: bool) -> Result<[StdioKind; 3], Error> {
    if !command {
        return Ok([StdioKind::Console; 3]);
    }
    if startup.resources.len() != 4 {
        return Err(Error::Startup);
    }
    Ok([
        StdioKind::Socket(startup.resources[0]),
        StdioKind::Socket(startup.resources[1]),
        StdioKind::Socket(startup.resources[2]),
    ])
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
    let command = bexos_userspace::command::from_startup(&startup).map_err(|_| Error::Startup)?;
    if launch.service != launch.migratable
        || (launch.migratable && startup.migration.is_none() && !startup.migration_target)
    {
        return Err(Error::Startup);
    }
    let arguments = command
        .as_ref()
        .map(|command| command.arguments.clone())
        .unwrap_or_else(|| {
            if launch.options.arguments.is_empty() {
                vec![launch.options.path.clone()]
            } else {
                launch.options.arguments.clone()
            }
        });
    let environment: Vec<_> = command
        .as_ref()
        .map(|command| command.environment.clone())
        .unwrap_or_else(|| {
            launch
                .options
                .environment
                .iter()
                .map(|entry| (entry.name.clone(), entry.value.clone()))
                .collect()
        });
    let image =
        starnix_kernel::prepare_image(&image_bytes, architecture(), &arguments, &environment)
            .map_err(|_| Error::Image)?;

    let namespace: Vec<_> = startup
        .namespace
        .iter()
        .map(|entry| (entry.path.clone(), entry.directory))
        .collect();
    let root = match launch.options.rootfs.source {
        NixRootSource::Package => "/pkg",
        NixRootSource::Data => "/data",
    };
    let mut vfs = Vfs::new(
        &namespace,
        root,
        &launch.options.rootfs.subpath,
        stdio(&startup, command.is_some())?,
        command.is_some(),
    )
    .map_err(|_| Error::Startup)?;
    for entry in &startup.namespace {
        let _ = Memory::close(entry.directory);
    }
    if command.is_some() {
        let _ = Memory::close(startup.resources[3]);
    } else {
        for handle in &startup.resources {
            let _ = Memory::close(*handle);
        }
    }
    let initial_runtime = migration::RuntimeState {
        signals: SignalState::default().checkpoint(),
        dispatcher: Dispatcher::new(architecture()).checkpoint(),
        vfs: vfs.snapshot().map_err(|_| Error::Startup)?,
        signal_frames: Vec::new(),
    }
    .encode()
    .map_err(|_| Error::Startup)?;

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
            .map(|mapping| Mapping {
                vmo: Arc::new(unsafe { Vmo::from_raw(mapping.handle) }),
                address: mapping.address,
                size: mapping.size,
                vmo_offset: mapping.offset,
                rights: mapping.rights,
                kind: MappingKind::Anonymous,
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
                    MappingKind::Image,
                )?);
            }
            if let Some(zero) = segment.zero_fill {
                mappings.push(map_zero(
                    zero.vaddr,
                    zero.size_bytes,
                    segment.rights,
                    MappingKind::Image,
                )?);
            }
        }
        mappings.push(map_bytes(
            starnix_kernel::GUEST_STACK_TOP - starnix_kernel::GUEST_STACK_SIZE,
            &image.stack,
            starnix_kernel::GUEST_STACK_SIZE,
            2 | 4,
            MappingKind::Stack,
        )?);
        mappings.push(map_bytes(
            SIGNAL_TRAMPOLINE,
            signal_trampoline(),
            4096,
            2 | 8,
            MappingKind::SignalTrampoline,
        )?);
        let restricted = RestrictedState::create().map_err(|_| Error::Restricted)?;
        unsafe {
            initialize_state(
                restricted.address(),
                image.plan.entry_vaddr,
                image.stack_pointer,
            )
        };
        let memory = AddressSpace::new(mappings);
        let snapshot = migration::Snapshot::source(
            architecture_id(),
            &launch.options,
            capture_registers(&restricted),
            initial_runtime,
            migration_mappings(&memory)?,
            startup.migration,
        )
        .map_err(|_| Error::Startup)?;
        drop(restricted);
        (memory.into_mappings(), snapshot, false)
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
    let runtime_state =
        migration::RuntimeState::decode(migration.runtime()).map_err(|_| Error::Startup)?;
    let signals = SignalState::restore(&runtime_state.signals).map_err(|_| Error::Startup)?;
    let dispatcher = Dispatcher::restore(architecture(), &runtime_state.dispatcher)
        .map_err(|_| Error::Startup)?;
    let signal_frames = runtime_state
        .signal_frames
        .into_iter()
        .map(|(address, length, old_mask)| {
            Ok(SavedSignalFrame {
                address,
                length: usize::try_from(length).map_err(|_| Error::Startup)?,
                old_mask,
            })
        })
        .collect::<Result<Vec<_>, Error>>()?;
    if candidate {
        vfs.restore(&runtime_state.vfs)
            .map_err(|_| Error::Startup)?;
    }
    restricted.bind().map_err(|_| Error::Restricted)?;
    if !candidate {
        Startup::ready(channel).map_err(|_| Error::Startup)?;
    }
    let source = Some(Source::new(migration.migration()));
    let runtime = Box::new(Runtime {
        restricted,
        memory: AddressSpace::new(mappings),
        vfs,
        signals,
        dispatcher,
        signal_frames,
        migration,
        source,
        control: channel,
    });
    let context = Box::into_raw(runtime) as u64;
    let runtime = unsafe { &*(context as *const Runtime) };
    unsafe { runtime.restricted.enter(vector, context) }.map_err(|_| Error::Restricted)?;
    unreachable!()
}
