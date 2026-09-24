use bexos_e2e::E2eDevice;
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
    let archive = std::fs::read(extra.first().ok_or("missing restricted probe archive")?)
        .map_err(|error| error.to_string())?;
    let mut device = QemuDevice::new(artifacts)?;
    let mut session =
        device.boot_with_debugd(&[b"appd: app lifecycle registry ready for debugd"])?;
    session.assert_debugd_ready()?;
    session
        .client
        .install_app_bundle(0x52535452, &archive)
        .map_err(|error| format!("install restricted probe: {error:?}"))?;
    session.client.clear_received_trace();
    session
        .client
        .launch_app("bexos.platform.restricted_probe", "probe", 0, 0)
        .map_err(|error| format!("launch restricted probe: {error:?}"))?;

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        session
            .client
            .health_check()
            .map_err(|error| format!("restricted probe health: {error:?}"))?;
        let trace = String::from_utf8_lossy(session.client.received_trace());
        if trace.contains("guest panic") || trace.contains("kernel panic") {
            return Err(format!("restricted probe panic: {trace}"));
        }
        if trace.contains("restricted-probe: PASS syscall exception kick") {
            break;
        }
        if std::time::Instant::now() >= deadline {
            return Err(format!("restricted probe timed out: {trace}"));
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let health = session
        .client
        .health_check()
        .map_err(|error| format!("kernel health after restricted probe: {error:?}"))?;
    if health.status != "SERVING" {
        return Err("kernel stopped serving after restricted probe".into());
    }
    Ok(())
}
