use bexos_debug_client::{DebugClient, DebugTransport};
use bexos_debug_wire::ShellRequest;
use bexos_e2e::{BEXFS_MARKERS, E2eDevice, NVME_MARKERS, boot_markers};
use bexos_qemu_test::{QemuArtifacts, QemuDevice};
use std::time::{Duration, Instant};
type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;
fn main() {
    if let Err(e) = run() {
        eprintln!("shell e2e: {e}");
        std::process::exit(1);
    }
}
fn echo<T: DebugTransport>(c: &mut DebugClient<T>, id: u64, bytes: &[u8]) -> Result {
    let mut remaining = bytes;
    let mut output = Vec::new();
    let mut deadline = Instant::now() + Duration::from_secs(30);
    while !remaining.is_empty() || output.len() < bytes.len() {
        if Instant::now() > deadline {
            return Err(format!(
                "terminal echo timed out: sent={}, received={}",
                bytes.len() - remaining.len(),
                output.len()
            )
            .into());
        }
        let n = remaining.len().min(bexos_debug_wire::SHELL_CHUNK);
        let r = c.exchange_shell(id, &remaining[..n], false)?;
        remaining = &remaining[r.consumed as usize..];
        if r.consumed != 0 || !r.stdout.is_empty() || !r.stderr.is_empty() {
            deadline = Instant::now() + Duration::from_secs(30);
        }
        output.extend(r.stdout);
        assert!(!r.exited);
    }
    assert_eq!(output, bytes);
    Ok(())
}
fn run() -> Result {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let auth_only = args.iter().any(|arg| arg == "--auth-only");
    let artifact_args = args
        .iter()
        .filter(|arg| arg.as_str() != "--auth-only")
        .cloned()
        .collect::<Vec<_>>();
    let (artifacts, extra) = QemuArtifacts::from_env_or_args(&artifact_args)?;
    let archive = std::fs::read(extra.first().ok_or("missing shell fixture")?)?;
    let replacement = std::fs::read(extra.get(1).ok_or("missing debugd replacement")?)?;
    let rejected = std::fs::read(extra.get(2).ok_or("missing rollback candidate")?)?;
    let mut markers = boot_markers();
    markers.extend_from_slice(NVME_MARKERS);
    markers.extend_from_slice(BEXFS_MARKERS);
    markers.push(b"appd: app lifecycle registry ready for debugd");
    let mut device = QemuDevice::new(artifacts)?;
    let mut session = device.boot_with_debugd(&markers)?;
    session.assert_debugd_ready()?;
    let result = (|| -> Result {
        let c = &mut session.client;
        let system = || ShellRequest {
            system: true,
            rows: 24,
            cols: 80,
            ..Default::default()
        };
        let fixture_preinstalled =
            std::env::var_os("BEXOS_DEBUGD_SHELL_FIXTURE_PREINSTALLED").is_some();
        if fixture_preinstalled {
            eprintln!("e2e: terminal fixture preinstalled");
        } else {
            let e = c
                .open_shell(&system())
                .err()
                .ok_or("missing provider unexpectedly succeeded")?;
            assert!(e.to_string().contains("no shell provider installed"), "{e}");
            eprintln!("e2e: installing terminal fixture");
            c.install_app_bundle(0x5348, &archive)?;
            eprintln!("e2e: terminal fixture installed");
        }
        if auth_only {
            return authenticated_users(c, 0);
        }
        eprintln!("e2e: opening system terminal");
        let opened = c.open_shell(&system())?;
        assert_eq!(opened.uid, 0);
        let id = opened.session_id;
        assert!(c.open_shell(&system()).is_err());
        let mut cooked = c.exchange_shell(id, b"abc\x7fd\r", false)?;
        assert_eq!(cooked.consumed, 6);
        let mut cooked_bytes = cooked.stdout;
        let deadline = Instant::now() + Duration::from_secs(20);
        while cooked_bytes.len() < 13 {
            if Instant::now() > deadline {
                return Err("canonical input timed out".into());
            }
            cooked = c.exchange_shell(id, &[], false)?;
            cooked_bytes.extend(cooked.stdout);
        }
        assert_eq!(cooked_bytes, b"abc\x08 \x08d\r\nabd\n");
        c.set_shell_mode(id, 0)?;
        let data = (0..40000).map(|i| (i % 251) as u8).collect::<Vec<_>>();
        echo(c, id, &data)?;
        c.resize_shell(id, 42, 123)?;
        c.signal_shell(id, 1)?;
        let mut stderr = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(20);
        while !String::from_utf8_lossy(&stderr).contains("signal=1 rows=42 cols=123") {
            if Instant::now() > deadline {
                return Err("resize/signal not forwarded".into());
            }
            stderr.extend(c.exchange_shell(id, &[], false)?.stderr);
        }
        c.close_shell(id)?;
        authenticated_users(c, 1)?;
        // Retain a nonzero authenticated identity through migration when supported.
        let migrating = if c.tee_info()?.present {
            c.open_shell(&ShellRequest {
                uid: 1000,
                password: "correct horse".into(),
                ..Default::default()
            })?
        } else {
            c.open_shell(&system())?
        };
        let id = migrating.session_id;
        c.set_shell_mode(id, 0)?;
        eprintln!(
            "e2e: transplanting debugd with active terminal uid={}",
            migrating.uid
        );
        let key_id = *b"bexos-qemu-test-ed25519-key-v001";
        let seed = [
            0x9d, 0x61, 0xb1, 0x9d, 0xef, 0xfd, 0x5a, 0x60, 0xba, 0x84, 0x4a, 0xf4, 0x92, 0xec,
            0x2c, 0xc4, 0x44, 0x49, 0xc5, 0x69, 0x7b, 0x32, 0x69, 0x19, 0x70, 0x3b, 0xac, 0x03,
            0x1c, 0xae, 0x7f, 0x60,
        ];
        let manifest = bexos_update::build_signed_manifest(
            41,
            "bexos.driver.debugd",
            bexos_update::ArtifactKind::AppPackage,
            &replacement,
            key_id,
            seed,
        );
        eprintln!("e2e: uploading debugd replacement");
        c.upload_update(0x5349, &manifest, &replacement)?;
        eprintln!("e2e: applying debugd replacement");
        let r = c.exec_command("update.apply_service", &[])?;
        assert_eq!(r.exit_code, 0, "{}", r.stderr);
        eprintln!("e2e: debugd replacement apply accepted");
        let deadline = Instant::now() + Duration::from_secs(180);
        loop {
            let r = c.exec_command("update.service.status", &["bexos.driver.debugd".into()])?;
            if r.stdout.contains("completed") {
                break;
            }
            if Instant::now() > deadline {
                return Err(format!("debugd transplant: {} {}", r.stdout, r.stderr).into());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        echo(c, id, b"terminal survives heart transplant\0\xff")?;
        assert_eq!(c.exchange_shell(id, &[], false)?.uid, migrating.uid);
        let status = c.exec_command("update.status", &[])?;
        assert_eq!(
            status.exit_code, 0,
            "updated proxy lost during transplant: {}",
            status.stderr
        );
        eprintln!("e2e: rejecting a transplant with active terminal");
        let manifest = bexos_update::build_signed_manifest(
            42,
            "bexos.driver.debugd",
            bexos_update::ArtifactKind::AppPackage,
            &rejected,
            key_id,
            seed,
        );
        eprintln!("e2e: uploading rejecting debugd replacement");
        c.upload_update(0x5350, &manifest, &rejected)?;
        eprintln!("e2e: applying rejecting debugd replacement");
        let applied = c.exec_command("update.apply_service", &[])?;
        assert_eq!(applied.exit_code, 0, "{}", applied.stderr);
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let status =
                c.exec_command("update.service.status", &["bexos.driver.debugd".into()])?;
            if status.stdout.contains("pending=false") {
                assert!(
                    status.stdout.contains("generation=41 "),
                    "{}",
                    status.stdout
                );
                assert!(status.stdout.contains("rejected"), "{}", status.stdout);
                break;
            }
            if Instant::now() > deadline {
                return Err("rollback timed out".into());
            }
        }
        echo(c, id, b"terminal survives rollback")?;
        assert_eq!(c.exchange_shell(id, &[], false)?.uid, migrating.uid);
        let mut r = c.exchange_shell(id, b"final output", true)?;
        assert_eq!(r.consumed, 12);
        let mut final_output = r.stdout.clone();
        let deadline = Instant::now() + Duration::from_secs(20);
        while !r.exited {
            if Instant::now() > deadline {
                return Err("terminal did not exit after EOF".into());
            }
            r = c.exchange_shell(id, &[], true)?;
            final_output.extend_from_slice(&r.stdout);
        }
        assert_eq!(r.exit_code, 0);
        assert_eq!(final_output, b"final output");
        eprintln!("e2e: checking disconnect lease cleanup");
        let leased = c.open_shell(&system())?;
        std::thread::sleep(Duration::from_secs(32));
        assert!(c.exchange_shell(leased.session_id, &[], false).is_err());
        let reopened = c.open_shell(&system())?;
        c.close_shell(reopened.session_id)?;
        Ok(())
    })();
    result.map_err(|e| {
        format!(
            "{e}\n{}",
            String::from_utf8_lossy(session.client.received_trace())
        )
        .into()
    })
}

fn authenticated_users<T: DebugTransport>(
    c: &mut DebugClient<T>,
    system_providers: usize,
) -> Result {
    if !c.tee_info()?.present {
        // A guest without a TEE-backed user vault must fail closed.
        assert!(
            c.open_shell(&ShellRequest {
                uid: 1000,
                password: "unavailable".into(),
                ..Default::default()
            })
            .is_err()
        );
        return Ok(());
    }
    eprintln!("e2e: checking authenticated user isolation");
    c.create_user(1000, "shell_alice", "Alice", "correct horse")?;
    c.create_user(1001, "shell_bob", "Bob", "correct battery")?;
    for (uid, name, password) in [
        (1000, "shell_alice", "correct horse"),
        (1001, "shell_bob", "correct battery"),
    ] {
        assert!(
            c.open_shell(&ShellRequest {
                uid,
                password: "wrong".into(),
                ..Default::default()
            })
            .is_err()
        );
        eprintln!("e2e: rejected wrong shell password for user {uid}");
        eprintln!("e2e: authenticating shell user {uid}");
        let s = c.open_shell(&ShellRequest {
            user: if uid == 1000 {
                name.into()
            } else {
                String::new()
            },
            uid: if uid == 1001 { uid } else { 0 },
            password: password.into(),
            ..Default::default()
        })?;
        eprintln!("e2e: authenticated shell user {uid}");
        assert_eq!(s.uid, uid);
        c.set_shell_mode(s.session_id, 0)?;
        eprintln!("e2e: raw shell mode set for user {uid}");
        echo(c, s.session_id, name.as_bytes())?;
        eprintln!("e2e: shell echo verified for user {uid}");
        c.close_shell(s.session_id)?;
        eprintln!("e2e: shell closed for user {uid}");
        assert!(
            c.open_shell(&ShellRequest {
                uid,
                password: "wrong".into(),
                ..Default::default()
            })
            .is_err()
        );
        eprintln!("e2e: post-close wrong password rejected for user {uid}");
    }
    let processes = c.list_processes()?;
    assert_eq!(
        processes
            .iter()
            .filter(|p| p.package_id == "com.example.shell_fixture")
            .count(),
        system_providers + 2,
        "system and users must have distinct provider processes"
    );
    c.update_user(1001, "shell_bob", "Bob", true)?;
    assert!(
        c.open_shell(&ShellRequest {
            uid: 1001,
            password: "correct battery".into(),
            ..Default::default()
        })
        .is_err()
    );
    assert!(
        processes.iter().any(|p| !p.resource_group_name.is_empty()),
        "live kernel group names missing"
    );
    Ok(())
}
