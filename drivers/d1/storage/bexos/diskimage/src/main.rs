#![no_main]

bexos_libc::entry!(run);

fn run(channel: u64) -> ! {
    std::panic::set_hook(Box::new(|info| {
        bexos_userspace::log(&format!("diskimage: panic: {info}\n"));
    }));
    bexos_userspace::block_on(bexos_d1_diskimage::guest::main(channel))
}
