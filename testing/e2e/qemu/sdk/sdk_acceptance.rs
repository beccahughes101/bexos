use bexos_e2e::{BEXFS_MARKERS, E2eDevice, NVME_MARKERS, boot_markers};
use bexos_qemu_test::{QemuArtifacts, QemuDevice};
use std::time::{Duration, Instant};

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let (artifacts, _extra) = QemuArtifacts::from_env_or_args(&args)?;
    let mut markers = boot_markers()
        .into_iter()
        .filter(|marker| !marker.starts_with(b"appd: guest persistence"))
        .collect::<Vec<_>>();
    markers.extend_from_slice(NVME_MARKERS);
    markers.extend_from_slice(BEXFS_MARKERS);
    markers.push(b"sdk-fixture-service: BootFS std/libc+C service started value=42");
    markers.push(b"sdk-fixture-driver: e1000e bound with structured resources");
    let mut device = QemuDevice::new(artifacts)?;
    let mut session = device.boot_with_debugd(&markers)?;
    session.assert_debugd_ready()?;
    let mut boot_output = session.boot_output.clone();
    boot_output.extend_from_slice(session.client.received_trace());
    let boot_trace = String::from_utf8_lossy(&boot_output);
    let early = boot_trace
        .find("sdk-fixture-service: BootFS std/libc+C service started value=42")
        .ok_or("missing early service marker")?;
    let pivot = boot_trace
        .find("appd: pivot complete")
        .ok_or("missing storage pivot marker")?;
    if early >= pivot {
        return Err("external BootFS service did not start before storage pivot".into());
    }
    for (package, generation, archive, marker) in [
        (
            "bexos.sdk.fixture.service",
            201,
            "bexos.sdk.fixture.service.replacement",
            "sdk-fixture-service: replacement active with continuity",
        ),
        (
            "bexos.sdk.fixture.driver.e1000e",
            202,
            "bexos.sdk.fixture.driver.e1000e.replacement",
            "sdk-fixture-driver: replacement active with resource continuity",
        ),
    ] {
        let result = session
            .client
            .exec_command(
                "update.apply_stored_service",
                &[package.into(), generation.to_string(), archive.into()],
            )
            .map_err(|error| format!("stage {package}: {error:?}"))?;
        if result.exit_code != 0 {
            return Err(format!("stage {package}: {result:?}"));
        }
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            let status = session
                .client
                .exec_command("update.service.status", &[package.into()])
                .map_err(|error| format!("status {package}: {error:?}"))?;
            if status.stdout.contains("pending=false") {
                if !status.stdout.contains("migration completed") {
                    return Err(format!("migration failed {package}: {status:?}"));
                }
                break;
            }
            if Instant::now() >= deadline {
                return Err(format!("migration timeout {package}"));
            }
            session
                .client
                .drain_for(Duration::from_millis(250))
                .map_err(|error| format!("trace: {error:?}"))?;
        }
        session.wait_for_serial_markers(
            &[
                marker.as_bytes(),
                b"sdk-fixture: version-1 state and handles adopted",
            ],
            Duration::from_secs(30),
        )?;
    }
    Ok(())
}
