#![no_main]

bexos_libc::entry!(run);

fn run(channel: u64) -> ! {
    let status = match bexos_native_runner::run(bexos_userspace::Channel(channel)) {
        Ok(()) => 0,
        Err(error) => {
            let _ = bexos_component_runner::stop(kernel_fidl::Status::ErrInvalidArgs, 126);
            bexos_userspace::log(&format!("native_runner: failed: {error:?}\n"));
            126
        }
    };
    bexos_userspace::syscall::exit_with_status(status)
}
