#![no_main]
bexos_libc::entry!(run);
fn run(channel: u64) -> ! {
    std::panic::set_hook(Box::new(|panic| {
        bexos_userspace::log(&format!("pkgd: {panic}\n"))
    }));
    bexos_userspace::block_on(bexos_pkgd::runtime::main(channel))
}
