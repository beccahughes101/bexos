use bexos_component_config::{ConfigType, encode_config};
use bexos_debug_client::{DebugClient, DebugTransport};
use bexos_debug_wire::PreferencesRequest;
use std::time::{Duration, Instant};
fn cold_timeout() -> Duration {
    Duration::from_secs(360)
}
pub fn screen(
    c: &mut DebugClient<impl DebugTransport>,
    q: &mut crate::qmp::Qmp,
    name: &str,
    x: usize,
    y: usize,
    color: [u8; 3],
) -> Result<Vec<u8>, String> {
    screen_wait(c, q, name, x, y, color, Duration::from_secs(120))
}
pub fn cold_screen(
    c: &mut DebugClient<impl DebugTransport>,
    q: &mut crate::qmp::Qmp,
    name: &str,
    x: usize,
    y: usize,
    color: [u8; 3],
) -> Result<Vec<u8>, String> {
    screen_wait(c, q, name, x, y, color, cold_timeout())
}
fn screen_wait(
    c: &mut DebugClient<impl DebugTransport>,
    q: &mut crate::qmp::Qmp,
    name: &str,
    x: usize,
    y: usize,
    color: [u8; 3],
    timeout: Duration,
) -> Result<Vec<u8>, String> {
    let deadline = Instant::now() + timeout;
    let stall_timeout = Duration::from_secs(120);
    let mut progressed_at = Instant::now();
    let mut generation = 0;
    let mut previous_pixel = None;
    loop {
        let image = q.screenshot(name)?;
        let current_pixel = crate::qmp::pixel(&image, x, y);
        if current_pixel == Some(color) {
            return Ok(image);
        }
        let trace = c.received_trace();
        let current_generation = [
            b"user operation complete".as_slice(),
            b"localed: preferences pending",
            b"appd: loading user permissions",
            b"appd: process ready package=bexos.app.userui",
            b"wasm_runner: native UI first frame submitted",
        ]
        .iter()
        .filter(|marker| trace.windows(marker.len()).any(|window| window == **marker))
        .count();
        if current_generation != generation || previous_pixel != current_pixel {
            generation = current_generation;
            previous_pixel = current_pixel;
            progressed_at = Instant::now();
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "{name}: pixel {x},{y} expected {color:?}, got {:?}",
                current_pixel
            ));
        }
        if progressed_at.elapsed() >= stall_timeout {
            return Err(format!(
                "{name}: stalled for {stall_timeout:?} at progress generation {generation}; pixel {x},{y} expected {color:?}, got {current_pixel:?}"
            ));
        }
        c.drain_for(Duration::from_millis(500))
            .map_err(|e| format!("screen wait {e:?}"))?;
    }
}
pub fn bad_login(
    c: &mut DebugClient<impl DebugTransport>,
    q: &mut crate::qmp::Qmp,
    uid: u64,
) -> Result<(), String> {
    q.type_text("wrong")?;
    q.key("ret")?;
    // Wait for the UI to report the completed authentication failure, rather
    // than merely observing that the account was still locked during the RPC.
    screen(c, q, "authentication-error", 185, 378, [255, 166, 158])?;
    user(c, uid, false)?;
    Ok(())
}
pub fn login(
    c: &mut DebugClient<impl DebugTransport>,
    q: &mut crate::qmp::Qmp,
    uid: u64,
) -> Result<(), String> {
    cold_screen(c, q, "before-login", 10, 10, [18, 26, 42])?;
    q.type_text("testpass")?;
    q.key("ret")?;
    user(c, uid, true)?;
    // The desktop and secure screen deliberately share the main background.
    // The bottom taskbar is a desktop-only rendered state transition.
    cold_screen(c, q, "desktop", 400, 580, [18, 25, 38])?;
    Ok(())
}
pub fn logout(
    c: &mut DebugClient<impl DebugTransport>,
    q: &mut crate::qmp::Qmp,
    uid: u64,
) -> Result<(), String> {
    q.click(640, 580)?;
    user(c, uid, false)?;
    screen(c, q, "logout", 10, 10, [18, 26, 42])?;
    Ok(())
}
pub fn selection_is(
    c: &mut DebugClient<impl DebugTransport>,
    uid: u64,
    field: &str,
    value: &str,
) -> Result<(), String> {
    // ConfigGet exposes the raw operator overlay, which may be empty. Read
    // resolved preferences for both UID 0 defaults and per-user selections.
    let response = c
        .preferences(&PreferencesRequest {
            package_id: "bexos.platform.appd".into(),
            uid,
            ..Default::default()
        })
        .map_err(|e| format!("read selection {e:?}"))?;
    if response.status != 0 {
        return Err(format!("read selection: {}", response.message));
    }
    let table = bexos_component_config::ConfigTable::parse(&response.config)
        .map_err(|e| format!("selection config {e:?}"))?;
    if table
        .get_string(field)
        .map_err(|e| format!("selection field {e:?}"))?
        != value
    {
        return Err(format!("selection {field} did not retain {value}"));
    }
    Ok(())
}
pub fn user(
    c: &mut DebugClient<impl DebugTransport>,
    uid: u64,
    unlocked: bool,
) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(120);
    while Instant::now() < deadline {
        if c.get_user(uid).map_err(|e| format!("user {e:?}"))?.unlocked == unlocked {
            return Ok(());
        }
        c.drain_for(Duration::from_millis(250))
            .map_err(|e| format!("user wait {e:?}"))?;
    }
    Err(format!("UID {uid} did not reach unlocked={unlocked}"))
}
pub fn process(
    c: &mut DebugClient<impl DebugTransport>,
    package: &str,
    present: bool,
) -> Result<u64, String> {
    process_wait(c, package, present, Duration::from_secs(120))
}
pub fn cold_process(
    c: &mut DebugClient<impl DebugTransport>,
    package: &str,
) -> Result<u64, String> {
    process_wait(c, package, true, cold_timeout())
}
pub fn only_process(
    c: &mut DebugClient<impl DebugTransport>,
    package: &str,
    pid: u64,
) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(120);
    while Instant::now() < deadline {
        let running = c
            .list_processes()
            .map_err(|e| format!("processes {e:?}"))?
            .into_iter()
            .filter(|p| p.package_id == package && p.state == "Running")
            .map(|p| p.pid)
            .collect::<Vec<_>>();
        if running == [pid] {
            return Ok(());
        }
        c.drain_for(Duration::from_millis(250))
            .map_err(|e| format!("process wait {e:?}"))?;
    }
    Err(format!("logout did not leave only the system probe {pid}"))
}
fn process_wait(
    c: &mut DebugClient<impl DebugTransport>,
    package: &str,
    present: bool,
    timeout: Duration,
) -> Result<u64, String> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let p = c
            .list_processes()
            .map_err(|e| format!("processes {e:?}"))?
            .into_iter()
            .find(|p| p.package_id == package && p.state == "Running");
        if p.is_some() == present {
            return Ok(p.map_or(0, |p| p.pid));
        }
        c.drain_for(Duration::from_millis(250))
            .map_err(|e| format!("process wait {e:?}"))?;
    }
    Err(format!("package {package} did not reach present={present}"))
}
pub fn selection(
    c: &mut DebugClient<impl DebugTransport>,
    uid: u64,
    field: &str,
    value: &str,
) -> Result<(), String> {
    let bytes = encode_config(&[(field, ConfigType::String, value.as_bytes())]);
    if uid == 0 {
        let current = c
            .get_component_config("bexos.platform.appd")
            .map_err(|e| format!("config get {e:?}"))?;
        c.set_component_config("bexos.platform.appd", current.generation, &bytes)
            .map_err(|e| format!("config set {e:?}"))?;
    } else {
        let mut q = PreferencesRequest {
            package_id: "bexos.platform.appd".into(),
            uid,
            ..Default::default()
        };
        let current = c.preferences(&q).map_err(|e| format!("prefs get {e:?}"))?;
        if current.status != 0 {
            return Err(format!("prefs get {}", current.status));
        }
        q.operation = 1;
        q.expected_generation = current.generation;
        q.config = bytes;
        let r = c.preferences(&q).map_err(|e| format!("prefs set {e:?}"))?;
        if !matches!(r.status, 0 | 1) {
            return Err(format!("prefs set {}", r.status));
        }
    }
    Ok(())
}
pub fn transplant(
    c: &mut DebugClient<impl DebugTransport>,
    package: &str,
    generation: u64,
    archive: &str,
    success: bool,
) -> Result<(), String> {
    let r = c
        .exec_command(
            "update.apply_stored_service",
            &[package.into(), generation.to_string(), archive.into()],
        )
        .map_err(|e| format!("transplant {package} {e:?}"))?;
    if r.exit_code != 0 {
        // A rollback assertion must exercise an accepted replacement attempt,
        // rather than passing because staging never found its archive.
        return Err(format!("transplant staging failed {r:?}"));
    }
    let deadline = Instant::now() + cold_timeout();
    while Instant::now() < deadline {
        let status = c
            .exec_command("update.service.status", &[package.into()])
            .map_err(|e| format!("transplant status {e:?}"))?;
        if status.stdout.contains("pending=false") {
            let completed = status.stdout.contains("migration completed");
            return if success
                && completed
                && status.stdout.contains(&format!("generation={generation} "))
            {
                Ok(())
            } else if success {
                Err(format!("transplant failed {status:?}"))
            } else if completed {
                Err("rejected shell unexpectedly replaced the source".into())
            } else {
                Ok(())
            };
        }
        c.drain_for(Duration::from_millis(500))
            .map_err(|e| format!("transplant wait {e:?}"))?;
    }
    Err(format!("transplant timeout {package}"))
}
pub fn kill(
    c: &mut DebugClient<impl DebugTransport>,
    package: &str,
    uid: u64,
) -> Result<(), String> {
    let response = c
        .exec_command(
            "shell.terminate_selected",
            &[package.to_string(), uid.to_string()],
        )
        .map_err(|error| format!("selected shell termination {error:?}"))?;
    if response.exit_code == 0 {
        Ok(())
    } else {
        Err(format!("selected shell termination failed: {response:?}"))
    }
}
