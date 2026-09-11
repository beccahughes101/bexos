#![no_main]
bexos_libc::entry!(run);
fn run(channel: u64) -> ! {
    bexos_d1_virtio_console::main(channel)
}
