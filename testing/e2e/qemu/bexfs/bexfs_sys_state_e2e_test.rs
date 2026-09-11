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
    let artifacts = QemuArtifacts::from_args(&args)?;
    let mut markers = boot_markers()
        .iter()
        .copied()
        .filter(|marker| *marker != b"reclaimed physical pages reused=")
        .collect::<Vec<_>>();
    markers.extend_from_slice(NVME_MARKERS);
    markers.extend_from_slice(BEXFS_MARKERS);
    let mut device = QemuDevice::new(artifacts)?;
    let mut first_markers = markers.clone();
    first_markers.extend_from_slice(&[
        b"appd: launched preinstalled service package=bexos.service.netstackd".as_slice(),
        b"appd: launched preinstalled service package=bexos.service.timed".as_slice(),
        b"appd: registry launched signed app package=bexos.platform.storage_verify".as_slice(),
    ]);
    let first = traced_first_boot(&mut device, &first_markers)?;
    let first_text = String::from_utf8_lossy(&first);
    if !first_text.contains("generation=2 prior=1") {
        return Err("first boot did not advance SYS_STATE to generation 2".into());
    }
    if !first_text
        .contains("appd: registry launched signed app package=bexos.platform.storage_verify")
    {
        return Err(
            "first boot did not launch the signed disk verifier archive through the registry"
                .into(),
        );
    }
    if !first_text.contains("netstackd: service ready")
        || !first_text
            .contains("appd: launched preinstalled service package=bexos.service.netstackd")
    {
        return Err("first boot did not launch the preinstalled netstackd service".into());
    }
    if !first_text.contains("timed: ready")
        || !first_text.contains("appd: launched preinstalled service package=bexos.service.timed")
    {
        return Err("first boot did not launch the preinstalled timed service".into());
    }
    let second = device.boot(&markers)?;
    let text = String::from_utf8_lossy(&second);
    if !text.contains("generation=3 prior=2") {
        return Err("second boot did not advance SYS_STATE to generation 3".into());
    }
    device
        .inspect_path(
            "SYS_STATE",
            "SYS_STATE",
            "boot_state.bin",
            &["--sys-state", "--expect-generation", "3"],
        )
        .unwrap();
    Ok(())
}

fn traced_first_boot(device: &mut QemuDevice, markers: &[&[u8]]) -> Result<Vec<u8>, String> {
    let boot_seconds = std::env::var("BEXOS_QEMU_TIMEOUT_SECONDS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(60);
    let deadline = Instant::now() + Duration::from_secs(boot_seconds);
    let mut session = device.boot_with_debugd(&[
        b"debugd: QEMU socket transport ready".as_slice(),
        b"traced: ready".as_slice(),
    ])?;
    session.assert_debugd_ready()?;
    let mut traced = session.traced_test(
        "bexfs_sys_state_first_boot",
        bexos_trace::CATEGORY_DEBUG_SERVICE,
    )?;
    // A fast guest can satisfy all boot markers before tracing starts. Emit
    // the asserted operation within the session rather than relying on polling.
    traced
        .session()
        .client
        .health_check()
        .map_err(|error| format!("{error:?}"))?;
    // Early debug readiness and the late storage pivot share the configured
    // cold-boot deadline, including the work needed to start tracing.
    let remaining = deadline
        .checked_duration_since(Instant::now())
        .ok_or("cold-boot deadline expired before storage readiness")?;
    let session = traced.session();
    let output = session
        .wait_for_serial_markers(markers, remaining)
        .map_err(|error| {
            format!(
                "{error}\n{}",
                String::from_utf8_lossy(session.client.received_trace())
            )
        })?;
    let analysis = traced.finish()?;
    analysis.assert_event_present("debugd:health_check")?;
    Ok(output)
}
