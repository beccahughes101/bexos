#![no_main]
bexos_libc::entry!(run);
fn run(channel: u64) -> ! {
    bexos_userspace::log("input-fixture: runtime initialized\n");
    std::panic::set_hook(Box::new(|info| {
        bexos_userspace::log(&format!("input-fixture: panic: {info}\n"))
    }));
    bexos_userspace::block_on(bexos_input_fixture::main(channel))
}
