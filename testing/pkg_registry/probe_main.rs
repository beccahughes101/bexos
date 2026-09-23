#![no_main]
bexos_libc::entry!(run);
fn run(channel: u64) -> ! {
    pkg_probe::run(channel)
}
