#![no_main]
bexos_libc::entry!(run);
fn run(channel: u64) -> ! {
    #[cfg(wasm_runner_replacement)]
    bexos_userspace::log("wasm_runner: replacement runtime started\n");
    let status = match bexos_wasm_runner::launch::run(bexos_userspace::Channel(channel)) {
        Ok(status) => {
            bexos_userspace::log(&format!("wasm_runner: exit {status}\n"));
            status
        }
        Err(error) => {
            bexos_userspace::log(&format!("wasm_runner: failed: {error:#}\n"));
            126
        }
    };
    bexos_userspace::syscall::exit_with_status(status.into())
}
