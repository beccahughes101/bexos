use bexos_debug_client::DebugClientError;
use bexos_e2e::{BEXFS_MARKERS, E2eDevice, NVME_MARKERS, boot_markers};
use bexos_qemu_test::{QemuArtifacts, QemuDevice};

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let (mut artifacts, extra) = QemuArtifacts::from_env_or_args(&args)?;
    // The archive is a host-side installation fixture, not a QEMU disk argument.
    artifacts.extra_args.clear();
    let registry_test_archive = extra
        .last()
        .ok_or("missing registry test archive argument")?;
    let mut markers = boot_markers();
    markers.extend_from_slice(NVME_MARKERS);
    markers.extend_from_slice(BEXFS_MARKERS);
    markers.push(b"appd: app lifecycle registry ready for debugd");
    let mut device = QemuDevice::new(artifacts)?;
    let mut session = device.boot_with_debugd(&markers)?;
    session.assert_debugd_ready()?;

    let apps = session.client.list_apps().map_err(|e| format!("{e:?}"))?;
    let storage_verify = apps
        .iter()
        .find(|app| app.package_id == "bexos.platform.storage_verify")
        .ok_or("debugd app list did not include storage verifier")?;
    if storage_verify.state != "Running" || !storage_verify.protected {
        return Err(format!(
            "storage verifier app had unexpected state: {storage_verify:?}"
        ));
    }
    let netstack = apps
        .iter()
        .find(|app| app.package_id == "bexos.service.netstackd")
        .ok_or("debugd app list did not include netstackd")?;
    if netstack.state != "Running" || !netstack.protected {
        return Err(format!("netstack app had unexpected state: {netstack:?}"));
    }
    let timed = apps
        .iter()
        .find(|app| app.package_id == "bexos.service.timed")
        .ok_or("debugd app list did not include timed")?;
    if timed.state != "Running" || !timed.protected {
        return Err(format!("timed app had unexpected state: {timed:?}"));
    }

    match session
        .client
        .uninstall_app("bexos.platform.storage_verify")
    {
        Err(DebugClientError::RemoteStatus(status)) if status.status == -10 => Ok(()),
        other => Err(format!(
            "protected storage verifier uninstall should be denied, got {other:?}"
        )),
    }?;

    let archive =
        std::fs::read(registry_test_archive).map_err(|e| format!("read test archive: {e}"))?;
    session
        .client
        .install_app_bundle(0x6170_7073, &archive)
        .map_err(|e| {
            format!(
                "install signed app bundle: {e:?}\n{}",
                String::from_utf8_lossy(session.client.received_trace())
            )
        })?;
    let apps = session.client.list_apps().map_err(|e| format!("{e:?}"))?;
    if !apps
        .iter()
        .any(|app| app.package_id == "com.example.registry_verify" && !app.protected)
    {
        return Err(format!(
            "installed mutable app missing from app list: {apps:?}"
        ));
    }
    session
        .client
        .uninstall_app("com.example.registry_verify")
        .map_err(|e| {
            format!(
                "uninstall mutable signed app bundle: {e:?}\n{}",
                String::from_utf8_lossy(session.client.received_trace())
            )
        })?;
    let apps = session.client.list_apps().map_err(|e| format!("{e:?}"))?;
    if apps
        .iter()
        .any(|app| app.package_id == "com.example.registry_verify")
    {
        return Err(format!("uninstalled mutable app still listed: {apps:?}"));
    }
    Ok(())
}
