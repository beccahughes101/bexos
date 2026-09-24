mod checks;
mod qmp;
mod scenarios;
use bexos_debug_client::DebugTransport;
use bexos_e2e::{
    BEXFS_MARKERS, DebugSession, E2eContext, E2eDevice, NVME_MARKERS, VIRTIO_NET_MARKERS,
    boot_markers,
};
use bexos_qemu_test::{QemuArtifacts, QemuDevice};
use std::time::Duration;
fn main() {
    if let Err(e) = run() {
        eprintln!("sysui acceptance: {e}");
        std::process::exit(1);
    }
}
fn run() -> Result<(), String> {
    let (mut artifacts, _) =
        QemuArtifacts::from_env_or_args(&std::env::args().skip(1).collect::<Vec<_>>())?;
    let output = std::env::var_os("TEST_UNDECLARED_OUTPUTS_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    // Q35 secure-product policy authenticates the workstation devices at
    // slots 7-9. The AArch64 harness already reserves slot 7 for its RPMB
    // serial bridge, so keep the same device identities at free virt slots.
    let (gpu_slot, keyboard_slot, tablet_slot) =
        if std::env::var("BEXOS_QEMU_ARCH").as_deref() == Ok("x86_64") {
            (7, 8, 9)
        } else {
            (10, 11, 12)
        };
    artifacts.extra_args = vec![
        "-vga".into(),
        "none".into(),
        "-device".into(),
        format!(
            "virtio-gpu-pci,addr={gpu_slot},disable-legacy=on,disable-modern=off,iommu_platform=on,xres=800,yres=600"
        ),
        "-device".into(),
        format!(
            "virtio-keyboard-pci,addr={keyboard_slot},disable-legacy=on,disable-modern=off,iommu_platform=on"
        ),
        "-device".into(),
        format!(
            "virtio-tablet-pci,addr={tablet_slot},disable-legacy=on,disable-modern=off,iommu_platform=on"
        ),
        "-qmp".into(),
        "unix:s.sock,server=on,wait=off".into(),
    ];
    let context = E2eContext::from_env("sysui");
    let mut device = context.run_phase("stage-qemu", || QemuDevice::new(artifacts))?;
    let qmp_path = device.temporary_path("s.sock")?;
    let mut markers = boot_markers();
    markers.extend_from_slice(NVME_MARKERS);
    markers.extend_from_slice(BEXFS_MARKERS);
    markers.extend_from_slice(VIRTIO_NET_MARKERS);
    markers.push(b"appd: app lifecycle registry ready for debugd");
    markers.push(b"fontd: ready");
    markers.push(b"scened: ready background presented");
    let mut session = context.run_phase("boot", || device.boot_with_debugd(&markers))?;
    context.run_phase("debugd-ready", || session.assert_debugd_ready())?;
    let smoke_only = std::env::var("BEXOS_SYSUI_SMOKE_ONLY").as_deref() == Ok("1");
    let migration_only = std::env::var("BEXOS_SYSUI_MIGRATION_ONLY").as_deref() == Ok("1");
    let recovery_only = std::env::var("BEXOS_SYSUI_RECOVERY_ONLY").as_deref() == Ok("1");
    let preferences_only = std::env::var("BEXOS_SYSUI_PREFERENCES_ONLY").as_deref() == Ok("1");
    let result = (|| -> Result<(), String> {
        let mut q = qmp::Qmp::connect(&qmp_path, output.clone())?;
        context.run_phase("sysui-ready", || {
            checks::cold_process(&mut session.client, "bexos.app.sysui")?;
            checks::cold_screen(&mut session.client, &mut q, "setup", 10, 10, [18, 26, 42])
        })?;
        if migration_only {
            return scenarios::login_screen_migration(&mut session.client, &mut q);
        }
        if recovery_only || smoke_only || preferences_only {
            context.run_phase("provision-login", || {
                session
                    .client
                    .create_user(1000, "alice", "Alice", "testpass")
                    .map_err(|error| format!("scenario user provision: {error:?}"))?;
                checks::login(&mut session.client, &mut q, 1000)
            })?;
            if recovery_only {
                return scenarios::recovery(&mut session.client, &mut q);
            }
            if smoke_only {
                context.run_phase("window-smoke", || {
                    scenarios::smoke(&mut session.client, &mut q)
                })?;
                context.run_phase("presubmit-platform", || {
                    verify_presubmit_platform(&mut session, &mut q)
                })?;
                return Ok(());
            }
            return scenarios::preferences(&mut session.client, &mut q);
        }
        // Even an operator-launched ordinary view must remain below the secure root.
        session
            .client
            .launch_app("bexos.test.second", "dioxus_demo", 0, 0)
            .map_err(|e| format!("ordinary launch {e:?}"))?;
        session.wait_for_serial_markers(
            &[b"sysui-probe: first frame submitted"],
            Duration::from_secs(120),
        )?;
        checks::screen(
            &mut session.client,
            &mut q,
            "ordinary-hidden",
            10,
            10,
            [18, 26, 42],
        )?;
        let ordinary = checks::process(&mut session.client, "bexos.test.second", true)?;
        if session
            .client
            .launch_app("bexos.test.sysui", "sysui", 0x5348454c4c, 0)
            .is_ok()
        {
            return Err("external launch forged a shell grant".into());
        }
        eprintln!("sysui: provision user");
        session
            .client
            .create_user(1000, "alice", "Alice", "testpass")
            .map_err(|e| format!("provision: {e:?}"))?;
        q.screenshot("login")?;
        eprintln!("sysui: incorrect password");
        checks::bad_login(&mut session.client, &mut q, 1000)?;
        checks::screen(
            &mut session.client,
            &mut q,
            "ordinary-stays-hidden",
            10,
            10,
            [18, 26, 42],
        )?;
        if session
            .client
            .received_trace()
            .windows(b"sysui-probe: input received".len())
            .any(|w| w == b"sysui-probe: input received")
        {
            return Err("ordinary application received secure-screen input".into());
        }
        // Keep this UID-0 view unattached throughout the session. Killing a
        // service directly exercises automatic crash recovery, not app close.
        eprintln!("sysui: authenticate");
        checks::login(&mut session.client, &mut q, 1000)?;
        scenarios::windows(&mut session.client, &mut q)?;
        checks::logout(&mut session.client, &mut q, 1000)?;
        checks::only_process(&mut session.client, "bexos.test.second", ordinary)?;
        Ok(())
    })();
    if result.is_err() {
        eprintln!(
            "{}",
            String::from_utf8_lossy(session.client.received_trace())
        );
    }
    result?;
    if smoke_only || migration_only || recovery_only {
        return Ok(());
    }
    if !preferences_only {
        return Ok(());
    }
    eprintln!("sysui: reboot applies the persisted system selector");
    drop(session);
    let mut session =
        context.run_phase("persistence-reboot", || device.boot_with_debugd(&markers))?;
    context.run_phase("persistence-debugd-ready", || session.assert_debugd_ready())?;
    let result = (|| {
        let mut q = qmp::Qmp::connect(&qmp_path, output)?;
        checks::cold_process(&mut session.client, "bexos.test.sysui")?;
        checks::selection_is(&mut session.client, 0, "sysui_package", "bexos.test.sysui")?;
        checks::login(&mut session.client, &mut q, 1000)?;
        checks::process(&mut session.client, "bexos.test.userui", true)?;
        checks::selection_is(
            &mut session.client,
            1000,
            "userui_package",
            "bexos.test.userui",
        )?;
        Ok(())
    })();
    if result.is_err() {
        eprintln!(
            "{}",
            String::from_utf8_lossy(session.client.received_trace())
        );
    }
    result
}

fn verify_presubmit_platform<T: DebugTransport>(
    session: &mut DebugSession<T>,
    q: &mut qmp::Qmp,
) -> Result<(), String> {
    let apps = session
        .client
        .list_apps()
        .map_err(|error| format!("presubmit app registry: {error:?}"))?;
    for package in [
        "bexos.platform.storage_verify",
        "bexos.service.netstackd",
        "bexos.service.timed",
    ] {
        let app = apps
            .iter()
            .find(|app| app.package_id == package)
            .ok_or_else(|| format!("presubmit registry missing {package}"))?;
        if app.state != "Running" || !app.protected {
            return Err(format!(
                "presubmit protected app had unexpected state: {app:?}"
            ));
        }
    }
    if session
        .client
        .launch_app("bexos.test.sysui", "sysui", 0x5348454c4c, 0)
        .is_ok()
    {
        return Err("presubmit external launch forged a protected shell grant".into());
    }
    let mut trace = session.traced_test("presubmit", bexos_trace::CATEGORY_DEBUG_SERVICE)?;
    trace
        .session()
        .client
        .health_check()
        .map_err(|error| format!("presubmit traced health: {error:?}"))?;
    trace
        .finish()?
        .assert_event_present("debugd:health_check")?;
    checks::transplant(
        &mut session.client,
        "bexos.platform.appd",
        104,
        "appd-shell",
        true,
    )?;
    session.assert_debugd_ready()?;
    checks::user(&mut session.client, 1000, true)?;
    checks::screen(
        &mut session.client,
        q,
        "presubmit-after-appd",
        50,
        35,
        [60, 92, 136],
    )?;
    Ok(())
}
