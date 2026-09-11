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
    let (artifacts, extra) = QemuArtifacts::from_env_or_args(&args)?;
    let job_test_archive = extra.last().ok_or("missing job test archive argument")?;
    let mut markers = boot_markers();
    markers.extend_from_slice(NVME_MARKERS);
    markers.extend_from_slice(BEXFS_MARKERS);
    markers.push(b"appd: app lifecycle registry ready for debugd");
    let mut device = QemuDevice::new(artifacts)?;
    let mut session = device.boot_with_debugd(&markers)?;
    session.assert_debugd_ready()?;

    let apps = session.client.list_apps().map_err(|e| format!("{e:?}"))?;
    let jobd = apps
        .iter()
        .find(|app| app.package_id == "bexos.service.jobd")
        .ok_or("debugd app list did not include jobd")?;
    if jobd.state != "Running" || !jobd.protected {
        return Err(format!("jobd app had unexpected state: {jobd:?}"));
    }

    let archive = std::fs::read(job_test_archive).map_err(|e| format!("read test archive: {e}"))?;
    session
        .client
        .install_app_bundle(0x6170_7073, &archive)
        .map_err(|e| format!("install signed job test bundle: {e:?}"))?;
    let apps = session.client.list_apps().map_err(|e| format!("{e:?}"))?;
    if !apps
        .iter()
        .any(|app| app.package_id == "com.example.job_e2e" && !app.protected)
    {
        return Err(format!(
            "installed job test app missing from app list: {apps:?}"
        ));
    }
    match session.client.uninstall_app("com.example.job_e2e") {
        Ok(()) => {}
        Err(DebugClientError::RemoteStatus(status)) if status.status == -20 => {}
        Err(e) => return Err(format!("uninstall signed job test bundle: {e:?}")),
    }
    Ok(())
}
