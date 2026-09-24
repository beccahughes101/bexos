#![no_main]

bexos_libc::entry!(run);

fn run(channel: u64) -> ! {
    #[cfg(starnix_runner_replacement)]
    bexos_userspace::log("starnix_runner: replacement runtime started\n");
    let status = match bexos_starnix_runner::run(bexos_userspace::Channel(channel)) {
        Ok(status) => status,
        Err(error) => {
            bexos_userspace::log(&format!("starnix_runner: failed: {error:?}\n"));
            126
        }
    };
    bexos_userspace::log(&format!("starnix_runner: exit {status}\n"));
    bexos_userspace::syscall::exit_with_status(i32::from(status))
}
