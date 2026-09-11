use bexos_e2e::{E2eDevice, boot_markers};
use bexos_qemu_test::{QemuArtifacts, QemuDevice};
fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
fn run() -> Result<(), String> {
    let (artifacts, extra) =
        QemuArtifacts::from_env_or_args(&std::env::args().skip(1).collect::<Vec<_>>())?;
    let archive = std::fs::read(extra.first().ok_or("missing fault probe archive")?)
        .map_err(|e| e.to_string())?;
    let mut device = QemuDevice::new(artifacts)?;
    let mut session = device.boot_with_debugd(&boot_markers())?;
    session.assert_debugd_ready()?;
    session
        .client
        .install_app_bundle(0x4641554c, &archive)
        .map_err(|e| {
            format!(
                "install fault probe: {e:?}\n{}",
                String::from_utf8_lossy(session.client.received_trace())
            )
        })?;
    for (mode, process) in [(1, "readonly"), (2, "nonexecuting"), (3, "unmapped")] {
        session.client.clear_received_trace();
        session
            .client
            .launch_app("bexos.platform.fault_probe", process, mode, 0)
            .map_err(|e| {
                format!(
                    "launch {process}: {e:?}\n{}",
                    String::from_utf8_lossy(session.client.received_trace())
                )
            })?;
        // The general boot helper intentionally rejects every guest fault.
        // This fixture expects one, then checks the surviving control plane.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            session
                .client
                .health_check()
                .map_err(|e| format!("poll {process} fault: {e:?}"))?;
            let trace = String::from_utf8_lossy(session.client.received_trace());
            if trace.contains("panic") {
                return Err(format!("panic handling {process}: {trace}"));
            }
            if trace.contains("fault-probe: executing prohibited access")
                && trace.contains("guest fault")
            {
                break;
            }
            if std::time::Instant::now() >= deadline {
                return Err(format!("missing {process} fault: {trace}"));
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        if String::from_utf8_lossy(session.client.received_trace())
            .contains("ERROR prohibited access returned")
        {
            return Err(format!("{process} page permissions were not enforced"));
        }
        let health = session
            .client
            .health_check()
            .map_err(|e| format!("health after {process} fault: {e:?}"))?;
        if health.status != "SERVING" {
            return Err(format!("kernel stopped serving after {process} fault"));
        }
        eprintln!("e2e: {process} fault isolated; kernel and debugd still serving");
    }
    Ok(())
}
