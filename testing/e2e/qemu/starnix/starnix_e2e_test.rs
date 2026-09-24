use bexos_e2e::{DEFAULT_STALL_TIMEOUT, E2eContext, E2eDevice};
use bexos_qemu_test::{QemuArtifacts, QemuDevice};
use std::time::Duration;

fn loop_sequences(trace: &[u8]) -> Vec<u64> {
    String::from_utf8_lossy(trace)
        .lines()
        .filter_map(|line| {
            let value = line.split_once("starnix loop ")?.1.get(..16)?;
            u64::from_str_radix(value, 16).ok()
        })
        .collect()
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let context = E2eContext::from_env("starnix");
    let (artifacts, extra) =
        QemuArtifacts::from_env_or_args(&std::env::args().skip(1).collect::<Vec<_>>())?;
    let archive = std::fs::read(extra.first().ok_or("missing Starnix fixture archive")?)
        .map_err(|error| error.to_string())?;
    let mut device = QemuDevice::new(artifacts)?;
    let mut session = context.run_phase("boot", || {
        device.boot_with_debugd(&[b"appd: app lifecycle registry ready for debugd"])
    })?;
    context.run_phase("debugd-ready", || session.assert_debugd_ready())?;
    context.run_phase("install-fixture", || {
        session
            .client
            .install_app_bundle(0x53544152, &archive)
            .map_err(|error| format!("install Starnix fixture: {error:?}"))
    })?;
    context.run_phase("hello", || {
        session.client.clear_received_trace();
        if let Err(error) =
            session
                .client
                .launch_app("bexos.platform.starnix_fixture", "hello", 0, 0)
        {
            return Err(format!(
                "launch Starnix fixture: {error:?}\n{}",
                String::from_utf8_lossy(session.client.received_trace())
            ));
        }
        context.wait_until(
            "Starnix hello completion",
            Duration::from_secs(30),
            Duration::from_millis(50),
            Duration::from_secs(30),
            || {
                session
                    .client
                    .health_check()
                    .map_err(|error| format!("Starnix health: {error:?}"))?;
                let trace = String::from_utf8_lossy(session.client.received_trace());
                if trace.contains("guest panic") || trace.contains("kernel panic") {
                    return Err(format!("Starnix guest panic: {trace}"));
                }
                let hello = trace.contains("hello starnix\n");
                let exited = trace.contains("starnix_runner: guest exited 0");
                Ok((
                    (hello && exited).then_some(()),
                    hello as u64 + exited as u64,
                ))
            },
        )?;
        let health = session
            .client
            .health_check()
            .map_err(|error| format!("kernel health after Starnix: {error:?}"))?;
        if health.status != "SERVING" {
            return Err("kernel stopped serving after Starnix".into());
        }
        Ok(())
    })?;

    context.run_phase("looping-start", || {
        session.client.clear_received_trace();
        if let Err(error) =
            session
                .client
                .launch_app("bexos.platform.starnix_fixture", "looping", 0, 0)
        {
            return Err(format!(
                "launch looping Starnix fixture: {error:?}\n{}",
                String::from_utf8_lossy(session.client.received_trace())
            ));
        }
        session
            .wait_for_serial_markers(&[b"starnix loop 0000000000000000"], Duration::from_secs(60))
    })?;
    let before_sequence = loop_sequences(session.client.received_trace())
        .into_iter()
        .last()
        .ok_or("missing pre-transplant Starnix sequence")?;
    let before = session
        .client
        .list_processes()
        .map_err(|error| format!("process list before Starnix transplant: {error:?}"))?;
    let old_pid = before
        .iter()
        .find(|process| {
            process.package_id == "bexos.platform.starnix_fixture" && process.state == "Running"
        })
        .ok_or_else(|| format!("live looping Starnix process missing: {before:?}"))?
        .pid;
    const KEY_ID: [u8; 32] = *b"bexos-qemu-test-ed25519-key-v001";
    const SEED: [u8; 32] = [
        0x9d, 0x61, 0xb1, 0x9d, 0xef, 0xfd, 0x5a, 0x60, 0xba, 0x84, 0x4a, 0xf4, 0x92, 0xec, 0x2c,
        0xc4, 0x44, 0x49, 0xc5, 0x69, 0x7b, 0x32, 0x69, 0x19, 0x70, 0x3b, 0xac, 0x03, 0x1c, 0xae,
        0x7f, 0x60,
    ];
    let update = bexos_update::build_signed_manifest(
        1,
        "bexos.platform.starnix_fixture",
        bexos_update::ArtifactKind::AppPackage,
        &archive,
        KEY_ID,
        SEED,
    );
    context.run_phase("transplant-apply", || {
        session
            .client
            .upload_update(0x5354_4152, &update, &archive)
            .map_err(|error| format!("upload Starnix replacement: {error:?}"))?;
        let applied = session
            .client
            .exec_command("update.apply_service", &[])
            .map_err(|error| format!("apply Starnix replacement: {error:?}"))?;
        if applied.exit_code != 0 {
            return Err(format!("Starnix transplant rejected: {applied:?}"));
        }
        Ok(())
    })?;
    context.run_phase("transplant-commit", || {
        let mut last_status = String::new();
        context
            .wait_until(
                "Starnix transplant commit",
                Duration::from_secs(180),
                Duration::from_millis(100),
                DEFAULT_STALL_TIMEOUT,
                || {
                    let status = session
                        .client
                        .exec_command(
                            "update.service.status",
                            &["bexos.platform.starnix_fixture".into()],
                        )
                        .map_err(|error| format!("Starnix transplant status: {error:?}"))?;
                    last_status = status.stdout.clone();
                    if status.stdout.contains("pending=false") {
                        if !status.stdout.contains("generation=1 ") {
                            return Err(format!("Starnix transplant did not commit: {status:?}"));
                        }
                        return Ok((Some(()), 2));
                    }
                    Ok((None, u64::from(status.stdout.contains("pending=true"))))
                },
            )
            .map_err(|error| format!("{error}; last status: {last_status}"))
    })?;
    context.run_phase("retained-client", || {
        session.client.clear_received_trace();
        context.wait_until(
            "Starnix retained client",
            Duration::from_secs(60),
            Duration::from_millis(50),
            Duration::from_secs(60),
            || {
                session
                    .client
                    .health_check()
                    .map_err(|error| format!("health after Starnix transplant: {error:?}"))?;
                let trace = session.client.received_trace();
                let sequences = loop_sequences(trace);
                if !sequences.is_empty() {
                    if sequences[0] <= before_sequence
                        || sequences.windows(2).any(|pair| pair[1] <= pair[0])
                    {
                        return Err(format!(
                            "Starnix state/output sequence was not preserved: before={before_sequence:x} after={sequences:x?}"
                        ));
                    }
                    return Ok((Some(()), *sequences.last().unwrap()));
                }
                if String::from_utf8_lossy(trace).contains("panic") {
                    return Err(format!(
                        "panic after Starnix transplant: {}",
                        String::from_utf8_lossy(trace)
                    ));
                }
                Ok((None, 0))
            },
        )
    })?;
    context.run_phase("final-health", || {
        let after = session
            .client
            .list_processes()
            .map_err(|error| format!("process list after Starnix transplant: {error:?}"))?;
        if !after.iter().any(|process| {
            process.package_id == "bexos.platform.starnix_fixture"
                && process.state == "Running"
                && process.pid != old_pid
        }) {
            return Err(format!("Starnix replacement identity missing: {after:?}"));
        }
        Ok(())
    })
}
