use crate::{checks as c, qmp::Qmp};
use bexos_debug_client::{DebugClient, DebugTransport};
use std::time::Duration;

pub fn login_screen_migration(
    client: &mut DebugClient<impl DebugTransport>,
    q: &mut Qmp,
) -> Result<(), String> {
    eprintln!("sysui: focused login-screen migration and rollback");
    for generation in [101, 102] {
        c::transplant(
            client,
            "bexos.app.sysui",
            generation,
            "bexos.app.sysui.replacement",
            true,
        )?;
        c::screen(client, q, "login-transplanted", 10, 10, [18, 26, 42])?;
    }
    let source = c::process(client, "bexos.app.sysui", true)?;
    c::transplant(client, "bexos.app.sysui", 105, "sysui-rejected", false)?;
    if c::process(client, "bexos.app.sysui", true)? != source {
        return Err("failed replacement did not retain the login shell".into());
    }
    c::screen(client, q, "login-rollback", 10, 10, [18, 26, 42])?;
    // Appd stages its own replacement with prefsd frozen. The replacement must
    // thaw the provider before serving selectors and subsequent logins.
    c::transplant(client, "bexos.platform.appd", 106, "appd-shell", true)?;
    c::selection_is(client, 0, "sysui_package", "bexos.app.sysui")?;
    c::screen(client, q, "login-after-appd", 10, 10, [18, 26, 42])?;
    Ok(())
}

pub fn windows(client: &mut DebugClient<impl DebugTransport>, q: &mut Qmp) -> Result<(), String> {
    eprintln!("sysui: launch two applications through the desktop");
    q.click(40, 580)?;
    c::screen(client, q, "launcher", 2, 2, [28, 38, 56])?;
    q.click(100, 36)?;
    c::cold_screen(client, q, "first-window", 50, 35, [60, 92, 136])?;
    q.click(40, 580)?;
    c::screen(client, q, "launcher-second", 2, 2, [28, 38, 56])?;
    q.click(100, 74)?;
    // The first window also covers (75, 63). Use the exposed end of the
    // second title bar so this cannot pass before the second view arrives.
    c::cold_screen(client, q, "two-windows", 610, 63, [60, 92, 136])?;
    eprintln!("sysui: drag, resize, and focus");
    q.pointer(100, 75, Some(true))?;
    q.pointer(160, 115, None)?;
    q.pointer(160, 115, Some(false))?;
    c::screen(client, q, "dragged", 135, 103, [60, 92, 136])?;
    q.pointer(682, 494, Some(true))?;
    q.pointer(740, 540, None)?;
    q.pointer(740, 540, Some(false))?;
    // Sample inside the enlarged bottom border. Absolute tablet coordinates
    // are quantized; the requested corner can land just outside the window.
    c::screen(client, q, "resized", 720, 535, [60, 92, 136])?;
    q.click(60, 45)?;
    c::screen(client, q, "focused-first", 50, 35, [60, 92, 136])?;
    // Inspect the exposed end of the second title bar, outside the first window.
    c::screen(client, q, "unfocused-second", 620, 103, [43, 56, 76])?;
    // Focus the second app's body, and retain its pixels across credential input.
    q.click(700, 200)?;
    c::screen(client, q, "focused-second", 135, 103, [60, 92, 136])?;
    client
        .drain_for(Duration::from_secs(1))
        .map_err(|e| format!("settle app input {e:?}"))?;
    let before = q.screenshot("before-lock")?;
    q.click(750, 580)?;
    c::user(client, 1000, false)?;
    c::screen(client, q, "locked", 10, 10, [18, 26, 42])?;
    c::bad_login(client, q, 1000)?;
    c::login(client, q, 1000)?;
    c::screen(client, q, "resumed", 135, 103, [60, 92, 136])?;
    let after = q.screenshot("after-lock")?;
    for y in 140..480 {
        for x in 140..730 {
            if crate::qmp::pixel(&before, x, y) != crate::qmp::pixel(&after, x, y) {
                return Err(format!(
                    "application pixels changed during credential entry at {x},{y}"
                ));
            }
        }
    }
    eprintln!("sysui: transplant both shells, scened, and appd with windows retained");
    for (package, generation, archive) in [
        ("bexos.app.sysui", 101, "bexos.app.sysui.replacement"),
        ("bexos.app.userui", 102, "bexos.app.userui.replacement"),
        (
            "bexos.service.scened",
            103,
            "bexos.service.scened.replacement",
        ),
        ("bexos.platform.appd", 104, "appd-shell"),
    ] {
        c::transplant(client, package, generation, archive, true)?;
        c::screen(client, q, "transplanted", 135, 103, [60, 92, 136])?;
        c::user(client, 1000, true)?;
    }
    let sys = c::process(client, "bexos.app.sysui", true)?;
    c::transplant(client, "bexos.app.sysui", 105, "sysui-rejected", false)?;
    if c::process(client, "bexos.app.sysui", true)? != sys {
        return Err("failed replacement did not retain SysUI".into());
    }
    c::screen(client, q, "rollback", 135, 103, [60, 92, 136])?;
    // A lock arriving during a title-bar gesture must cancel the capture.
    q.pointer(150, 115, Some(true))?;
    client
        .lock_user(1000)
        .map_err(|e| format!("external lock {e:?}"))?;
    c::screen(client, q, "gesture-locked", 10, 10, [18, 26, 42])?;
    q.pointer(350, 200, None)?;
    c::login(client, q, 1000)?;
    q.pointer(500, 300, None)?;
    q.pointer(500, 300, Some(false))?;
    c::screen(client, q, "gesture-cancelled", 135, 103, [60, 92, 136])?;
    q.click(575, 45)?;
    c::process(client, "bexos.app.dioxus_demo", false)?;
    c::screen(client, q, "closed-first", 70, 50, [27, 48, 68])?;
    Ok(())
}

