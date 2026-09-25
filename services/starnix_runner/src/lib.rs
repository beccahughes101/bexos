mod abi;
mod dispatch;
mod events;
mod memory;
mod migration;
mod polling;
mod signals;
mod vfs;

use bexos_starnix_abi::{Control, Launch, NixRootSource};
use bexos_userspace::{Channel, Memory, Startup, live_migration::Source};
use bexos_zircon::{AsHandleRef, RestrictedState, Vmar, VmarFlags, Vmo};
use dispatch::{Dispatcher, FutexAtomicOperation, FutexComparison, Outcome};
use memory::{AddressSpace, Mapping, MappingKind, new_futex_id};
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

#[derive(Clone)]
struct SavedSignalFrame {
    address: u64,
    length: usize,
    old_mask: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RseqRegistration {
    address: u64,
    length: u32,
    signature: u32,
}

#[derive(Clone)]
enum TaskState {
    Runnable,
    Futex {
        address: u64,
        deadline: Option<u64>,
        private: bool,
        bitset: u32,
    },
    FutexWaitV {
        waiters: Vec<(u64, bool)>,
        deadline: Option<u64>,
    },
    PiFutex {
        address: u64,
        deadline: Option<u64>,
        private: bool,
        owner: u32,
        queued_at: u64,
    },
    PiRequeue {
        address: u64,
        target: u64,
        deadline: Option<u64>,
        private: bool,
        queued_at: u64,
    },
    Sleep {
        deadline: u64,
    },
    SignalWait,
    Wait {
        pid: i64,
        status: u64,
        rusage: u64,
    },
    WaitReady {
        child: u32,
        status: u64,
        rusage: u64,
        exit_status: i32,
    },
    Vfork {
        child: u32,
    },
}

struct LinuxTask {
    tid: u32,
    registers: Vec<u8>,
    clear_tid: u64,
    robust_list: u64,
    rseq: Option<RseqRegistration>,
    state: TaskState,
    signal_frames: Vec<SavedSignalFrame>,
}

const RSEQ_ABI_SIZE: u32 = 32;
const RSEQ_FLAG_UNREGISTER: u32 = 1;

fn rseq_transition(
    current: Option<RseqRegistration>,
    address: u64,
    length: u32,
    flags: u32,
    signature: u32,
) -> Result<(Option<RseqRegistration>, u32), i64> {
    if length != RSEQ_ABI_SIZE || address == 0 || address & 31 != 0 {
        return Err(starnix_kernel::EINVAL);
    }
    let requested = RseqRegistration {
        address,
        length,
        signature,
    };
    match flags {
        0 => {
            if current.is_some() {
                return Err(starnix_kernel::EBUSY);
            }
            Ok((Some(requested), 0))
        }
        RSEQ_FLAG_UNREGISTER if current == Some(requested) => Ok((None, u32::MAX)),
        RSEQ_FLAG_UNREGISTER => Err(starnix_kernel::EINVAL),
        _ => Err(starnix_kernel::EINVAL),
    }
}

struct ProcessContext {
    pid: u32,
    ppid: u32,
    exit_signal: u32,
    vfork_parent: Option<u32>,
    memory: AddressSpace,
    vfs: Vfs,
    signals: SignalState,
    dispatcher: Dispatcher,
    signal_frames: Vec<SavedSignalFrame>,
    tasks: Vec<LinuxTask>,
    current_task: usize,
    next_tid: u32,
    startup_writes: Vec<(u64, Vec<u8>)>,
}

struct ZombieProcess {
    pid: u32,
    ppid: u32,
    status: i32,
}

struct Runtime {
    restricted: RestrictedState,
    current: ProcessContext,
    processes: Vec<ProcessContext>,
    zombies: Vec<ZombieProcess>,
    next_pid: u32,
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
        futex_id: new_futex_id(),
        address,
        size,
        vmo_offset: 0,
        rights,
        kind,
        mapped: true,
    })
}

fn map_zero(address: u64, size: u64, rights: u32, kind: MappingKind) -> Result<Mapping, Error> {
    map_bytes(address, &[], size, rights, kind)
}

fn map_linux_image(
    image: &starnix_kernel::Image,
    executable: &[u8],
    interpreter: Option<&[u8]>,
) -> Result<Vec<Mapping>, Error> {
    let mut mappings = Vec::new();
    for (plan, bytes) in core::iter::once((&image.plan, executable))
        .chain(image.interpreter.as_ref().zip(interpreter))
    {
        for segment in &plan.segments {
            let size = bexos_boot::page_round(segment.file_size).ok_or(Error::Image)?;
            if size != 0 {
                let start = usize::try_from(segment.file_offset).map_err(|_| Error::Image)?;
                let end = start
                    .checked_add(usize::try_from(segment.file_size).map_err(|_| Error::Image)?)
                    .ok_or(Error::Image)?;
                mappings.push(map_bytes(
                    segment.vaddr,
                    bytes.get(start..end).ok_or(Error::Image)?,
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
    }
    mappings.push(map_bytes(
        starnix_kernel::GUEST_STACK_TOP - starnix_kernel::GUEST_STACK_SIZE,
        &image.stack,
        starnix_kernel::GUEST_STACK_SIZE,
        2 | 4,
        MappingKind::Stack,
    )?);
    Ok(mappings)
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
            futex_id: mapping.futex_id,
            address: mapping.address,
            size: mapping.size,
            offset: mapping.vmo_offset,
            rights: mapping.rights,
            kind: mapping_kind_id(mapping.kind),
            file_fd: match mapping.kind {
                MappingKind::File { fd, .. } => fd,
                _ => -1,
            },
            file_offset: match mapping.kind {
                MappingKind::File { file_offset, .. } => file_offset,
                _ => 0,
            },
            shared: matches!(mapping.kind, MappingKind::File { shared: true, .. }),
            owned: repeated,
        });
    }
    Ok(out)
}

fn mapping_kind_id(kind: MappingKind) -> u64 {
    match kind {
        MappingKind::Image => 0,
        MappingKind::Stack => 1,
        MappingKind::Anonymous => 2,
        MappingKind::File { .. } => 3,
        MappingKind::SignalTrampoline => 4,
    }
}

fn restore_mapping_kind(
    kind: u64,
    file_fd: i32,
    file_offset: u64,
    shared: bool,
) -> Result<MappingKind, Error> {
    match kind {
        0 => Ok(MappingKind::Image),
        1 => Ok(MappingKind::Stack),
        2 => Ok(MappingKind::Anonymous),
        3 => Ok(MappingKind::File {
            fd: file_fd,
            file_offset,
            shared,
        }),
        4 => Ok(MappingKind::SignalTrampoline),
        _ => Err(Error::Startup),
    }
}

fn migration_tasks(runtime: &Runtime) -> Vec<migration::TaskSnapshot> {
    process_tasks(
        &runtime.current,
        Some((
            runtime.current.current_task,
            capture_registers(&runtime.restricted),
            &runtime.current.signal_frames,
        )),
    )
}

fn process_tasks(
    process: &ProcessContext,
    active: Option<(usize, &[u8], &[SavedSignalFrame])>,
) -> Vec<migration::TaskSnapshot> {
    process
        .tasks
        .iter()
        .enumerate()
        .map(|(index, task)| {
            let is_active = active.is_some_and(|(current, _, _)| current == index);
            let registers = active
                .filter(|(current, _, _)| *current == index)
                .map_or_else(
                    || task.registers.clone(),
                    |(_, registers, _)| registers.to_vec(),
                );
            let frames = if is_active {
                active
                    .map(|(_, _, frames)| frames)
                    .unwrap_or(&task.signal_frames)
            } else {
                &task.signal_frames
            };
            migration::TaskSnapshot {
                tid: task.tid,
                registers,
                clear_tid: task.clear_tid,
                robust_list: task.robust_list,
                rseq: task.rseq.map(|registration| {
                    (
                        registration.address,
                        registration.length,
                        registration.signature,
                    )
                }),
                futex: match task.state {
                    TaskState::Futex {
                        address,
                        deadline,
                        private,
                        bitset,
                    } => Some((address, deadline, private, bitset)),
                    _ => None,
                },
                futex_waitv: match &task.state {
                    TaskState::FutexWaitV { waiters, deadline } => {
                        Some((waiters.clone(), *deadline))
                    }
                    _ => None,
                },
                pi_futex: match task.state {
                    TaskState::PiFutex {
                        address,
                        deadline,
                        private,
                        owner,
                        queued_at,
                    } => Some((address, deadline, private, owner, queued_at)),
                    _ => None,
                },
                pi_requeue: match task.state {
                    TaskState::PiRequeue {
                        address,
                        target,
                        deadline,
                        private,
                        queued_at,
                    } => Some((address, target, deadline, private, queued_at)),
                    _ => None,
                },
                sleep_deadline: match task.state {
                    TaskState::Sleep { deadline } => Some(deadline),
                    _ => None,
                },
                signal_frames: frames
                    .iter()
                    .map(|frame| (frame.address, frame.length as u64, frame.old_mask))
                    .collect(),
                wait: match task.state {
                    TaskState::Wait {
                        pid,
                        status,
                        rusage,
                    } => Some((pid, status, rusage)),
                    _ => None,
                },
                wait_ready: match task.state {
                    TaskState::WaitReady {
                        child,
                        status,
                        rusage,
                        exit_status,
                    } => Some((child, status, rusage, exit_status)),
                    _ => None,
                },
                vfork_child: match task.state {
                    TaskState::Vfork { child } => Some(child),
                    _ => None,
                },
                signal_wait: matches!(task.state, TaskState::SignalWait),
            }
        })
        .collect()
}

fn process_snapshot(
    process: &mut ProcessContext,
    seen_handles: &mut Vec<u64>,
    owned_handles: &mut Vec<u64>,
) -> Result<migration::ProcessSnapshot, Error> {
    let runtime = migration::RuntimeState {
        signals: process.signals.checkpoint(),
        dispatcher: process.dispatcher.checkpoint(),
        vfs: process.vfs.snapshot().map_err(|_| Error::Mapping)?,
        signal_frames: process
            .signal_frames
            .iter()
            .map(|frame| (frame.address, frame.length as u64, frame.old_mask))
            .collect(),
        tasks: process_tasks(process, None),
        current_task: process.current_task,
        next_tid: process.next_tid,
        pid: process.pid,
        ppid: process.ppid,
        exit_signal: process.exit_signal,
        vfork_parent: process.vfork_parent,
        startup_writes: process.startup_writes.clone(),
        processes: Vec::new(),
        zombies: Vec::new(),
        next_pid: process.next_tid,
    }
    .encode()
    .map_err(|_| Error::Mapping)?;
    let mut mappings = Vec::with_capacity(process.memory.mappings().len());
    for mapping in process.memory.mappings() {
        let original = mapping.vmo.as_handle_ref().raw_handle();
        let handle = if seen_handles.contains(&original) {
            let (_, rights) = Memory::object_info(original).map_err(|_| Error::Mapping)?;
            let duplicate = Memory::duplicate(original, rights).map_err(|_| Error::Mapping)?;
            owned_handles.push(duplicate);
            duplicate
        } else {
            seen_handles.push(original);
            original
        };
        mappings.push(migration::ProcessMapping {
            handle,
            futex_id: mapping.futex_id,
            address: mapping.address,
            size: mapping.size,
            offset: mapping.vmo_offset,
            rights: mapping.rights,
            kind: mapping_kind_id(mapping.kind),
            file_fd: match mapping.kind {
                MappingKind::File { fd, .. } => fd,
                _ => -1,
            },
            file_offset: match mapping.kind {
                MappingKind::File { file_offset, .. } => file_offset,
                _ => 0,
            },
            shared: matches!(mapping.kind, MappingKind::File { shared: true, .. }),
        });
    }
    Ok(migration::ProcessSnapshot {
        pid: process.pid,
        ppid: process.ppid,
        exit_signal: process.exit_signal,
        vfork_parent: process.vfork_parent,
        runtime,
        mappings,
        startup_writes: process.startup_writes.clone(),
    })
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
            if runtime.current.pid == 1 {
                let _ = queue_process_signal(&mut runtime.current, signal);
            } else if let Some(process) = runtime
                .processes
                .iter_mut()
                .find(|process| process.pid == 1)
            {
                let _ = queue_process_signal(process, signal);
            }
        }
    }
}

