#![no_main]
bexos_libc::entry!(run);
fn run(channel: u64) -> ! {
    let startup = bexos_userspace::Startup::receive(bexos_userspace::Channel(channel)).unwrap();
    bexos_libc::install_startup(&startup);
    bexos_userspace::Startup::ready(bexos_userspace::Channel(channel)).unwrap();
    bexos_userspace::log("pkg-remote-app: launched verified OCI installation\n");
    bexos_userspace::syscall::exit_with_status(0)
}