pub fn recovery(client: &mut DebugClient<impl DebugTransport>, q: &mut Qmp) -> Result<(), String> {
    eprintln!("sysui: shell-loss recovery");
    let sys = c::process(client, "bexos.app.sysui", true)?;
    c::kill(client, "bexos.app.sysui", 0)?;
    c::user(client, 1000, false)?;
    c::screen(client, q, "sysui-loss", 10, 10, [18, 26, 42])?;
    let replacement = c::process(client, "bexos.app.sysui", true)?;
    if replacement == sys {
        return Err("SysUI was not replaced after process loss".into());
    }
    c::login(client, q, 1000)?;
    let user = c::process(client, "bexos.app.userui", true)?;
    c::kill(client, "bexos.app.userui", 1000)?;
    client
        .drain_for(Duration::from_secs(6))
        .map_err(|e| format!("user shell restart {e:?}"))?;
    if c::process(client, "bexos.app.userui", true)? == user {
        return Err("UserUI was not replaced".into());
    }
    c::cold_screen(client, q, "userui-loss", 10, 10, [27, 48, 68])?;
    Ok(())
}

pub fn preferences(
    client: &mut DebugClient<impl DebugTransport>,
    q: &mut Qmp,
) -> Result<(), String> {
    eprintln!("sysui: per-user selection and invalid-package recovery");
    if c::selection(client, 1000, "sysui_package", "bexos.test.sysui").is_ok() {
        return Err("user preference overrode system-only SysUI selection".into());
    }
    c::selection(client, 1000, "userui_package", "bexos.test.userui")?;
    c::process(client, "bexos.app.userui", true)?;
    q.click(750, 580)?;
    c::user(client, 1000, false)?;
    c::screen(client, q, "lock-screen-logout", 381, 325, [48, 60, 80])?;
    q.click(400, 335)?;
    c::process(client, "bexos.app.userui", false)?;
    c::process(client, "bexos.test.second", false)?;
    c::login(client, q, 1000)?;
    c::process(client, "bexos.test.userui", true)?;
    c::selection_is(client, 1000, "userui_package", "bexos.test.userui")?;
    // Recovery must preserve the selected package while its UID is locked.
    q.click(750, 580)?;
    c::user(client, 1000, false)?;
    c::kill(client, "bexos.test.userui", 1000)?;
    client
        .drain_for(Duration::from_secs(6))
        .map_err(|e| format!("locked desktop recovery {e:?}"))?;
    c::process(client, "bexos.test.userui", false)?;
    c::process(client, "bexos.app.userui", false)?;
    c::login(client, q, 1000)?;
    c::process(client, "bexos.test.userui", true)?;
    c::logout(client, q, 1000)?;
    client
        .create_user(1001, "bob", "Bob", "testpass")
        .map_err(|e| format!("second account {e:?}"))?;
    client
        .drain_for(Duration::from_secs(2))
        .map_err(|e| format!("picker {e:?}"))?;
    q.key("tab")?;
    c::login(client, q, 1001)?;
    c::process(client, "bexos.app.userui", true)?;
    c::selection_is(client, 1001, "userui_package", "bexos.app.userui")?;
    c::selection(client, 1001, "userui_package", "missing.desktop")?;
    c::logout(client, q, 1001)?;
    c::login(client, q, 1001)?;
    c::process(client, "bexos.app.userui", true)?;
    c::selection_is(client, 1001, "userui_package", "missing.desktop")?;
    let diagnostic = b"appd: shell: Unable to launch missing.desktop; trying bundled shell.";
    if !client
        .received_trace()
        .windows(diagnostic.len())
        .any(|w| w == diagnostic)
    {
        return Err("invalid shell selection did not report its recovery diagnostic".into());
    }
    let sys = c::process(client, "bexos.app.sysui", true)?;
    c::selection(client, 0, "sysui_package", "bexos.test.sysui")?;
    if c::process(client, "bexos.app.sysui", true)? != sys {
        return Err("system selection changed before reboot".into());
    }
    c::logout(client, q, 1001)?;
    Ok(())
}
