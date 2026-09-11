#![no_std]
#![no_main]
extern crate alloc;
mod guest;
bexos_userspace::entry!(run);
fn run(channel: u64) -> ! {
    guest::run(channel)
}
