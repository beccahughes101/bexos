mod checks;
mod qmp;
mod scenarios;
use bexos_e2e::{E2eDevice, boot_markers};
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
    let qmp_path = std::env::temp_dir().join(format!("sysui-{}.sock", std::process::id()));
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
        format!("unix:{},server=on,wait=off", qmp_path.display()),
    ];
    let mut device = QemuDevice::new(artifacts)?;
    let mut markers = boot_markers();
    markers.push(b"appd: app lifecycle registry ready for debugd");
    markers.push(b"fontd: ready");
    markers.push(b"scened: ready background presented");
    let mut session = device.boot_with_debugd(&markers)?;
    session.assert_debugd_ready()?;
    let migration_only = std::env::var("BEXOS_SYSUI_MIGRATION_ONLY").as_deref() == Ok("1");
    let recovery_only = std::env::var("BEXOS_SYSUI_RECOVERY_ONLY").as_deref() == Ok("1");
    let result = (|| -> Result<(), String> {
        let mut q = qmp::Qmp::connect(&qmp_path, output.clone())?;
        checks::cold_process(&mut session.client, "bexos.app.sysui")?;
        checks::cold_screen(&mut session.client, &mut q, "setup", 10, 10, [18, 26, 42])?;
        if migration_only {
            return scenarios::login_screen_migration(&mut session.client, &mut q);
        }
        if recovery_only {
            session
                .client
                .create_user(1000, "alice", "Alice", "testpass")
                .map_err(|error| format!("recovery user provision: {error:?}"))?;
            std::thread::sleep(Duration::from_secs(3));
            checks::login(&mut session.client, &mut q, 1000)?;
            return scenarios::recovery(&mut session.client, &mut q);
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
        std::thread::sleep(Duration::from_secs(3));
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
    if migration_only || recovery_only {
        return Ok(());
    }
    // The kernel currently has a bounded lifetime process table. Exercise
    // recovery and preference changes on a fresh boot of the same installation.
    eprintln!("sysui: reboot before recovery and preference scenarios");
    drop(session);
    let mut session = device.boot_with_debugd(&markers)?;
    session.assert_debugd_ready()?;
    let result = (|| {
        let mut q = qmp::Qmp::connect(&qmp_path, output.clone())?;
        checks::login(&mut session.client, &mut q, 1000)?;
        scenarios::recovery(&mut session.client, &mut q)?;
        scenarios::preferences(&mut session.client, &mut q)
    })();
    if result.is_err() {
        eprintln!(
            "{}",
            String::from_utf8_lossy(session.client.received_trace())
        );
    }
    result?;
    eprintln!("sysui: reboot applies the system selector");
    drop(session);
    let mut session = device.boot_with_debugd(&markers)?;
    session.assert_debugd_ready()?;
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
