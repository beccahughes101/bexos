#![no_main]

bexos_libc::entry!(run);

fn run(channel: u64) -> ! {
    bexos_userspace::block_on(bexos_i2cd::main(channel))
}
