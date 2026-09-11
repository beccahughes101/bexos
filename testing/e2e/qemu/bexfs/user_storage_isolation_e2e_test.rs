use bexos_debug_client::DebugClientError;
use bexos_e2e::{BEXFS_MARKERS, E2eDevice, NVME_MARKERS, boot_markers};
use bexos_qemu_test::{QemuArtifacts, QemuDevice};
use std::time::Duration;

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let (artifacts, extra) = QemuArtifacts::from_env_or_args(&args)?;
    let archive = extra
        .last()
        .ok_or("missing user storage probe archive argument")?;
    let mut markers = boot_markers();
    markers.extend_from_slice(NVME_MARKERS);
    markers.extend_from_slice(BEXFS_MARKERS);
    markers.push(b"appd: app lifecycle registry ready for debugd");
    let mut device = QemuDevice::new(artifacts)?;
    let mut session = device.boot_with_debugd(&markers)?;
    session.assert_debugd_ready()?;

    let outcome = (|| -> Result<(), String> {
        let archive = std::fs::read(archive).map_err(|e| format!("read probe archive: {e}"))?;
        session
            .client
            .install_app_bundle(0x7573_7467, &archive)
            .map_err(|e| format!("install user storage probe: {e:?}"))?;
        eprintln!("e2e: installed user storage probe");
        session
            .client
            .create_user(1000, "alice", "Alice", "correct horse")
            .map_err(|e| format!("create user 1000: {e:?}"))?;
        eprintln!("e2e: created user 1000");
        session
            .client
            .unlock_user(1000, "correct horse")
            .map_err(|e| format!("unlock user 1000: {e:?}"))?;
        eprintln!("e2e: unlocked user 1000");
        session
            .client
            .launch_app(
                "com.example.user_storage_probe",
                "user_storage_probe",
                0,
                1000,
            )
            .map_err(|e| format!("launch probe as user 1000: {e:?}"))?;
        eprintln!("e2e: launched user storage probe");
        session
            .wait_for_serial_markers(
                &[b"user-storage-probe: isolated user data written and reread"],
                Duration::from_secs(180),
            )
            .map_err(|e| format!("probe marker: {e}"))?;
        session
            .client
            .lock_user(1000)
            .map_err(|e| format!("lock user 1000: {e:?}"))?;
        match session.client.launch_app(
            "com.example.user_storage_probe",
            "user_storage_probe",
            0,
            1000,
        ) {
            Err(DebugClientError::RemoteStatus(status)) if status.status == -10 => {}
            other => {
                return Err(format!(
                    "locked user launch should be denied, got {other:?}"
                ));
            }
        }
        match session.client.launch_app(
            "com.example.user_storage_probe",
            "user_storage_probe",
            0,
            1001,
        ) {
            Err(DebugClientError::RemoteStatus(status)) if status.status == -10 => {}
            other => {
                return Err(format!(
                    "different locked user launch should be denied, got {other:?}"
                ));
            }
        }
        Ok(())
    })();
    outcome.map_err(|error| {
        format!(
            "{error}\n{}",
            String::from_utf8_lossy(session.client.received_trace())
        )
    })
}
