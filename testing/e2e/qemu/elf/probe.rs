#![no_main]
mod keychain;
mod network;
use bexos_userspace::{Channel, Startup, dynamic_link, log, yield_now};
use std::cell::Cell;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Barrier};
#[cfg(all(bexos_guest, target_arch = "x86_64"))]
#[path = "arch/x86_64.rs"]
mod arch;
#[cfg(all(bexos_guest, target_arch = "aarch64"))]
#[path = "arch/aarch64.rs"]
mod arch;
bexos_libc::entry!(run);
std::thread_local! { static LOCAL: Cell<u64> = const { Cell::new(23) }; }
fn run(channel: u64) -> ! {
    std::panic::set_hook(Box::new(|panic| log(&format!("elf-probe: {panic}\n"))));
    log("elf-probe: receiving startup\n");
    let control = Channel(channel);
    let startup = Startup::receive(control).unwrap();
    bexos_libc::install_startup(&startup);
    log("elf-probe: startup and constructors installed\n");
    // The host starts replacements after launch returns, while the workload
    // deliberately keeps a TCP request pending across those replacements.
    Startup::ready(control).unwrap();
    let read: extern "C" fn() -> u64 = unsafe {
        core::mem::transmute(dynamic_link::symbol_address(b"bexos_fixture_read").unwrap() as usize)
    };
    let write: extern "C" fn(u64) = unsafe {
        core::mem::transmute(dynamic_link::symbol_address(b"bexos_fixture_write").unwrap() as usize)
    };
    let constructors: extern "C" fn() -> u32 = unsafe {
        core::mem::transmute(
            dynamic_link::symbol_address(b"bexos_fixture_constructors").unwrap() as usize,
        )
    };
    assert_eq!(constructors(), 1);
    assert_eq!(read(), 17);
    write(100);
    LOCAL.with(|value| {
        assert_eq!(value.get(), 23);
        value.set(101);
    });
    let barrier = Arc::new(Barrier::new(3));
    let ready = Arc::new([AtomicU64::new(0), AtomicU64::new(0)]);
    let mut threads = Vec::new();
    for id in 1..=2u64 {
        let barrier = barrier.clone();
        let ready = ready.clone();
        log(&format!("elf-probe: spawning thread {id}\n"));
        threads.push(std::thread::spawn(move || {
            log(&format!("elf-probe: thread {id} started\n"));
            assert_eq!(read(), 17);
            LOCAL.with(|value| {
                assert_eq!(value.get(), 23);
                value.set(id);
            });
            write(id * 1000);
            barrier.wait();
            log(&format!("elf-probe: thread {id} testing preemption\n"));
            arch::preserve_vectors(
                0x1122334400000000 | id,
                &ready[(id - 1) as usize],
                &ready[(2 - id) as usize],
            );
            log(&format!("elf-probe: thread {id} preserved vectors\n"));
            for _ in 0..32 {
                yield_now();
            }
            assert_eq!(read(), id * 1000 + 3);
            LOCAL.with(|value| assert_eq!(value.get(), id));
            assert_eq!(constructors(), 1);
            log(&format!("elf-probe: thread {id} finished TLS checks\n"));
        }));
    }
    log("elf-probe: main waiting for threads\n");
    barrier.wait();
    for thread in threads {
        thread.join().unwrap();
        log("elf-probe: joined thread\n");
    }
    assert_eq!(read(), 103);
    LOCAL.with(|value| assert_eq!(value.get(), 101));
    log("elf-probe: constructors and executable/library TLS across three threads verified\n");
    log("elf-probe: timer preemption and vector preservation verified\n");
    let mode = startup.arg0 >> 32;
    if mode == 1 {
        keychain::store(&startup);
    }
    network::verify(&startup);
    if mode == 1 {
        keychain::verify(&startup);
    }
    if mode == 2 {
        keychain::verify_lazy_activation(&startup);
        bexos_userspace::exit();
    }
    loop {
        yield_now();
    }
}
