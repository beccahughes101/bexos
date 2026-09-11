#![no_main]
bexos_libc::entry!(run);

fn run(channel: u64) -> ! {
    bexos_userspace::log("appd: enter main\n");
    bexos_userspace::block_on(bexos_appd::guest::main(channel))
}
