//! Keep a macOS VM responsive while its window is covered or the screen locked.
//! The assertion belongs to this QEMU PID and ends when the managed child exits.

pub fn configure(command: &mut std::process::Command) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let path = super::resolve_runfile_path(env!("QEMU_HOST_ACTIVITY_LIBRARY"))
            .canonicalize()
            .map_err(|e| format!("resolve QEMU host activity library: {e}"))?;
        let mut libraries = path.into_os_string();
        if let Some(existing) = std::env::var_os("DYLD_INSERT_LIBRARIES") {
            libraries.push(":");
            libraries.push(existing);
        }
        command.env("DYLD_INSERT_LIBRARIES", libraries);
    }
    #[cfg(not(target_os = "macos"))]
    let _ = command;
    Ok(())
}
