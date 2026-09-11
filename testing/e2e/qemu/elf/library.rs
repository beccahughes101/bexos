#![no_std]
#[global_allocator]
static ALLOCATOR: bexos_libc::Allocator = bexos_libc::Allocator;
unsafe extern "C" {
    fn fixture_read() -> u64;
    fn fixture_write(value: u64);
    fn fixture_constructors() -> u32;
}
#[unsafe(no_mangle)]
pub extern "C" fn bexos_fixture_read() -> u64 {
    unsafe { fixture_read() }
}
#[unsafe(no_mangle)]
pub extern "C" fn bexos_fixture_write(value: u64) {
    unsafe { fixture_write(value) }
}
#[unsafe(no_mangle)]
pub extern "C" fn bexos_fixture_constructors() -> u32 {
    unsafe { fixture_constructors() }
}
#[unsafe(no_mangle)]
pub extern "C" fn rust_eh_personality() {}
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    bexos_userspace::exit()
}