fn close_checkpoint_handles(mappings: Vec<migration::Mapping>, handles: Vec<u64>) {
    for mapping in mappings.into_iter().filter(|mapping| mapping.owned) {
        let _ = Memory::close(mapping.handle);
    }
    for handle in handles {
        let _ = Memory::close(handle);
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
    let mappings = match migration_mappings(&runtime.current.memory) {
        Ok(mappings) => mappings,
        Err(_) => {
            bexos_userspace::log("starnix_runner: migration mapping capture failed\n");
            runtime.source = Some(source);
            return;
        }
    };
    let mut seen_handles = runtime
        .current
        .memory
        .mappings()
        .iter()
        .map(|mapping| mapping.vmo.as_handle_ref().raw_handle())
        .collect::<Vec<_>>();
    seen_handles.sort_unstable();
    seen_handles.dedup();
    let mut owned_handles = Vec::new();
    let processes = match runtime
        .processes
        .iter_mut()
        .map(|process| process_snapshot(process, &mut seen_handles, &mut owned_handles))
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(processes) => processes,
        Err(_) => {
            close_checkpoint_handles(mappings, owned_handles);
            bexos_userspace::log("starnix_runner: migration process capture failed\n");
            runtime.source = Some(source);
            return;
        }
    };
    let runtime_state = runtime
        .current
        .vfs
        .snapshot()
        .map_err(|_| ())
        .and_then(|vfs| {
            migration::RuntimeState {
                signals: runtime.current.signals.checkpoint(),
                dispatcher: runtime.current.dispatcher.checkpoint(),
                vfs,
                signal_frames: runtime
                    .current
                    .signal_frames
                    .iter()
                    .map(|frame| (frame.address, frame.length as u64, frame.old_mask))
                    .collect(),
                tasks: migration_tasks(runtime),
                current_task: runtime.current.current_task,
                next_tid: runtime.current.next_tid,
                pid: runtime.current.pid,
                ppid: runtime.current.ppid,
                exit_signal: runtime.current.exit_signal,
                vfork_parent: runtime.current.vfork_parent,
                startup_writes: runtime.current.startup_writes.clone(),
                processes,
                zombies: runtime
                    .zombies
                    .iter()
                    .map(|zombie| (zombie.pid, zombie.ppid, zombie.status))
                    .collect(),
                next_pid: runtime.next_pid,
            }
            .encode()
            .map_err(|_| ())
        });
    let Ok(runtime_state) = runtime_state else {
        close_checkpoint_handles(mappings, owned_handles);
        bexos_userspace::log("starnix_runner: migration runtime capture failed\n");
        runtime.source = Some(source);
        return;
    };
    if runtime.migration.capture_runtime(runtime_state).is_err()
        || runtime
            .migration
            .capture_registers(capture_registers(&runtime.restricted))
            .is_err()
    {
        close_checkpoint_handles(mappings, owned_handles);
        bexos_userspace::log("starnix_runner: migration aborted at restricted safe point\n");
        runtime.source = Some(source);
        return;
    }
    runtime.migration.replace_mappings(mappings);
    runtime.migration.replace_owned_handles(owned_handles);
    if source.poll_pending(&runtime.migration).is_err() {
        bexos_userspace::log("starnix_runner: migration transfer failed\n");
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
    let frame = runtime.current.signal_frames.pop().ok_or(())?;
    let registers = runtime
        .current
        .memory
        .read(frame.address, frame.length)
        .map_err(|_| ())?
        .to_vec();
    restore_registers(&runtime.restricted, &registers).map_err(|_| ())?;
    runtime.current.signals.restore_mask(frame.old_mask);
    Ok(())
}

#[allow(unreachable_code)]
fn deliver_signal(runtime: &mut Runtime) -> Option<i32> {
    let (signal, action, old_mask) = loop {
        let (signal, action, old_mask) = runtime.current.signals.next()?;
        if action.handler == 1
            || (action.handler == 0 && matches!(signal, 17 | 18 | 20 | 21 | 22 | 23 | 28))
        {
            runtime.current.signals.restore_mask(old_mask);
            continue;
        }
        break (signal, action, old_mask);
    };
    if action.handler == 0 {
        return Some(128 + signal as i32);
    }
    #[cfg(not(bexos_guest))]
    let _ = old_mask;
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
            let alternate = runtime.current.signals.alt_stack();
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
        if runtime.current.memory.write(frame, &registers).is_err()
            || runtime
                .current
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
            let alternate = runtime.current.signals.alt_stack();
            if alternate.flags == 0 && alternate.size != 0 {
                alternate.address.saturating_add(alternate.size)
            } else {
                state.sp
            }
        } else {
            state.sp
        };
        let frame = stack_top.saturating_sub(registers.len() as u64) & !15;
        if runtime.current.memory.write(frame, &registers).is_err() {
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

    runtime.current.signal_frames.push(SavedSignalFrame {
        address: frame_address,
        length: registers.len(),
        old_mask,
    });
    None
}

fn set_task_result(registers: &mut [u8], value: u64) {
    #[cfg(all(bexos_guest, target_arch = "x86_64"))]
    unsafe {
        (*(registers.as_mut_ptr() as *mut bexos_userspace::restricted::X86_64StateV1)).rax = value;
    }
    #[cfg(all(bexos_guest, target_arch = "aarch64"))]
    unsafe {
        (*(registers.as_mut_ptr() as *mut bexos_userspace::restricted::Aarch64StateV1)).x[0] =
            value;
    }
    #[cfg(not(bexos_guest))]
    let _ = (registers, value);
}

fn initialize_clone(registers: &mut [u8], stack: u64, tls: u64, set_tls: bool) {
    set_task_result(registers, 0);
    #[cfg(all(bexos_guest, target_arch = "x86_64"))]
    unsafe {
        let state =
            &mut *(registers.as_mut_ptr() as *mut bexos_userspace::restricted::X86_64StateV1);
        if stack != 0 {
            state.rsp = stack;
        }
        if set_tls {
            state.fs_base = tls;
        }
    }
    #[cfg(all(bexos_guest, target_arch = "aarch64"))]
    unsafe {
        let state =
            &mut *(registers.as_mut_ptr() as *mut bexos_userspace::restricted::Aarch64StateV1);
        if stack != 0 {
            state.sp = stack;
        }
        if set_tls {
            state.tpidr_el0 = tls;
        }
    }
    #[cfg(not(bexos_guest))]
    let _ = (registers, stack, tls, set_tls);
}

fn runnable(state: &TaskState) -> bool {
    matches!(state, TaskState::Runnable | TaskState::WaitReady { .. })
}

fn update_deadlines(process: &mut ProcessContext, now: u64) {
    for task in &mut process.tasks {
        match task.state {
            TaskState::Futex {
                deadline: Some(deadline),
                ..
            }
            | TaskState::PiFutex {
                deadline: Some(deadline),
                ..
            }
            | TaskState::PiRequeue {
                deadline: Some(deadline),
                ..
            }
            | TaskState::FutexWaitV {
                deadline: Some(deadline),
                ..
            } if now >= deadline => {
                task.state = TaskState::Runnable;
                set_task_result(
                    &mut task.registers,
                    starnix_kernel::error(starnix_kernel::ETIMEDOUT),
                );
            }
            TaskState::Sleep { deadline } if now >= deadline => {
                task.state = TaskState::Runnable;
                set_task_result(&mut task.registers, 0);
            }
            _ => {}
        }
    }
}

fn first_runnable(process: &ProcessContext) -> Option<usize> {
    if process.tasks.is_empty() {
        return None;
    }
    (1..=process.tasks.len())
        .map(|offset| (process.current_task + offset) % process.tasks.len())
        .find(|index| runnable(&process.tasks[*index].state))
}

fn task_location(runtime: &Runtime, tid: u32) -> Option<PiWaiterLocation> {
    if let Some(index) = runtime
        .current
        .tasks
        .iter()
        .position(|task| task.tid == tid)
    {
        return Some(PiWaiterLocation::Current(index));
    }
    runtime
        .processes
        .iter()
        .enumerate()
        .find_map(|(process, context)| {
            context
                .tasks
                .iter()
                .position(|task| task.tid == tid)
                .map(|task| PiWaiterLocation::Process(process, task))
        })
}

fn task_at(runtime: &Runtime, location: PiWaiterLocation) -> &LinuxTask {
    match location {
        PiWaiterLocation::Current(task) => &runtime.current.tasks[task],
        PiWaiterLocation::Process(process, task) => &runtime.processes[process].tasks[task],
    }
}

fn task_at_mut(runtime: &mut Runtime, location: PiWaiterLocation) -> &mut LinuxTask {
    match location {
        PiWaiterLocation::Current(task) => &mut runtime.current.tasks[task],
        PiWaiterLocation::Process(process, task) => &mut runtime.processes[process].tasks[task],
    }
}

fn inherited_owner(runtime: &Runtime, mut tid: u32) -> Option<PiWaiterLocation> {
    for _ in 0..256 {
        let location = task_location(runtime, tid)?;
        match &task_at(runtime, location).state {
            TaskState::Runnable | TaskState::WaitReady { .. } => return Some(location),
            TaskState::PiFutex { owner, .. } if *owner != tid => tid = *owner,
            _ => return None,
        }
    }
    None
}

fn inherited_runnable(runtime: &Runtime) -> Option<PiWaiterLocation> {
    let mut selected: Option<((i8, u64, u32), PiWaiterLocation)> = None;
    let mut inspect = |process: &ProcessContext| {
        for task in &process.tasks {
            let TaskState::PiFutex {
                owner, queued_at, ..
            } = task.state
            else {
                continue;
            };
            let Some(location) = inherited_owner(runtime, owner) else {
                continue;
            };
            let key = (process.dispatcher.nice(), queued_at, task.tid);
            if selected.is_none_or(|current| key < current.0) {
                selected = Some((key, location));
            }
        }
    };
    inspect(&runtime.current);
    for process in &runtime.processes {
        inspect(process);
    }
    selected.map(|(_, location)| location)
}

fn complete_task_wakeup(process: &mut ProcessContext) {
    for (address, bytes) in core::mem::take(&mut process.startup_writes) {
        let _ = process.memory.write(address, &bytes);
    }
    let task = &mut process.tasks[process.current_task];
    if let TaskState::WaitReady {
        child,
        status,
        rusage,
        exit_status,
    } = task.state
    {
        let wait_status = if exit_status >= 128 {
            (exit_status - 128) & 0x7f
        } else {
            (exit_status & 0xff) << 8
        };
        let mut result = Ok(());
        if status != 0 {
            result = process.memory.write(status, &wait_status.to_ne_bytes());
        }
        if result.is_ok() && rusage != 0 {
            result = process.memory.write(rusage, &[0; 144]);
        }
        set_task_result(
            &mut task.registers,
            result
                .map(|()| u64::from(child))
                .unwrap_or_else(starnix_kernel::error),
        );
        task.state = TaskState::Runnable;
    }
}

fn restore_current_task(runtime: &mut Runtime) -> Result<(), ()> {
    complete_task_wakeup(&mut runtime.current);
    restore_registers(
        &runtime.restricted,
        &runtime.current.tasks[runtime.current.current_task].registers,
    )
    .map_err(|_| ())?;
    runtime.current.signal_frames =
        core::mem::take(&mut runtime.current.tasks[runtime.current.current_task].signal_frames);
    Ok(())
}

fn switch_task(runtime: &mut Runtime) -> Result<(), ()> {
    if runtime.current.tasks.is_empty() {
        return Err(());
    }
    runtime.current.tasks[runtime.current.current_task].registers =
        capture_registers(&runtime.restricted).to_vec();
    runtime.current.tasks[runtime.current.current_task].signal_frames =
        core::mem::take(&mut runtime.current.signal_frames);
    loop {
        let now = bexos_userspace::syscall::ticks();
        update_deadlines(&mut runtime.current, now);
        for process in &mut runtime.processes {
            update_deadlines(process, now);
        }
        if let Some(location) = inherited_runnable(runtime) {
            match location {
                PiWaiterLocation::Current(task) => {
                    runtime.current.current_task = task;
                }
                PiWaiterLocation::Process(process, task) => {
                    runtime.current.memory.deactivate().map_err(|_| ())?;
                    let mut next = runtime.processes.remove(process);
                    next.current_task = task;
                    let previous = core::mem::replace(&mut runtime.current, next);
                    runtime.processes.push(previous);
                    runtime.current.memory.activate().map_err(|_| ())?;
                }
            }
            break;
        }
        if let Some((process_index, task_index)) = runtime
            .processes
            .iter()
            .enumerate()
            .find_map(|(index, process)| first_runnable(process).map(|task| (index, task)))
        {
            runtime.current.memory.deactivate().map_err(|_| ())?;
            let mut next = runtime.processes.remove(process_index);
            next.current_task = task_index;
            let previous = core::mem::replace(&mut runtime.current, next);
            runtime.processes.push(previous);
            runtime.current.memory.activate().map_err(|_| ())?;
            break;
        }
        if let Some(next) = first_runnable(&runtime.current) {
            runtime.current.current_task = next;
            break;
        }
        poll_control(runtime);
        poll_migration(runtime);
        bexos_userspace::yield_now();
    }
    restore_current_task(runtime)
}

fn deadline_after_nanos(nanos: u64) -> u64 {
    let delta = (u128::from(nanos) * u128::from(bexos_userspace::syscall::frequency())
        / 1_000_000_000) as u64;
    bexos_userspace::syscall::ticks().saturating_add(delta)
}

fn wake_futex(runtime: &mut Runtime, address: u64, count: u32, private: bool) -> u32 {
    wake_futex_bitset(runtime, address, count, private, u32::MAX)
}

fn wake_futex_bitset(
    runtime: &mut Runtime,
    address: u64,
    count: u32,
    private: bool,
    bitset: u32,
) -> u32 {
    if private {
        return wake_process_futex(
            &mut runtime.current,
            address,
            count,
            Some(true),
            None,
            bitset,
        );
    }
    let Ok(key) = runtime.current.memory.futex_key(address) else {
        return 0;
    };
    let mut woken = wake_process_futex(
        &mut runtime.current,
        address,
        count,
        Some(false),
        Some(key),
        bitset,
    );
    for process in &mut runtime.processes {
        if woken == count {
            break;
        }
        woken += wake_process_futex(
            process,
            address,
            count - woken,
            Some(false),
            Some(key),
            bitset,
        );
    }
    woken
}

fn wake_process_futex(
    process: &mut ProcessContext,
    address: u64,
    count: u32,
    required_private: Option<bool>,
    shared_key: Option<(u64, u64)>,
    bitset: u32,
) -> u32 {
    let mut woken = 0;
    for task in &mut process.tasks {
        if woken == count {
            break;
        }
        let result = match &task.state {
            TaskState::Futex {
                address: waiting,
                private,
                bitset: waiting_bitset,
                ..
            } if required_private.is_none_or(|required| required == *private)
                && *waiting_bitset & bitset != 0
                && if *private || shared_key.is_none() {
                    *waiting == address
                } else {
                    process.memory.futex_key(*waiting).ok() == shared_key
                } =>
            {
                Some(0)
            }
            TaskState::FutexWaitV { waiters, .. } => waiters
                .iter()
                .position(|(waiting, private)| {
                    required_private.is_none_or(|required| required == *private)
                        && if *private || shared_key.is_none() {
                            *waiting == address
                        } else {
                            process.memory.futex_key(*waiting).ok() == shared_key
                        }
                })
                .map(|index| index as u64),
            _ => None,
        };
        if let Some(result) = result {
            task.state = TaskState::Runnable;
            set_task_result(&mut task.registers, result);
            woken += 1;
        }
    }
    woken
}

fn address_for_futex_key(process: &ProcessContext, key: (u64, u64)) -> Option<u64> {
    process.memory.mappings().iter().find_map(|mapping| {
        let offset = key.1.checked_sub(mapping.vmo_offset)?;
        (mapping.futex_id == key.0 && offset.checked_add(4)? <= mapping.size)
            .then(|| mapping.address.checked_add(offset))?
    })
}

fn requeue_process_futex(
    process: &mut ProcessContext,
    address: u64,
    source_key: Option<(u64, u64)>,
    target: u64,
    target_key: Option<(u64, u64)>,
    private: bool,
    wake_count: u32,
    requeue_count: u32,
) -> (u32, u32) {
    let target = if private {
        Some(target)
    } else {
        target_key.and_then(|key| address_for_futex_key(process, key))
    };
    let candidates = process
        .tasks
        .iter()
        .enumerate()
        .filter_map(|(index, task)| {
            let TaskState::Futex {
                address: waiting,
                private: waiting_private,
                ..
            } = task.state
            else {
                return None;
            };
            (waiting_private == private
                && if private {
                    waiting == address
                } else {
                    process.memory.futex_key(waiting).ok() == source_key
                })
            .then_some(index)
        })
        .collect::<Vec<_>>();
    let mut woken = 0;
    let mut requeued = 0;
    for index in candidates {
        let task = &mut process.tasks[index];
        if woken < wake_count {
            task.state = TaskState::Runnable;
            set_task_result(&mut task.registers, 0);
            woken += 1;
        } else if requeued < requeue_count
            && let Some(target) = target
        {
            if let TaskState::Futex { address, .. } = &mut task.state {
                *address = target;
                requeued += 1;
            }
        } else {
            break;
        }
    }
    (woken, requeued)
}

fn requeue_futex(
    runtime: &mut Runtime,
    address: u64,
    target: u64,
    wake_count: u32,
    requeue_count: u32,
    private: bool,
) -> Result<u32, i64> {
    let (source_key, target_key) = if private {
        (None, None)
    } else {
        (
            Some(runtime.current.memory.futex_key(address)?),
            Some(runtime.current.memory.futex_key(target)?),
        )
    };
    let (mut woken, mut requeued) = requeue_process_futex(
        &mut runtime.current,
        address,
        source_key,
        target,
        target_key,
        private,
        wake_count,
        requeue_count,
    );
    if !private {
        for process in &mut runtime.processes {
            if woken == wake_count && requeued == requeue_count {
                break;
            }
            let (process_woken, process_requeued) = requeue_process_futex(
                process,
                address,
                source_key,
                target,
                target_key,
                false,
                wake_count - woken,
                requeue_count - requeued,
            );
            woken += process_woken;
            requeued += process_requeued;
        }
    }
    Ok(woken + requeued)
}

fn futex_atomic_result(
    old: u32,
    operation: FutexAtomicOperation,
    operand: i32,
    comparison: FutexComparison,
    comparison_operand: i32,
) -> (u32, bool) {
    let operand = operand as u32;
    let new = match operation {
        FutexAtomicOperation::Set => operand,
        FutexAtomicOperation::Add => old.wrapping_add(operand),
        FutexAtomicOperation::Or => old | operand,
        FutexAtomicOperation::AndNot => old & !operand,
        FutexAtomicOperation::Xor => old ^ operand,
    };
    let old = old as i32;
    let compared = match comparison {
        FutexComparison::Equal => old == comparison_operand,
        FutexComparison::NotEqual => old != comparison_operand,
        FutexComparison::Less => old < comparison_operand,
        FutexComparison::LessOrEqual => old <= comparison_operand,
        FutexComparison::Greater => old > comparison_operand,
        FutexComparison::GreaterOrEqual => old >= comparison_operand,
    };
    (new, compared)
}

#[allow(clippy::too_many_arguments)]
fn wake_op_futex(
    runtime: &mut Runtime,
    address: u64,
    wake_count: u32,
    target: u64,
    target_wake_count: u32,
    operation: FutexAtomicOperation,
    operand: i32,
    comparison: FutexComparison,
    comparison_operand: i32,
    private: bool,
) -> Result<u32, i64> {
    let old = read_futex(&runtime.current, target)?;
    let (new, compared) =
        futex_atomic_result(old, operation, operand, comparison, comparison_operand);
    write_futex(&mut runtime.current, target, new)?;
    let mut woken = wake_futex(runtime, address, wake_count, private);
    if compared {
        woken = woken.saturating_add(wake_futex(runtime, target, target_wake_count, private));
    }
    Ok(woken)
}

fn pi_requeue_candidate(
    process: &ProcessContext,
    task: &LinuxTask,
    address: u64,
    private: bool,
    source_key: Option<(u64, u64)>,
    target_key: Option<(u64, u64)>,
) -> Option<((i8, u64, u32), u64)> {
    let TaskState::PiRequeue {
        address: waiting,
        target,
        private: waiting_private,
        queued_at,
        ..
    } = task.state
    else {
        return None;
    };
    let matches = if private {
        waiting_private && waiting == address
    } else {
        !waiting_private
            && process.memory.futex_key(waiting).ok() == source_key
            && process.memory.futex_key(target).ok() == target_key
    };
    matches.then_some(((process.dispatcher.nice(), queued_at, task.tid), target))
}

fn cmp_requeue_pi_futex(
    runtime: &mut Runtime,
    address: u64,
    target: u64,
    requeue_count: u32,
    private: bool,
) -> Result<u32, i64> {
    let (source_key, target_key) = if private {
        (None, None)
    } else {
        (
            Some(runtime.current.memory.futex_key(address)?),
            Some(runtime.current.memory.futex_key(target)?),
        )
    };
    let mut candidates = Vec::new();
    for (index, task) in runtime.current.tasks.iter().enumerate() {
        if let Some((key, waiter_target)) = pi_requeue_candidate(
            &runtime.current,
            task,
            address,
            private,
            source_key,
            target_key,
        ) {
            candidates.push((key, PiWaiterLocation::Current(index), waiter_target));
        }
    }
    if !private {
        for (process_index, process) in runtime.processes.iter().enumerate() {
            for (task_index, task) in process.tasks.iter().enumerate() {
                if let Some((key, waiter_target)) =
                    pi_requeue_candidate(process, task, address, false, source_key, target_key)
                {
                    candidates.push((
                        key,
                        PiWaiterLocation::Process(process_index, task_index),
                        waiter_target,
                    ));
                }
            }
        }
    }
    candidates.sort_by_key(|candidate| candidate.0);
    candidates.truncate(usize::try_from(requeue_count.saturating_add(1)).unwrap_or(usize::MAX));
    if candidates.is_empty() {
        return Ok(0);
    }

    let value = read_futex(&runtime.current, target)?;
    let current_owner = value & FUTEX_TID_MASK;
    let acquire = current_owner == 0;
    if !acquire && !task_exists(runtime, current_owner) {
        return Err(starnix_kernel::ESRCH);
    }
    let owner = if acquire {
        task_at(runtime, candidates[0].1).tid
    } else {
        current_owner
    };
    let replacement = if acquire {
        owner | (value & FUTEX_OWNER_DIED) | u32::from(candidates.len() > 1) * FUTEX_WAITERS
    } else {
        value | FUTEX_WAITERS
    };
    write_futex(&mut runtime.current, target, replacement)?;

    for (index, (_, location, waiter_target)) in candidates.iter().copied().enumerate() {
        let task = task_at_mut(runtime, location);
        let (deadline, queued_at, waiting_private) = match task.state {
            TaskState::PiRequeue {
                deadline,
                private,
                queued_at,
                ..
            } => (deadline, queued_at, private),
            _ => continue,
        };
        if acquire && index == 0 {
            task.state = TaskState::Runnable;
            let result = if value & FUTEX_OWNER_DIED != 0 {
                starnix_kernel::error(starnix_kernel::EOWNERDEAD)
            } else {
                0
            };
            set_task_result(&mut task.registers, result);
        } else {
            task.state = TaskState::PiFutex {
                address: waiter_target,
                deadline,
                private: waiting_private,
                owner,
                queued_at,
            };
        }
    }
    Ok(candidates.len() as u32)
}

const FUTEX_TID_MASK: u32 = 0x3fff_ffff;
const FUTEX_OWNER_DIED: u32 = 0x4000_0000;
const FUTEX_WAITERS: u32 = 0x8000_0000;

fn read_futex(process: &ProcessContext, address: u64) -> Result<u32, i64> {
    Ok(u32::from_ne_bytes(
        process.memory.read(address, 4)?.try_into().unwrap(),
    ))
}

fn write_futex(process: &mut ProcessContext, address: u64, value: u32) -> Result<(), i64> {
    process.memory.write(address, &value.to_ne_bytes())
}

fn task_exists(runtime: &Runtime, tid: u32) -> bool {
    runtime.current.tasks.iter().any(|task| task.tid == tid)
        || runtime
            .processes
            .iter()
            .any(|process| process.tasks.iter().any(|task| task.tid == tid))
}

fn lock_pi_futex(
    runtime: &mut Runtime,
    address: u64,
    timeout_nanos: i64,
    private: bool,
    try_only: bool,
) -> Result<bool, i64> {
    let tid = runtime.current.tasks[runtime.current.current_task].tid;
    let value = read_futex(&runtime.current, address)?;
    let owner = value & FUTEX_TID_MASK;
    if owner == tid {
        return Err(starnix_kernel::EDEADLK);
    }
    if value == 0 {
        write_futex(&mut runtime.current, address, tid)?;
        return Ok(true);
    }
    if owner == 0 && value & FUTEX_OWNER_DIED != 0 {
        write_futex(
            &mut runtime.current,
            address,
            tid | (value & (FUTEX_OWNER_DIED | FUTEX_WAITERS)),
        )?;
        return Err(starnix_kernel::EOWNERDEAD);
    }
    if try_only {
        return Err(starnix_kernel::EAGAIN);
    }
    write_futex(&mut runtime.current, address, value | FUTEX_WAITERS)?;
    if owner == 0 || !task_exists(runtime, owner) {
        return Err(starnix_kernel::ESRCH);
    }
    let deadline = (timeout_nanos >= 0).then(|| deadline_after_nanos(timeout_nanos as u64));
    runtime.current.tasks[runtime.current.current_task].state = TaskState::PiFutex {
        address,
        deadline,
        private,
        owner,
        queued_at: bexos_userspace::syscall::ticks(),
    };
    Ok(false)
}

#[derive(Clone, Copy)]
enum PiWaiterLocation {
    Current(usize),
    Process(usize, usize),
}

fn pi_waiter_key(
    process: &ProcessContext,
    task: &LinuxTask,
    address: u64,
    private: bool,
    shared_key: Option<(u64, u64)>,
) -> Option<(i8, u64, u32)> {
    let TaskState::PiFutex {
        address: waiting,
        private: waiting_private,
        queued_at,
        ..
    } = task.state
    else {
        return None;
    };
    let matches = if private {
        waiting_private && waiting == address
    } else {
        !waiting_private && process.memory.futex_key(waiting).ok() == shared_key
    };
    matches.then_some((process.dispatcher.nice(), queued_at, task.tid))
}

fn unlock_pi_futex(runtime: &mut Runtime, address: u64, private: bool) -> Result<(), i64> {
    let tid = runtime.current.tasks[runtime.current.current_task].tid;
    let value = read_futex(&runtime.current, address)?;
    if value & FUTEX_TID_MASK != tid {
        return Err(starnix_kernel::EPERM);
    }
    let shared_key = (!private)
        .then(|| runtime.current.memory.futex_key(address))
        .transpose()?;
    let mut selected: Option<((i8, u64, u32), PiWaiterLocation)> = None;
    let mut waiters = 0u32;
    for (index, task) in runtime.current.tasks.iter().enumerate() {
        if let Some(key) = pi_waiter_key(&runtime.current, task, address, private, shared_key) {
            waiters = waiters.saturating_add(1);
            if selected.is_none_or(|current| key < current.0) {
                selected = Some((key, PiWaiterLocation::Current(index)));
            }
        }
    }
    if !private {
        for (process_index, process) in runtime.processes.iter().enumerate() {
            for (task_index, task) in process.tasks.iter().enumerate() {
                if let Some(key) = pi_waiter_key(process, task, address, false, shared_key) {
                    waiters = waiters.saturating_add(1);
                    if selected.is_none_or(|current| key < current.0) {
                        selected =
                            Some((key, PiWaiterLocation::Process(process_index, task_index)));
                    }
                }
            }
        }
    }
    let Some(((_, _, next_tid), location)) = selected else {
        return write_futex(&mut runtime.current, address, 0);
    };
    let replacement = next_tid | u32::from(waiters > 1) * FUTEX_WAITERS;
    write_futex(&mut runtime.current, address, replacement)?;
    let task = match location {
        PiWaiterLocation::Current(index) => &mut runtime.current.tasks[index],
        PiWaiterLocation::Process(process, task) => &mut runtime.processes[process].tasks[task],
    };
    task.state = TaskState::Runnable;
    set_task_result(&mut task.registers, 0);
    Ok(())
}

fn robust_futex_address(node: u64, offset: i64) -> Option<u64> {
    if offset >= 0 {
        node.checked_add(offset as u64)
    } else {
        node.checked_sub(offset.unsigned_abs())
    }
}

fn robust_pointer(value: u64) -> u64 {
    value & !1
}

fn robust_owner_died(value: u32, tid: u32) -> Option<u32> {
    (value & FUTEX_TID_MASK == tid).then_some((value & FUTEX_WAITERS) | FUTEX_OWNER_DIED)
}

fn notify_robust_futex(
    process: &mut ProcessContext,
    tid: u32,
    node: u64,
    offset: i64,
) -> Option<u64> {
    let Some(address) = robust_futex_address(node, offset) else {
        return None;
    };
    let Ok(bytes) = process.memory.read(address, 4) else {
        return None;
    };
    let value = u32::from_ne_bytes(bytes.try_into().unwrap());
    let Some(replacement) = robust_owner_died(value, tid) else {
        return None;
    };
    (process
        .memory
        .write(address, &replacement.to_ne_bytes())
        .is_ok()
        && value & FUTEX_WAITERS != 0)
        .then_some(address)
}

fn handoff_dead_pi_futex(runtime: &mut Runtime, address: u64) -> bool {
    let shared_key = runtime.current.memory.futex_key(address).ok();
    let mut selected: Option<((i8, u64, u32), PiWaiterLocation)> = None;
    let mut waiters = 0u32;
    for (index, task) in runtime.current.tasks.iter().enumerate() {
        let TaskState::PiFutex {
            address: waiting,
            queued_at,
            ..
        } = task.state
        else {
            continue;
        };
        if waiting != address {
            continue;
        }
        waiters = waiters.saturating_add(1);
        let key = (runtime.current.dispatcher.nice(), queued_at, task.tid);
        if selected.is_none_or(|current| key < current.0) {
            selected = Some((key, PiWaiterLocation::Current(index)));
        }
    }
    for (process_index, process) in runtime.processes.iter().enumerate() {
        for (task_index, task) in process.tasks.iter().enumerate() {
            let TaskState::PiFutex {
                address: waiting,
                private,
                queued_at,
                ..
            } = task.state
            else {
                continue;
            };
            if private || process.memory.futex_key(waiting).ok() != shared_key {
                continue;
            }
            waiters = waiters.saturating_add(1);
            let key = (process.dispatcher.nice(), queued_at, task.tid);
            if selected.is_none_or(|current| key < current.0) {
                selected = Some((key, PiWaiterLocation::Process(process_index, task_index)));
            }
        }
    }
    let Some(((_, _, next_tid), location)) = selected else {
        return false;
    };
    let replacement = next_tid | FUTEX_OWNER_DIED | u32::from(waiters > 1) * FUTEX_WAITERS;
    if write_futex(&mut runtime.current, address, replacement).is_err() {
        return false;
    }
    let task = match location {
        PiWaiterLocation::Current(index) => &mut runtime.current.tasks[index],
        PiWaiterLocation::Process(process, task) => &mut runtime.processes[process].tasks[task],
    };
    task.state = TaskState::Runnable;
    set_task_result(&mut task.registers, 0);
    true
}

fn recover_robust_futex(runtime: &mut Runtime, address: u64) {
    if !handoff_dead_pi_futex(runtime, address) && wake_futex(runtime, address, 1, true) == 0 {
        let _ = wake_futex(runtime, address, 1, false);
    }
}

#[cfg(test)]
mod robust_tests {
    use super::*;

    #[test]
    fn robust_futex_offsets_and_owner_death_are_checked() {
        assert_eq!(robust_futex_address(0x1000, 8), Some(0x1008));
        assert_eq!(robust_futex_address(0x1000, -8), Some(0x0ff8));
        assert_eq!(robust_futex_address(4, -8), None);
        assert_eq!(robust_pointer(0x1001), 0x1000);
        assert_eq!(robust_owner_died(77, 77), Some(0x4000_0000));
        assert_eq!(robust_owner_died(0x8000_004d, 77), Some(0xc000_0000));
        assert_eq!(robust_owner_died(78, 77), None);
    }

    #[test]
    fn futex_wake_atomic_operations_compare_the_original_value() {
        assert_eq!(
            futex_atomic_result(
                7,
                FutexAtomicOperation::Add,
                -2,
                FutexComparison::Greater,
                6,
            ),
            (5, true)
        );
        assert_eq!(
            futex_atomic_result(
                0b1110,
                FutexAtomicOperation::AndNot,
                0b0110,
                FutexComparison::Equal,
                0b1000,
            ),
            (0b1000, false)
        );
        assert_eq!(
            futex_atomic_result(
                u32::MAX,
                FutexAtomicOperation::Set,
                3,
                FutexComparison::Less,
                0,
            ),
            (3, true)
        );
    }
}

fn notify_robust_list(process: &mut ProcessContext, tid: u32, head: u64) -> Vec<u64> {
    const ROBUST_LIST_LIMIT: usize = 2048;
    if head == 0 {
        return Vec::new();
    }
    let Ok(bytes) = process.memory.read(head, 24) else {
        return Vec::new();
    };
    let mut current = robust_pointer(u64::from_ne_bytes(bytes[0..8].try_into().unwrap()));
    let offset = i64::from_ne_bytes(bytes[8..16].try_into().unwrap());
    let pending = robust_pointer(u64::from_ne_bytes(bytes[16..24].try_into().unwrap()));
    let mut visited = Vec::new();
    let mut changed = Vec::new();
    while current != head && current != 0 && visited.len() < ROBUST_LIST_LIMIT {
        if visited.contains(&current) {
            return changed;
        }
        let Ok(node) = process.memory.read(current, 8) else {
            return changed;
        };
        let next = robust_pointer(u64::from_ne_bytes(node.try_into().unwrap()));
        if let Some(address) = notify_robust_futex(process, tid, current, offset) {
            changed.push(address);
        }
        visited.push(current);
        current = next;
    }
    if pending != 0 && !visited.contains(&pending) {
        if let Some(address) = notify_robust_futex(process, tid, pending, offset) {
            changed.push(address);
        }
    }
    changed
}

fn notify_all_robust_lists(process: &mut ProcessContext) -> Vec<u64> {
    let lists = process
        .tasks
        .iter()
        .map(|task| (task.tid, task.robust_list))
        .collect::<Vec<_>>();
    lists
        .into_iter()
        .flat_map(|(tid, head)| notify_robust_list(process, tid, head))
        .collect()
}

fn clone_thread(
    runtime: &mut Runtime,
    flags: u64,
    stack: u64,
    parent_tid: u64,
    child_tid: u64,
    tls: u64,
) -> Result<u32, i64> {
    const CLONE_VM: u64 = 0x0000_0100;
    const CLONE_FS: u64 = 0x0000_0200;
    const CLONE_FILES: u64 = 0x0000_0400;
    const CLONE_SIGHAND: u64 = 0x0000_0800;
    const CLONE_THREAD: u64 = 0x0001_0000;
    const CLONE_SYSVSEM: u64 = 0x0004_0000;
    const CLONE_SETTLS: u64 = 0x0008_0000;
    const CLONE_PARENT_SETTID: u64 = 0x0010_0000;
    const CLONE_CHILD_CLEARTID: u64 = 0x0020_0000;
    const CLONE_CHILD_SETTID: u64 = 0x0100_0000;
    const SUPPORTED: u64 = CLONE_VM
        | CLONE_FS
        | CLONE_FILES
        | CLONE_SIGHAND
        | CLONE_THREAD
        | CLONE_SYSVSEM
        | CLONE_SETTLS
        | CLONE_PARENT_SETTID
        | CLONE_CHILD_CLEARTID
        | CLONE_CHILD_SETTID;
    if flags & (CLONE_VM | CLONE_SIGHAND | CLONE_THREAD)
        != (CLONE_VM | CLONE_SIGHAND | CLONE_THREAD)
        || flags & 0xff != 0
        || flags & !SUPPORTED != 0
        || runtime.current.tasks.len() >= 256
    {
        return Err(starnix_kernel::ENOTSUP);
    }
    let tid = runtime.next_pid;
    runtime.next_pid = runtime
        .next_pid
        .checked_add(1)
        .ok_or(starnix_kernel::EAGAIN)?;
    runtime.current.next_tid = runtime.next_pid;
    if flags & CLONE_PARENT_SETTID != 0 {
        runtime
            .current
            .memory
            .write(parent_tid, &tid.to_ne_bytes())?;
    }
    if flags & CLONE_CHILD_SETTID != 0 {
        runtime
            .current
            .memory
            .write(child_tid, &tid.to_ne_bytes())?;
    }
    let mut registers = capture_registers(&runtime.restricted).to_vec();
    initialize_clone(&mut registers, stack, tls, flags & CLONE_SETTLS != 0);
    runtime.current.tasks.push(LinuxTask {
        tid,
        registers,
        clear_tid: if flags & CLONE_CHILD_CLEARTID != 0 {
            child_tid
        } else {
            0
        },
        robust_list: 0,
        rseq: None,
        state: TaskState::Runnable,
        signal_frames: Vec::new(),
    });
    Ok(tid)
}

fn clone_process(
    runtime: &mut Runtime,
    flags: u64,
    stack: u64,
    parent_tid: u64,
    child_tid: u64,
    tls: u64,
) -> Result<u32, i64> {
    const CLONE_VM: u64 = 0x0000_0100;
    const CLONE_SIGHAND: u64 = 0x0000_0800;
    const CLONE_VFORK: u64 = 0x0000_4000;
    const CLONE_THREAD: u64 = 0x0001_0000;
    const CLONE_SETTLS: u64 = 0x0008_0000;
    const CLONE_PARENT_SETTID: u64 = 0x0010_0000;
    const CLONE_CHILD_CLEARTID: u64 = 0x0020_0000;
    const CLONE_CHILD_SETTID: u64 = 0x0100_0000;
    const CLONE_CLEAR_SIGHAND: u64 = 0x1_0000_0000;
    const SUPPORTED: u64 = CLONE_VM
        | CLONE_SIGHAND
        | CLONE_VFORK
        | CLONE_SETTLS
        | CLONE_PARENT_SETTID
        | CLONE_CHILD_CLEARTID
        | CLONE_CHILD_SETTID
        | CLONE_CLEAR_SIGHAND
        | 0xff;
    let exit_signal = flags as u32 & 0xff;
    if flags & CLONE_THREAD != 0
        || flags & !SUPPORTED != 0
        || flags & CLONE_SIGHAND != 0
        || exit_signal > 64
        || runtime.processes.len() + runtime.zombies.len() >= 63
    {
        return Err(starnix_kernel::ENOTSUP);
    }
    let pid = runtime.next_pid;
    runtime.next_pid = runtime
        .next_pid
        .checked_add(1)
        .ok_or(starnix_kernel::EAGAIN)?;
    let memory = runtime.current.memory.fork(flags & CLONE_VM != 0)?;
    let vfs = runtime.current.vfs.fork_clone()?;
    let mut signals = runtime.current.signals.fork();
    if flags & CLONE_CLEAR_SIGHAND != 0 {
        signals.clear_handlers();
    }
    let mut registers = capture_registers(&runtime.restricted).to_vec();
    initialize_clone(&mut registers, stack, tls, flags & CLONE_SETTLS != 0);
    let clear_tid = if flags & CLONE_CHILD_CLEARTID != 0 {
        child_tid
    } else {
        0
    };
    let mut startup_writes = Vec::new();
    if flags & CLONE_CHILD_SETTID != 0 {
        startup_writes.push((child_tid, pid.to_ne_bytes().to_vec()));
    }
    if flags & CLONE_PARENT_SETTID != 0 {
        runtime
            .current
            .memory
            .write(parent_tid, &pid.to_ne_bytes())?;
    }
    let rseq = if flags & CLONE_VM == 0 {
        runtime.current.tasks[runtime.current.current_task].rseq
    } else {
        None
    };
    runtime.processes.push(ProcessContext {
        pid,
        ppid: runtime.current.pid,
        exit_signal,
        vfork_parent: (flags & CLONE_VFORK != 0).then_some(runtime.current.pid),
        memory,
        vfs,
        signals,
        dispatcher: runtime.current.dispatcher.clone(),
        signal_frames: Vec::new(),
        tasks: vec![LinuxTask {
            tid: pid,
            registers,
            clear_tid,
            robust_list: 0,
            rseq,
            state: TaskState::Runnable,
            signal_frames: Vec::new(),
        }],
        current_task: 0,
        next_tid: runtime.next_pid,
        startup_writes,
    });
    if flags & CLONE_VFORK != 0 {
        runtime.current.tasks[runtime.current.current_task].state = TaskState::Vfork { child: pid };
    }
    Ok(pid)
}

fn matches_wait(target: i64, child: u32) -> bool {
    target == -1 || target == i64::from(child)
}

fn queue_wait_result(process: &mut ProcessContext, child: u32, status: i32) -> bool {
    if let Some(task) = process
        .tasks
        .iter_mut()
        .find(|task| matches!(task.state, TaskState::Wait { pid, .. } if matches_wait(pid, child)))
    {
        let TaskState::Wait {
            status: status_address,
            rusage,
            ..
        } = task.state
        else {
            return false;
        };
        task.state = TaskState::WaitReady {
            child,
            status: status_address,
            rusage,
            exit_status: status,
        };
        true
    } else {
        false
    }
}

fn wake_vfork_parent(process: &mut ProcessContext, child: u32) {
    if let Some(task) = process
        .tasks
        .iter_mut()
        .find(|task| matches!(task.state, TaskState::Vfork { child: waiting } if waiting == child))
    {
        task.state = TaskState::Runnable;
    }
}

fn finish_child_process(runtime: &mut Runtime, status: i32) -> Result<(), ()> {
    for address in notify_all_robust_lists(&mut runtime.current) {
        recover_robust_futex(runtime, address);
    }
    let pid = runtime.current.pid;
    let ppid = runtime.current.ppid;
    let vfork_parent = runtime.current.vfork_parent;
    let parent = runtime
        .processes
        .iter_mut()
        .find(|process| process.pid == ppid);
    let reaped = if let Some(parent) = parent {
        if vfork_parent == Some(parent.pid) {
            wake_vfork_parent(parent, pid);
        }
        let reaped = queue_wait_result(parent, pid, status);
        if runtime.current.exit_signal != 0 {
            let _ = queue_process_signal(parent, runtime.current.exit_signal);
        }
        reaped
    } else {
        false
    };
    for process in &mut runtime.processes {
        if process.ppid == pid {
            process.ppid = 1;
        }
    }
    for zombie in &mut runtime.zombies {
        if zombie.ppid == pid {
            zombie.ppid = 1;
        }
    }
    runtime.current.memory.deactivate().map_err(|_| ())?;
    let (next_index, next_task) = runtime
        .processes
        .iter()
        .enumerate()
        .find_map(|(index, process)| first_runnable(process).map(|task| (index, task)))
        .or_else(|| {
            runtime
                .processes
                .first()
                .map(|process| (0, process.current_task))
        })
        .ok_or(())?;
    let mut next = runtime.processes.remove(next_index);
    next.current_task = next_task;
    next.memory.activate().map_err(|_| ())?;
    let exiting = core::mem::replace(&mut runtime.current, next);
    drop(exiting);
    if !reaped {
        runtime.zombies.push(ZombieProcess { pid, ppid, status });
    }
    Ok(())
}

fn schedule_after_process_exit(runtime: &mut Runtime) -> Result<(), ()> {
    restore_current_task(runtime)?;
    switch_task(runtime)
}

fn wait_status(status: i32) -> i32 {
    if status >= 128 {
        (status - 128) & 0x7f
    } else {
        (status & 0xff) << 8
    }
}

fn robust_list_for_tid(runtime: &Runtime, tid: i64) -> Result<u64, i64> {
    if tid == 0 {
        return Ok(runtime.current.tasks[runtime.current.current_task].robust_list);
    }
    let tid = u32::try_from(tid).map_err(|_| starnix_kernel::ESRCH)?;
    runtime
        .current
        .tasks
        .iter()
        .chain(
            runtime
                .processes
                .iter()
                .flat_map(|process| process.tasks.iter()),
        )
        .find(|task| task.tid == tid)
        .map(|task| task.robust_list)
        .ok_or(starnix_kernel::ESRCH)
}

fn wait_process(
    runtime: &mut Runtime,
    pid: i64,
    status: u64,
    options: u32,
    rusage: u64,
) -> Result<Option<u64>, i64> {
    const WNOHANG: u32 = 1;
    if (pid == 0 || pid < -1) || options & !WNOHANG != 0 {
        return Err(starnix_kernel::ENOTSUP);
    }
    let has_live_child = runtime
        .processes
        .iter()
        .any(|process| process.ppid == runtime.current.pid && matches_wait(pid, process.pid));
    if let Some(index) = runtime
        .zombies
        .iter()
        .position(|zombie| zombie.ppid == runtime.current.pid && matches_wait(pid, zombie.pid))
    {
        let zombie = runtime.zombies.remove(index);
        if status != 0 {
            runtime
                .current
                .memory
                .write(status, &wait_status(zombie.status).to_ne_bytes())?;
        }
        if rusage != 0 {
            runtime.current.memory.write(rusage, &[0; 144])?;
        }
        return Ok(Some(u64::from(zombie.pid)));
    }
    if !has_live_child {
        return Err(starnix_kernel::ECHILD);
    }
    if options & WNOHANG != 0 {
        return Ok(Some(0));
    }
    runtime.current.tasks[runtime.current.current_task].state = TaskState::Wait {
        pid,
        status,
        rusage,
    };
    Ok(None)
}

fn signal_process(
    runtime: &mut Runtime,
    pid: i64,
    tid: Option<u32>,
    signal: u32,
) -> Result<(), i64> {
    let current_pid = runtime.current.pid;
    let target_matches = |candidate: u32| match pid {
        -1 => true,
        0 => candidate == current_pid,
        value if value > 0 => candidate == value as u32,
        _ => false,
    };
    let valid_thread = |process: &ProcessContext| {
        tid.is_none_or(|tid| process.tasks.iter().any(|task| task.tid == tid))
    };
    let mut matched = false;
    if target_matches(runtime.current.pid) && valid_thread(&runtime.current) {
        matched = true;
        if signal != 0 {
            queue_process_signal(&mut runtime.current, signal)?;
        }
    }
    for process in &mut runtime.processes {
        if target_matches(process.pid) && valid_thread(process) {
            matched = true;
            if signal != 0 {
                queue_process_signal(process, signal)?;
            }
        }
    }
    if matched {
        Ok(())
    } else {
        Err(starnix_kernel::ESRCH)
    }
}

fn queue_process_signal(process: &mut ProcessContext, signal: u32) -> Result<(), i64> {
    process.signals.queue(signal)?;
    if !process.signals.has_interrupting_pending() {
        return Ok(());
    }
    if let Some(task) = process.tasks.iter_mut().find(|task| {
        matches!(
            task.state,
            TaskState::Futex { .. }
                | TaskState::FutexWaitV { .. }
                | TaskState::PiFutex { .. }
                | TaskState::PiRequeue { .. }
                | TaskState::Sleep { .. }
                | TaskState::Wait { .. }
                | TaskState::SignalWait
        )
    }) {
        task.state = TaskState::Runnable;
        set_task_result(
            &mut task.registers,
            starnix_kernel::error(starnix_kernel::EINTR),
        );
    }
    Ok(())
}

fn exit_current_task(runtime: &mut Runtime) {
    let task = runtime.current.tasks.remove(runtime.current.current_task);
    for address in notify_robust_list(&mut runtime.current, task.tid, task.robust_list) {
        recover_robust_futex(runtime, address);
    }
    if task.clear_tid != 0 {
        let _ = runtime
            .current
            .memory
            .write(task.clear_tid, &0u32.to_ne_bytes());
        if wake_futex(runtime, task.clear_tid, 1, true) == 0 {
            let _ = wake_futex(runtime, task.clear_tid, 1, false);
        }
    }
    runtime.current.signal_frames.clear();
    if !runtime.current.tasks.is_empty() {
        runtime.current.current_task %= runtime.current.tasks.len();
        restore_registers(
            &runtime.restricted,
            &runtime.current.tasks[runtime.current.current_task].registers,
        )
        .expect("valid task register frame");
        runtime.current.signal_frames =
            core::mem::take(&mut runtime.current.tasks[runtime.current.current_task].signal_frames);
    }
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
            if runtime.current.pid == 1 || finish_child_process(runtime, status).is_err() {
                unsafe {
                    finish(
                        context,
                        status,
                        "starnix_runner: guest terminated by signal\n",
                    )
                }
            }
            if schedule_after_process_exit(runtime).is_err() {
                unsafe { finish(context, 126, "starnix_runner: process resume failed\n") }
            }
        }
        unsafe { runtime.restricted.enter(vector, context) }.unwrap();
        unreachable!()
    }
    if reason != bexos_userspace::restricted::Reason::Syscall {
        unsafe { finish(context, 126, "starnix_runner: Linux exception exit\n") }
    }

    let (number, args) = syscall_state(runtime);
    let tids = runtime
        .current
        .tasks
        .iter()
        .map(|task| task.tid)
        .collect::<Vec<_>>();
    let mut process_ids = Vec::with_capacity(runtime.processes.len() + 1);
    process_ids.push(runtime.current.pid);
    process_ids.extend(runtime.processes.iter().map(|process| process.pid));
    runtime.current.dispatcher.set_task_context(
        runtime.current.tasks[runtime.current.current_task].tid,
        &tids,
    );
    runtime.current.dispatcher.set_process_context(
        runtime.current.pid,
        runtime.current.ppid,
        &process_ids,
    );
    let outcome = {
        let process = &mut runtime.current;
        process.dispatcher.call(
            Syscall::decode(architecture(), number),
            args,
            &mut process.memory,
            &mut process.vfs,
            &mut process.signals,
        )
    };
    match outcome {
        Outcome::Return(value) => set_result(runtime, value),
        Outcome::Exit { status, group } => {
            if group || runtime.current.tasks.len() == 1 {
                if runtime.current.pid == 1 || finish_child_process(runtime, status).is_err() {
                    unsafe {
                        finish(
                            context,
                            status,
                            &format!("starnix_runner: guest exited {status}\n"),
                        )
                    }
                }
                if schedule_after_process_exit(runtime).is_err() {
                    unsafe { finish(context, 126, "starnix_runner: process resume failed\n") }
                }
                unsafe { runtime.restricted.enter(vector, context) }.unwrap();
                unreachable!()
            }
            exit_current_task(runtime);
            unsafe { runtime.restricted.enter(vector, context) }.unwrap();
            unreachable!()
        }
        Outcome::Sigreturn => {
            if restore_signal(runtime).is_err() {
                unsafe { finish(context, 126, "starnix_runner: invalid signal return\n") }
            }
        }
        Outcome::SetArchBase { fs, value } => set_arch_base(runtime, fs, value),
        Outcome::SetTidAddress(address) => {
            runtime.current.tasks[runtime.current.current_task].clear_tid = address;
            set_result(
                runtime,
                u64::from(runtime.current.tasks[runtime.current.current_task].tid),
            );
        }
        Outcome::SetRobustList(address) => {
            runtime.current.tasks[runtime.current.current_task].robust_list = address;
            set_result(runtime, 0);
        }
        Outcome::GetRobustList { tid, head, length } => {
            let result = robust_list_for_tid(runtime, tid).and_then(|robust_list| {
                runtime
                    .current
                    .memory
                    .write(head, &robust_list.to_ne_bytes())?;
                runtime.current.memory.write(length, &24u64.to_ne_bytes())?;
                Ok(0)
            });
            set_result(runtime, result.unwrap_or_else(starnix_kernel::error));
        }
        Outcome::Rseq {
            address,
            length,
            flags,
            signature,
        } => {
            let current = runtime.current.tasks[runtime.current.current_task].rseq;
            let result = rseq_transition(current, address, length, flags, signature).and_then(
                |(registration, cpu_id)| {
                    runtime
                        .current
                        .memory
                        .validate_write(address, length as usize)?;
                    let mut cpu_fields = [0; 8];
                    cpu_fields[4..].copy_from_slice(&cpu_id.to_ne_bytes());
                    runtime.current.memory.write(address, &cpu_fields)?;
                    runtime.current.tasks[runtime.current.current_task].rseq = registration;
                    Ok(0)
                },
            );
            set_result(runtime, result.unwrap_or_else(starnix_kernel::error));
        }
        Outcome::Clone {
            flags,
            stack,
            parent_tid,
            child_tid,
            tls,
        } => {
            let result = if flags & 0x0001_0000 != 0 {
                clone_thread(runtime, flags, stack, parent_tid, child_tid, tls)
            } else {
                clone_process(runtime, flags, stack, parent_tid, child_tid, tls)
            }
            .map(u64::from)
            .unwrap_or_else(starnix_kernel::error);
            set_result(runtime, result);
        }
        Outcome::Wait {
            pid,
            status,
            options,
            rusage,
        } => match wait_process(runtime, pid, status, options, rusage) {
            Ok(Some(value)) => set_result(runtime, value),
            Ok(None) => {}
            Err(error) => set_result(runtime, starnix_kernel::error(error)),
        },
        Outcome::Signal { pid, tid, signal } => {
            let result = signal_process(runtime, pid, tid, signal)
                .map(|()| 0)
                .unwrap_or_else(starnix_kernel::error);
            set_result(runtime, result);
        }
        Outcome::FutexWait {
            address,
            timeout_nanos,
            private,
            bitset,
        } => {
            let deadline = (timeout_nanos >= 0).then(|| deadline_after_nanos(timeout_nanos as u64));
            runtime.current.tasks[runtime.current.current_task].state = TaskState::Futex {
                address,
                deadline,
                private,
                bitset,
            };
        }
        Outcome::FutexWake {
            address,
            count,
            private,
            bitset,
        } => {
            let woken = wake_futex_bitset(runtime, address, count, private, bitset);
            set_result(runtime, u64::from(woken));
        }
        Outcome::FutexRequeue {
            address,
            wake_count,
            requeue_count,
            target,
            private,
        } => {
            let result =
                requeue_futex(runtime, address, target, wake_count, requeue_count, private)
                    .map(u64::from)
                    .unwrap_or_else(starnix_kernel::error);
            set_result(runtime, result);
        }
        Outcome::FutexWakeOp {
            address,
            wake_count,
            target,
            target_wake_count,
            operation,
            operand,
            comparison,
            comparison_operand,
            private,
        } => {
            let result = wake_op_futex(
                runtime,
                address,
                wake_count,
                target,
                target_wake_count,
                operation,
                operand,
                comparison,
                comparison_operand,
                private,
            )
            .map(u64::from)
            .unwrap_or_else(starnix_kernel::error);
            set_result(runtime, result);
        }
        Outcome::FutexPiLock {
            address,
            timeout_nanos,
            private,
            try_only,
        } => match lock_pi_futex(runtime, address, timeout_nanos, private, try_only) {
            Ok(true) => set_result(runtime, 0),
            Ok(false) => {}
            Err(error) => set_result(runtime, starnix_kernel::error(error)),
        },
        Outcome::FutexPiUnlock { address, private } => {
            let result = unlock_pi_futex(runtime, address, private)
                .map(|()| 0)
                .unwrap_or_else(starnix_kernel::error);
            set_result(runtime, result);
        }
        Outcome::FutexWaitRequeuePi {
            address,
            target,
            timeout_nanos,
            private,
        } => {
            let deadline = (timeout_nanos >= 0).then(|| deadline_after_nanos(timeout_nanos as u64));
            runtime.current.tasks[runtime.current.current_task].state = TaskState::PiRequeue {
                address,
                target,
                deadline,
                private,
                queued_at: bexos_userspace::syscall::ticks(),
            };
        }
        Outcome::FutexCmpRequeuePi {
            address,
            target,
            requeue_count,
            private,
        } => {
            let result = cmp_requeue_pi_futex(runtime, address, target, requeue_count, private)
                .map(u64::from)
                .unwrap_or_else(starnix_kernel::error);
            set_result(runtime, result);
        }
        Outcome::FutexWaitV {
            waiters,
            timeout_nanos,
        } => {
            let deadline = (timeout_nanos >= 0).then(|| deadline_after_nanos(timeout_nanos as u64));
            runtime.current.tasks[runtime.current.current_task].state =
                TaskState::FutexWaitV { waiters, deadline };
        }
        Outcome::Sleep { duration_nanos } => {
            runtime.current.tasks[runtime.current.current_task].state = TaskState::Sleep {
                deadline: deadline_after_nanos(duration_nanos),
            };
        }
        Outcome::SignalWait => {
            set_result(runtime, starnix_kernel::error(starnix_kernel::EINTR));
            runtime.current.tasks[runtime.current.current_task].state =
                if runtime.current.signals.has_interrupting_pending() {
                    TaskState::Runnable
                } else {
                    TaskState::SignalWait
                };
        }
        Outcome::Exec {
            path,
            image,
            executable,
            interpreter,
        } => {
            let tid = runtime.current.pid;
            for address in notify_all_robust_lists(&mut runtime.current) {
                recover_robust_futex(runtime, address);
            }
            runtime.current.tasks.clear();
            runtime.current.tasks.push(LinuxTask {
                tid,
                registers: Vec::new(),
                clear_tid: 0,
                robust_list: 0,
                rseq: None,
                state: TaskState::Runnable,
                signal_frames: Vec::new(),
            });
            runtime.current.current_task = 0;
            runtime.current.memory.prepare_exec();
            let mappings = match map_linux_image(&image, &executable, interpreter.as_deref()) {
                Ok(mappings) => mappings,
                Err(_) => unsafe { finish(context, 126, "starnix_runner: exec mapping failed\n") },
            };
            for mapping in mappings {
                runtime.current.memory.add_mapping(mapping);
            }
            runtime.current.vfs.finish_exec(&path);
            runtime.current.signals.finish_exec();
            runtime.current.dispatcher.finish_exec(&path);
            runtime.current.signal_frames.clear();
            if let Some(parent_pid) = runtime.current.vfork_parent.take() {
                if let Some(parent) = runtime
                    .processes
                    .iter_mut()
                    .find(|process| process.pid == parent_pid)
                {
                    wake_vfork_parent(parent, runtime.current.pid);
                }
            }
            unsafe {
                initialize_state(
                    runtime.restricted.address(),
                    image.entry_vaddr,
                    image.stack_pointer,
                )
            };
        }
    }
    if let Some(status) = deliver_signal(runtime) {
        if runtime.current.pid == 1 || finish_child_process(runtime, status).is_err() {
            unsafe {
                finish(
                    context,
                    status,
                    "starnix_runner: guest terminated by signal\n",
                )
            }
        }
        if schedule_after_process_exit(runtime).is_err() {
            unsafe { finish(context, 126, "starnix_runner: process resume failed\n") }
        }
        unsafe { runtime.restricted.enter(vector, context) }.unwrap();
        unreachable!()
    }
    if switch_task(runtime).is_err() {
        unsafe { finish(context, 126, "starnix_runner: process schedule failed\n") }
    }
    if let Some(status) = deliver_signal(runtime) {
        if runtime.current.pid == 1 || finish_child_process(runtime, status).is_err() {
            unsafe {
                finish(
                    context,
                    status,
                    "starnix_runner: guest terminated by signal\n",
                )
            }
        }
        if schedule_after_process_exit(runtime).is_err() {
            unsafe { finish(context, 126, "starnix_runner: process resume failed\n") }
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

fn restore_frames(frames: Vec<(u64, u64, u64)>) -> Result<Vec<SavedSignalFrame>, Error> {
    frames
        .into_iter()
        .map(|(address, length, old_mask)| {
            Ok(SavedSignalFrame {
                address,
                length: usize::try_from(length).map_err(|_| Error::Startup)?,
                old_mask,
            })
        })
        .collect()
}

fn restore_tasks(snapshots: Vec<migration::TaskSnapshot>) -> Result<Vec<LinuxTask>, Error> {
    snapshots
        .into_iter()
        .map(|task| {
            let state = if let Some((child, status, rusage, exit_status)) = task.wait_ready {
                TaskState::WaitReady {
                    child,
                    status,
                    rusage,
                    exit_status,
                }
            } else if let Some((pid, status, rusage)) = task.wait {
                TaskState::Wait {
                    pid,
                    status,
                    rusage,
                }
            } else if let Some(child) = task.vfork_child {
                TaskState::Vfork { child }
            } else if let Some((address, deadline, private, bitset)) = task.futex {
                TaskState::Futex {
                    address,
                    deadline,
                    private,
                    bitset,
                }
            } else if let Some((waiters, deadline)) = task.futex_waitv {
                TaskState::FutexWaitV { waiters, deadline }
            } else if let Some((address, target, deadline, private, queued_at)) = task.pi_requeue {
                TaskState::PiRequeue {
                    address,
                    target,
                    deadline,
                    private,
                    queued_at,
                }
            } else if let Some((address, deadline, private, owner, queued_at)) = task.pi_futex {
                TaskState::PiFutex {
                    address,
                    deadline,
                    private,
                    owner,
                    queued_at,
                }
            } else if let Some(deadline) = task.sleep_deadline {
                TaskState::Sleep { deadline }
            } else if task.signal_wait {
                TaskState::SignalWait
            } else {
                TaskState::Runnable
            };
            Ok(LinuxTask {
                tid: task.tid,
                registers: task.registers,
                clear_tid: task.clear_tid,
                robust_list: task.robust_list,
                rseq: task
                    .rseq
                    .map(|(address, length, signature)| RseqRegistration {
                        address,
                        length,
                        signature,
                    }),
                state,
                signal_frames: restore_frames(task.signal_frames)?,
            })
        })
        .collect()
}

fn restore_process(
    snapshot: migration::ProcessSnapshot,
    base_vfs: &Vfs,
    process_ids: &[u32],
) -> Result<ProcessContext, Error> {
    let state = migration::RuntimeState::decode(&snapshot.runtime).map_err(|_| Error::Startup)?;
    if state.pid != snapshot.pid
        || state.ppid != snapshot.ppid
        || state.exit_signal != snapshot.exit_signal
        || state.vfork_parent != snapshot.vfork_parent
        || state.startup_writes != snapshot.startup_writes
        || !state.processes.is_empty()
        || !state.zombies.is_empty()
    {
        return Err(Error::Startup);
    }
    let mut vfs = base_vfs.fork_clone().map_err(|_| Error::Startup)?;
    vfs.restore(&state.vfs).map_err(|_| Error::Startup)?;
    let signals = SignalState::restore(&state.signals).map_err(|_| Error::Startup)?;
    let mut dispatcher =
        Dispatcher::restore(architecture(), &state.dispatcher).map_err(|_| Error::Startup)?;
    let tasks = restore_tasks(state.tasks)?;
    if tasks.is_empty() || state.current_task >= tasks.len() {
        return Err(Error::Startup);
    }
    let tids = tasks.iter().map(|task| task.tid).collect::<Vec<_>>();
    dispatcher.set_task_context(tasks[state.current_task].tid, &tids);
    dispatcher.set_process_context(snapshot.pid, snapshot.ppid, process_ids);
    let mappings = snapshot
        .mappings
        .into_iter()
        .map(|mapping| {
            Ok(Mapping {
                vmo: Arc::new(unsafe { Vmo::from_raw(mapping.handle) }),
                futex_id: mapping.futex_id,
                address: mapping.address,
                size: mapping.size,
                vmo_offset: mapping.offset,
                rights: mapping.rights,
                kind: restore_mapping_kind(
                    mapping.kind,
                    mapping.file_fd,
                    mapping.file_offset,
                    mapping.shared,
                )?,
                mapped: false,
            })
        })
        .collect::<Result<Vec<_>, Error>>()?;
    Ok(ProcessContext {
        pid: snapshot.pid,
        ppid: snapshot.ppid,
        exit_signal: snapshot.exit_signal,
        vfork_parent: snapshot.vfork_parent,
        memory: AddressSpace::new(mappings),
        vfs,
        signals,
        dispatcher,
        signal_frames: restore_frames(state.signal_frames)?,
        tasks,
        current_task: state.current_task,
        next_tid: state.next_tid,
        startup_writes: snapshot.startup_writes,
    })
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
        launch.options.rootfs.readonly,
        stdio(&startup, command.is_some())?,
        command.is_some(),
        &launch.options.path,
    )
    .map_err(|_| Error::Startup)?;
    let mut cmdline = arguments.join("\0").into_bytes();
    cmdline.push(0);
    for path in [
        "/proc/self/cmdline",
        "/proc/1/cmdline",
        "/proc/thread-self/cmdline",
    ] {
        vfs.set_synthetic(path, cmdline.clone());
    }
    if !launch.options.working_directory.is_empty() {
        vfs.chdir(&launch.options.working_directory)
            .map_err(|_| Error::Startup)?;
    }
    let interpreter_path = starnix_kernel::interpreter(&image_bytes).map_err(|_| Error::Image)?;
    let interpreter_bytes = interpreter_path
        .as_deref()
        .map(|path| vfs.read_file(path, 64 * 1024 * 1024))
        .transpose()
        .map_err(|_| Error::Image)?;
    let image = starnix_kernel::prepare_image_with_interpreter(
        &image_bytes,
        interpreter_bytes.as_deref(),
        architecture(),
        &arguments,
        &environment,
        launch.options.uid,
        launch.options.gid,
    )
    .map_err(|_| Error::Image)?;
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
        dispatcher: Dispatcher::new(architecture(), &launch.options).checkpoint(),
        vfs: vfs.snapshot().map_err(|_| Error::Startup)?,
        signal_frames: Vec::new(),
        tasks: vec![migration::TaskSnapshot {
            tid: 1,
            registers: Vec::new(),
            clear_tid: 0,
            robust_list: 0,
            rseq: None,
            futex: None,
            pi_futex: None,
            futex_waitv: None,
            pi_requeue: None,
            sleep_deadline: None,
            signal_frames: Vec::new(),
            wait: None,
            wait_ready: None,
            vfork_child: None,
            signal_wait: false,
        }],
        current_task: 0,
        next_tid: 2,
        pid: 1,
        ppid: 0,
        exit_signal: 0,
        vfork_parent: None,
        startup_writes: Vec::new(),
        processes: Vec::new(),
        zombies: Vec::new(),
        next_pid: 2,
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
            .map(|mapping| {
                Ok(Mapping {
                    vmo: Arc::new(unsafe { Vmo::from_raw(mapping.handle) }),
                    futex_id: mapping.futex_id,
                    address: mapping.address,
                    size: mapping.size,
                    vmo_offset: mapping.offset,
                    rights: mapping.rights,
                    kind: restore_mapping_kind(
                        mapping.kind,
                        mapping.file_fd,
                        mapping.file_offset,
                        mapping.shared,
                    )?,
                    mapped: true,
                })
            })
            .collect::<Result<Vec<_>, Error>>()?;
        (mappings, snapshot, true)
    } else {
        let mut mappings = Vec::new();
        mappings.extend(map_linux_image(
            &image,
            &image_bytes,
            interpreter_bytes.as_deref(),
        )?);
        mappings.push(map_bytes(
            SIGNAL_TRAMPOLINE,
            signal_trampoline(),
            4096,
            2 | 8,
            MappingKind::SignalTrampoline,
        )?);
        let restricted = RestrictedState::create().map_err(|_| Error::Restricted)?;
        unsafe { initialize_state(restricted.address(), image.entry_vaddr, image.stack_pointer) };
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
        unsafe { initialize_state(restricted.address(), image.entry_vaddr, image.stack_pointer) };
    }
    let runtime_state =
        migration::RuntimeState::decode(migration.runtime()).map_err(|_| Error::Startup)?;
    let current_pid = runtime_state.pid;
    let current_ppid = runtime_state.ppid;
    let current_exit_signal = runtime_state.exit_signal;
    let current_vfork_parent = runtime_state.vfork_parent;
    let current_startup_writes = runtime_state.startup_writes.clone();
    let restored_next_pid = runtime_state.next_pid;
    let signals = SignalState::restore(&runtime_state.signals).map_err(|_| Error::Startup)?;
    let mut dispatcher = Dispatcher::restore(architecture(), &runtime_state.dispatcher)
        .map_err(|_| Error::Startup)?;
    let signal_frames = restore_frames(runtime_state.signal_frames.clone())?;
    if candidate {
        vfs.restore(&runtime_state.vfs)
            .map_err(|_| Error::Startup)?;
    }
    let mut restored_process_ids = vec![current_pid];
    restored_process_ids.extend(runtime_state.processes.iter().map(|process| process.pid));
    let restored_processes = runtime_state
        .processes
        .clone()
        .into_iter()
        .map(|process| restore_process(process, &vfs, &restored_process_ids))
        .collect::<Result<Vec<_>, Error>>()?;
    let restored_zombies = runtime_state
        .zombies
        .iter()
        .map(|(pid, ppid, status)| ZombieProcess {
            pid: *pid,
            ppid: *ppid,
            status: *status,
        })
        .collect();
    restricted.bind().map_err(|_| Error::Restricted)?;
    if !candidate {
        Startup::ready(channel).map_err(|_| Error::Startup)?;
    }
    let source = Some(Source::new(migration.migration()));
    let initial_registers = capture_registers(&restricted).to_vec();
    let current_task = runtime_state.current_task;
    let next_tid = runtime_state.next_tid;
    let mut tasks = restore_tasks(runtime_state.tasks.clone())?;
    if tasks.is_empty() {
        tasks.push(LinuxTask {
            tid: 1,
            registers: initial_registers.clone(),
            clear_tid: 0,
            robust_list: 0,
            rseq: None,
            state: TaskState::Runnable,
            signal_frames: Vec::new(),
        });
    }
    tasks[current_task].registers = initial_registers;
    let mut signal_frames = if tasks[current_task].signal_frames.is_empty() {
        signal_frames
    } else {
        core::mem::take(&mut tasks[current_task].signal_frames)
    };
    let tids = tasks.iter().map(|task| task.tid).collect::<Vec<_>>();
    dispatcher.set_task_context(tasks[current_task].tid, &tids);
    dispatcher.set_process_context(current_pid, current_ppid, &restored_process_ids);
    let runtime = Box::new(Runtime {
        restricted,
        current: ProcessContext {
            pid: current_pid,
            ppid: current_ppid,
            exit_signal: current_exit_signal,
            vfork_parent: current_vfork_parent,
            memory: AddressSpace::new(mappings),
            vfs,
            signals,
            dispatcher,
            signal_frames: core::mem::take(&mut signal_frames),
            tasks,
            current_task,
            next_tid,
            startup_writes: current_startup_writes,
        },
        processes: restored_processes,
        zombies: restored_zombies,
        next_pid: restored_next_pid.max(next_tid).max(2),
        migration,
        source,
        control: channel,
    });
    let context = Box::into_raw(runtime) as u64;
    let runtime = unsafe { &*(context as *const Runtime) };
    unsafe { runtime.restricted.enter(vector, context) }.map_err(|_| Error::Restricted)?;
    unreachable!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rseq_registration_enforces_linux_lifecycle() {
        let signature = 0x5305_3053;
        let registration = RseqRegistration {
            address: 0x2000,
            length: RSEQ_ABI_SIZE,
            signature,
        };
        assert_eq!(
            rseq_transition(None, 0x2000, RSEQ_ABI_SIZE, 0, signature),
            Ok((Some(registration), 0))
        );
        assert_eq!(
            rseq_transition(Some(registration), 0x2000, RSEQ_ABI_SIZE, 0, signature),
            Err(starnix_kernel::EBUSY)
        );
        assert_eq!(
            rseq_transition(
                Some(registration),
                0x2000,
                RSEQ_ABI_SIZE,
                RSEQ_FLAG_UNREGISTER,
                signature,
            ),
            Ok((None, u32::MAX))
        );
        assert_eq!(
            rseq_transition(
                Some(registration),
                0x2000,
                RSEQ_ABI_SIZE,
                RSEQ_FLAG_UNREGISTER,
                signature ^ 1,
            ),
            Err(starnix_kernel::EINVAL)
        );
        assert_eq!(
            rseq_transition(None, 0x2001, RSEQ_ABI_SIZE, 0, signature),
            Err(starnix_kernel::EINVAL)
        );
        assert_eq!(
            rseq_transition(None, 0x2000, RSEQ_ABI_SIZE - 1, 0, signature),
            Err(starnix_kernel::EINVAL)
        );
        assert_eq!(
            rseq_transition(None, 0x2000, RSEQ_ABI_SIZE, 2, signature),
            Err(starnix_kernel::EINVAL)
        );
    }
}
