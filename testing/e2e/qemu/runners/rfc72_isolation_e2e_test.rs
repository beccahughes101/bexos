use bexos_e2e::{E2eDevice, boot_markers};
use bexos_qemu_test::{QemuArtifacts, QemuDevice};
use std::time::Duration;

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let (artifacts, extra) =
        QemuArtifacts::from_env_or_args(&std::env::args().skip(1).collect::<Vec<_>>())?;
    if extra.len() != 4 {
        return Err("expected ELF library, ELF probe, Starnix, and provider archives".into());
    }
    let archives = extra
        .iter()
        .map(|path| std::fs::read(path).map_err(|error| error.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    let mut device = QemuDevice::new(artifacts)?;
    let mut session = device.boot_with_debugd(&boot_markers())?;
    session.assert_debugd_ready()?;
    for (index, archive) in archives.iter().take(3).enumerate() {
        session
            .client
            .install_app_bundle(0x7246_4300 + index as u64, archive)
            .map_err(|error| format!("install RFC 72 fixture {index}: {error:?}"))?;
    }

    // Start all execution profiles before waiting for any one of them. The
    // Dioxus instance remains live while the three fault fixtures terminate.
    session
        .client
        .launch_app("bexos.app.dioxus_demo", "dioxus_demo", 0, 0)
        .map_err(|error| format!("launch Dioxus profile: {error:?}"))?;
    session
        .client
        .launch_app("bexos.platform.elf_probe", "elf_probe", 3u64 << 32, 0)
        .map_err(|error| format!("launch native crash: {error:?}"))?;
    session
        .client
        .launch_app("bexos.test.wasm.trap", "trap", 0, 0)
        .map_err(|error| format!("launch WASM crash: {error:?}"))?;
    session
        .client
        .launch_app("bexos.platform.starnix_fixture", "crash", 0, 0)
        .map_err(|error| format!("launch Starnix crash: {error:?}"))?;
    session.wait_for_serial_markers(
        &[
            b"elf-probe: intentional isolated runner crash",
            b"wasm_runner: failed:",
            b"starnix crash fixture: injecting fault",
        ],
        Duration::from_secs(120),
    )?;
    let health = session
        .client
        .health_check()
        .map_err(|error| format!("appd/debugd unavailable after runner crashes: {error:?}"))?;
    if health.status != "SERVING" {
        return Err(format!(
            "appd/debugd unhealthy after runner crashes: {health:?}"
        ));
    }
    let processes = session
        .client
        .list_processes()
        .map_err(|error| format!("process list after runner crashes: {error:?}"))?;
    if !processes
        .iter()
        .any(|process| process.package_id == "bexos.app.dioxus_demo" && process.state == "Running")
    {
        return Err(format!(
            "unrelated Dioxus component was lost: {processes:?}"
        ));
    }

    session
        .client
        .launch_app("bexos.test.wasm.service", "service", 0, 0)
        .map_err(|error| format!("launch provider-rollout service: {error:?}"))?;
    session
        .client
        .launch_app("bexos.test.wasm.client", "client", 0, 0)
        .map_err(|error| format!("launch provider-rollout client: {error:?}"))?;
    session.wait_for_serial_markers(&[b"wasm-client: counter=2"], Duration::from_secs(60))?;
    session.client.clear_received_trace();
    session
        .client
        .install_app_bundle(0x7246_43ff, &archives[3])
        .map_err(|error| format!("install runner provider replacement: {error:?}"))?;
    session.wait_for_serial_markers(
        &[
            b"wasm_runner: replacement runtime started",
            b"runner provider rollout completed provider=bexos.platform.wasm_runner",
        ],
        Duration::from_secs(180),
    )?;
    session.client.clear_received_trace();
    session.wait_for_serial_markers(&[b"wasm-client: counter="], Duration::from_secs(60))?;
    if String::from_utf8_lossy(session.client.received_trace()).contains("wasm-client: failed:") {
        return Err("live client failed after provider replacement".into());
    }
    session
        .client
        .list_apps()
        .map_err(|error| format!("appd unresponsive after provider replacement: {error:?}"))?;
    Ok(())
}
