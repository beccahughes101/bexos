use bexos_e2e::{BEXFS_MARKERS, BOOT_MARKERS, E2eDevice, NVME_MARKERS};
use bexos_qemu_test::{QemuAarch64Device, QemuArtifacts};
fn main() {
    if let Err(e) = run() {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
fn run() -> Result<(), String> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let (artifacts, _extra) = QemuArtifacts::from_env_or_args(&args)?;
    let mut markers = BOOT_MARKERS.to_vec();
    markers.extend_from_slice(NVME_MARKERS);
    markers.extend_from_slice(BEXFS_MARKERS);
    markers.push(b"appd: app lifecycle registry ready for debugd");
    markers.push(b"wasi-fixture: streams clocks random environment ok");
    let mut device = QemuAarch64Device::new(artifacts)?;
    let mut session = device.boot_with_debugd(&markers)?;
    session.assert_debugd_ready()?;
    let outcome = (|| {
        let launch_fixtures: &[&str] = if std::env::var_os("BEXOS_WASM_MIGRATION_ONLY").is_some() {
            &[]
        } else {
            &[
                "command",
                "children",
                "trap",
                "memory",
                "network_denied",
                "network_granted",
            ]
        };
        for name in launch_fixtures.iter() {
            session.client.clear_received_trace();
            session
                .client
                .launch_app(&format!("bexos.test.wasm.{name}"), name, 0, 0)
                .map_err(|e| format!("launch {name}: {e:?}"))?;
            session.wait_for_serial_markers(
                &[if *name != "trap" {
                    b"wasm_runner: exit 0"
                } else {
                    b"wasm_runner: failed:"
                }],
                std::time::Duration::from_secs(60),
            )?;
            eprintln!("e2e: {name} completed with expected exit/trap reporting");
            session
                .client
                .list_apps()
                .map_err(|e| format!("host unusable after {name}: {e:?}"))?;
        }
        eprintln!("e2e: launching WASM service and persistent client");
        session
            .client
            .launch_app("bexos.test.wasm.service", "service", 0, 0)
            .map_err(|e| format!("launch service: {e:?}"))?;
        session
            .client
            .launch_app("bexos.test.wasm.client", "client", 0, 0)
            .map_err(|e| format!("launch client: {e:?}"))?;
        session.wait_for_serial_markers(
            &[b"wasm-client: counter=2"],
            std::time::Duration::from_secs(60),
        )?;
        for (generation, archive, commit) in [
            (1, "bexos.test.wasm.service.rejected", false),
            (2, "bexos.test.wasm.service.replacement", true),
            (3, "bexos.test.wasm.service.runner_replacement", true),
        ] {
            eprintln!(
                "e2e: staging WASM migration generation={generation} expected_commit={commit}"
            );
            let before = session
                .client
                .list_processes()
                .map_err(|e| format!("process list: {e:?}"))?;
            let old = before
                .iter()
                .find(|p| p.package_id == "bexos.test.wasm.service" && p.state == "Running")
                .ok_or("live WASM service missing")?
                .pid;
            session.client.clear_received_trace();
            let response = session
                .client
                .exec_command(
                    "update.apply_stored_service",
                    &[
                        "bexos.test.wasm.service".into(),
                        generation.to_string(),
                        archive.into(),
                    ],
                )
                .map_err(|e| format!("stage migration: {e:?}"))?;
            if response.exit_code != 0 {
                return Err(format!("migration staging failed: {response:?}"));
            }
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(180);
            loop {
                let status = session
                    .client
                    .exec_command("update.service.status", &["bexos.test.wasm.service".into()])
                    .map_err(|e| format!("migration status: {e:?}"))?;
                if status.stdout.contains("pending=false") {
                    let expected = if commit { generation } else { 0 };
                    if !status.stdout.contains(&format!("generation={expected} ")) {
                        return Err(format!("unexpected migration outcome: {status:?}"));
                    }
                    break;
                }
                if std::time::Instant::now() > deadline {
                    return Err(format!("migration timed out: {status:?}"));
                }
                std::thread::sleep(std::time::Duration::from_millis(1000));
            }
            if generation == 3
                && !String::from_utf8_lossy(session.client.received_trace())
                    .contains("wasm_runner: replacement runtime started")
            {
                return Err("replacement did not execute the new trusted runner binary".into());
            }
            eprintln!(
                "{}",
                String::from_utf8_lossy(session.client.received_trace())
            );
            let after = session
                .client
                .list_processes()
                .map_err(|e| format!("process list: {e:?}"))?;
            let active = after
                .iter()
                .find(|p| p.package_id == "bexos.test.wasm.service" && p.state == "Running")
                .ok_or("WASM service lost during migration")?
                .pid;
            if (active != old) != commit {
                return Err("migration changed the wrong process".into());
            }
            session.client.clear_received_trace();
            session.wait_for_serial_markers(
                &[b"wasm-client: counter="],
                std::time::Duration::from_secs(60),
            )?;
            if String::from_utf8_lossy(session.client.received_trace())
                .contains("wasm-client: failed:")
            {
                return Err("client state/connectivity did not survive migration".into());
            }
            eprintln!(
                "e2e: WASM migration generation={generation} commit={commit} preserved the live client"
            );
        }
        eprintln!("e2e: launching WASI file-stream service and client");
        session
            .client
            .launch_app("bexos.test.wasm.file_service", "file_service", 0, 0)
            .map_err(|e| format!("launch file service: {e:?}"))?;
        session
            .client
            .launch_app("bexos.test.wasm.file_client", "file_client", 0, 0)
            .map_err(|e| format!("launch file client: {e:?}"))?;
        session.wait_for_serial_markers(
            &[b"wasm-file-client: counter=2"],
            std::time::Duration::from_secs(60),
        )?;
        session.client.clear_received_trace();
        let response = session
            .client
            .exec_command(
                "update.apply_stored_service",
                &[
                    "bexos.test.wasm.file_service".into(),
                    "1".into(),
                    "bexos.test.wasm.file_service.replacement".into(),
                ],
            )
            .map_err(|e| format!("stage file service: {e:?}"))?;
        if response.exit_code != 0 {
            return Err(format!("file migration staging failed: {response:?}"));
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(180);
        loop {
            let status = session
                .client
                .exec_command(
                    "update.service.status",
                    &["bexos.test.wasm.file_service".into()],
                )
                .map_err(|e| format!("file migration status: {e:?}"))?;
            if status.stdout.contains("pending=false") {
                if !status.stdout.contains("generation=1 ") {
                    return Err(format!("file migration failed: {status:?}"));
                }
                break;
            }
            if std::time::Instant::now() > deadline {
                return Err("file migration timeout".into());
            }
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
        session.client.clear_received_trace();
        session.wait_for_serial_markers(
            &[b"wasm-file-client: counter="],
            std::time::Duration::from_secs(60),
        )?;
        if String::from_utf8_lossy(session.client.received_trace())
            .contains("wasm-file-client: failed:")
        {
            return Err("file stream or client state lost after migration".into());
        }
        eprintln!("e2e: WASI descriptor, input offset, pollable, and client survived migration");
        Ok(())
    })();
    outcome.map_err(|error: String| {
        format!(
            "{error}\n{}",
            String::from_utf8_lossy(session.client.received_trace())
        )
    })
}
