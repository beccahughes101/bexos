use crate::{args::*, input, output};
use bexos_debug_client::UnixDebugClient;
use serde_json::json;
use std::path::Path;
mod apps;
mod config;
mod inspection;
mod preferences;
mod tee;
mod trace;
mod updates;
mod users;
pub type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;
#[derive(Debug)]
pub struct RemoteExit(pub i32);
impl std::fmt::Display for RemoteExit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "remote command exited {}", self.0)
    }
}
impl std::error::Error for RemoteExit {}
pub fn read(path: &Path) -> Result<Vec<u8>> {
    std::fs::read(path).map_err(|e| format!("read {}: {e}", path.display()).into())
}
pub fn write(path: &Path, bytes: &[u8]) -> Result {
    std::fs::write(path, bytes).map_err(|e| format!("write {}: {e}", path.display()).into())
}
pub fn exec(c: &mut UnixDebugClient, name: &str, args: &[String]) -> Result {
    use std::io::Write;
    let r = c.exec_command(name, args)?;
    std::io::stdout().write_all(r.stdout.as_bytes())?;
    std::io::stderr().write_all(r.stderr.as_bytes())?;
    if r.exit_code != 0 {
        return Err(Box::new(RemoteExit(r.exit_code)));
    }
    Ok(())
}
pub(super) fn diagnostic_record(
    c: &mut UnixDebugClient,
    name: &str,
    args: &[String],
    format: Format,
) -> Result {
    let result = c.exec_command(name, args)?;
    if result.exit_code != 0 {
        use std::io::Write;
        std::io::stderr().write_all(result.stderr.as_bytes())?;
        return Err(Box::new(RemoteExit(result.exit_code)));
    }
    output::records(
        format,
        &[("diagnostic", "DIAGNOSTIC"), ("result", "RESULT")],
        vec![json!({"diagnostic": name, "result": result.stdout.trim_end()})],
    )?;
    Ok(())
}
pub fn run(c: &mut UnixDebugClient, cmd: Command, f: Format) -> Result {
    match cmd {
        Command::Health => {
            let h = c.health_check()?;
            output::records(
                f,
                &[
                    ("service", "SERVICE"),
                    ("status", "STATUS"),
                    ("version", "VERSION"),
                ],
                vec![json!({"service":h.service_name,"status":h.status,"version":h.version})],
            )?;
        }
        Command::Ps { wide } => inspection::processes(c, wide, f)?,
        Command::Config { command } => config::run(c, command, f)?,
        Command::Prefs { command } => preferences::run(c, command, f)?,
        Command::Users { command } => users::run(c, command, f)?,
        Command::Tee { command } => tee::run(c, command, f)?,
        Command::Trace { command } => trace::run(c, command, f)?,
        Command::Update { command } => updates::run(c, command, f)?,
        Command::Kernel {
            command: KernelCommand::UpdateStatus,
        } => diagnostic_record(c, "kernel.update.status", &[], f)?,
        Command::Exec { component, args } => exec(c, &component, &args)?,
        Command::Shell(args) => crate::terminal::run(c, args)?,
        Command::Completions { .. } => unreachable!("offline command"),
        other => apps::run(c, other, f)?,
    }
    Ok(())
}
