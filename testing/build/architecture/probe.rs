#![no_std]
#![no_main]

unsafe extern "C" {
    fn architecture_c_probe() -> usize;
    fn architecture_cpp_probe() -> usize;
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    // Exercise the real C/C++/Rust cross-link without executing a guest on the host.
    core::hint::black_box(unsafe { (architecture_c_probe(), architecture_cpp_probe()) });
    loop {
        core::hint::spin_loop();
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
