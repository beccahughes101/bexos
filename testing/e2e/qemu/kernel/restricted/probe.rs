#![no_main]
#![no_std]

#[cfg(all(bexos_guest, target_arch = "aarch64"))]
use bexos_userspace::restricted::Aarch64StateV1;
#[cfg(all(bexos_guest, target_arch = "x86_64"))]
use bexos_userspace::restricted::X86_64StateV1;
use bexos_userspace::restricted::{Reason, VectorEntry};
use bexos_userspace::{Channel, Memory, Startup, log, restricted, yield_now};
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use kernel_fidl::{
    HandleRef, Status, TaskControlCreateThreadRequest, TaskControlCreateThreadResponse,
};

bexos_userspace::entry!(run);

static WORKER_READY: AtomicBool = AtomicBool::new(false);
static WORKER_KICKED: AtomicBool = AtomicBool::new(false);
static MAIN_STAGE: AtomicU64 = AtomicU64::new(0);

unsafe extern "C" {
    fn restricted_syscall_guest();
    fn restricted_after_syscall();
    fn restricted_exception_guest();
    fn restricted_spin_guest();
}

#[cfg(all(bexos_guest, target_arch = "x86_64"))]
core::arch::global_asm!(
    r#"
.global restricted_syscall_guest
restricted_syscall_guest:
    mov rax, 0x321
    mov rdi, 0x44
    syscall
.global restricted_after_syscall
restricted_after_syscall:
    cmp rax, 77
    jne 1f
.global restricted_exception_guest
restricted_exception_guest:
    ud2
1:
    int3
.global restricted_spin_guest
restricted_spin_guest:
    pause
    jmp restricted_spin_guest
"#
);

#[cfg(all(bexos_guest, target_arch = "aarch64"))]
core::arch::global_asm!(
    r#"
.global restricted_syscall_guest
restricted_syscall_guest:
    mov x8, #0x321
    mov x0, #0x44
    svc #0
.global restricted_after_syscall
restricted_after_syscall:
    cmp x0, #77
    b.ne 1f
.global restricted_exception_guest
restricted_exception_guest:
    brk #0
1:
    brk #1
.global restricted_spin_guest
restricted_spin_guest:
    yield
    b restricted_spin_guest
"#
);

fn stack() -> u64 {
    let vmo = Memory::create(64 * 1024, 0).unwrap();
    let base = Memory::map(vmo, 64 * 1024, 2 | 4).unwrap();
    Memory::close(vmo).unwrap();
    base + 64 * 1024
}

unsafe fn initialize_state(address: u64, pc: u64, sp: u64) {
    #[cfg(all(bexos_guest, target_arch = "x86_64"))]
    unsafe {
        let mut state = X86_64StateV1::zeroed();
        state.rip = pc;
        state.rsp = sp;
        state.rflags = 0x202;
        (address as *mut X86_64StateV1).write(state);
    }
    #[cfg(all(bexos_guest, target_arch = "aarch64"))]
    unsafe {
        let mut state = Aarch64StateV1::zeroed();
        state.pc = pc;
        state.sp = sp;
        (address as *mut Aarch64StateV1).write(state);
    }
}

unsafe extern "C" fn worker_vector(_context: u64, reason: Reason) -> ! {
    assert_eq!(reason, Reason::Kick);
    restricted::unbind().unwrap();
    WORKER_KICKED.store(true, Ordering::Release);
    bexos_userspace::syscall::exit_with_status(0)
}

unsafe extern "C" fn main_vector(context: u64, reason: Reason) -> ! {
    match MAIN_STAGE.fetch_add(1, Ordering::AcqRel) {
        0 => {
            assert_eq!(reason, Reason::Syscall);
            #[cfg(all(bexos_guest, target_arch = "x86_64"))]
            unsafe {
                let state = &mut *(context as *mut X86_64StateV1);
                assert_eq!(state.rax, 0x321);
                assert_eq!(state.rdi, 0x44);
                assert_eq!(state.rip, restricted_after_syscall as *const () as u64);
                state.rax = 77;
            }
            #[cfg(all(bexos_guest, target_arch = "aarch64"))]
            unsafe {
                let state = &mut *(context as *mut Aarch64StateV1);
                assert_eq!(state.x[8], 0x321);
                assert_eq!(state.x[0], 0x44);
                assert_eq!(state.pc, restricted_after_syscall as *const () as u64);
                state.x[0] = 77;
            }
            unsafe { restricted::enter(main_vector as VectorEntry, context) }.unwrap();
            unreachable!()
        }
        1 => {
            assert_eq!(reason, Reason::Exception);
            #[cfg(all(bexos_guest, target_arch = "x86_64"))]
            unsafe {
                let state = &*(context as *const X86_64StateV1);
                assert_eq!(state.rip, restricted_exception_guest as *const () as u64);
                assert_ne!(state.header.exception_code, 0);
            }
            #[cfg(all(bexos_guest, target_arch = "aarch64"))]
            unsafe {
                let state = &*(context as *const Aarch64StateV1);
                assert_eq!(state.pc, restricted_exception_guest as *const () as u64);
                assert_ne!(state.header.exception_code, 0);
            }
            restricted::unbind().unwrap();
            assert!(WORKER_KICKED.load(Ordering::Acquire));
            log("restricted-probe: PASS syscall exception kick\n");
            bexos_userspace::exit()
        }
        _ => panic!("unexpected restricted callback"),
    }
}

extern "C" fn worker(_argument: u64) -> ! {
    let state = restricted::RestrictedState::create().unwrap();
    unsafe {
        initialize_state(
            state.address(),
            restricted_spin_guest as *const () as u64,
            stack(),
        );
    }
    state.bind().unwrap();
    WORKER_READY.store(true, Ordering::Release);
    unsafe { state.enter(worker_vector as VectorEntry, state.address()) }.unwrap();
    panic!("restricted worker enter returned")
}

fn create_worker() -> Result<u64, Status> {
    let response: TaskControlCreateThreadResponse = bexos_userspace::ipc::kernel_call(
        3,
        "CreateThread",
        kernel_fidl::TASK_CONTROL_PUBLIC_METHODS,
        &TaskControlCreateThreadRequest {
            entry_vaddr: worker as *const () as u64,
            stack_top_vaddr: stack(),
            arg_handle: HandleRef { raw: 0 },
        },
    )?;
    bexos_userspace::ipc::check(response.status)?;
    Ok(response.thread_handle.raw)
}

fn run(channel: u64) -> ! {
    let control = Channel(channel);
    Startup::receive(control).unwrap();
    Startup::ready(control).unwrap();

    let worker = create_worker().unwrap();
    while !WORKER_READY.load(Ordering::Acquire) {
        yield_now();
    }
    restricted::kick(worker).unwrap();
    while !WORKER_KICKED.load(Ordering::Acquire) {
        yield_now();
    }

    let state = restricted::RestrictedState::create().unwrap();
    unsafe {
        initialize_state(
            state.address(),
            restricted_syscall_guest as *const () as u64,
            stack(),
        );
    }
    state.bind().unwrap();
    unsafe { state.enter(main_vector as VectorEntry, state.address()) }.unwrap();
    panic!("restricted main enter returned")
}
