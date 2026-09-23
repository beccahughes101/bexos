use bexos_e2e::E2eDevice;
use bexos_qemu_test::{QemuArtifacts, QemuDevice};
use pkg_https_fixture::{Reply, Server};
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
fn wait_for_registry(reached: &AtomicBool) -> Result<(), String> {
    let until = std::time::Instant::now() + Duration::from_secs(30);
    while !reached.load(Ordering::SeqCst) {
        if std::time::Instant::now() >= until {
            return Err("pending read never reached registry".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}
fn run() -> Result<(), String> {
    let (artifacts, extra) =
        QemuArtifacts::from_env_or_args(&std::env::args().skip(1).collect::<Vec<_>>())?;
    if extra.len() != 1 {
        return Err("expected signed OCI fixture responses".into());
    }
    let responses: BTreeMap<String, Vec<u8>> =
        serde_json::from_slice(&std::fs::read(&extra[0]).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    let hold = Arc::new(AtomicBool::new(false));
    let reached = Arc::new(AtomicBool::new(false));
    let rotated = Arc::new(AtomicBool::new(false));
    let server_hold = hold.clone();
    let server_reached = reached.clone();
    let server_rotated = rotated.clone();
    let registry = Server::start_port_with_timeout(
        18464,
        true,
        true,
        Duration::from_secs(240),
        move |path, headers| {
            eprintln!(
                "pkg-registry: request {}",
                path.split('?').next().unwrap_or(path)
            );
            if path.starts_with("/token?") {
                assert!(path.contains("scope=repository%3Aapps%2Fdemo%3Apull"));
                assert!(headers.iter().all(|(k, _)| k != "authorization"));
                return Reply {
                    status: 200,
                    headers: vec![],
                    body: if server_rotated.load(Ordering::SeqCst) {
                        br#"{"token":"fixture-rotated"}"#.to_vec()
                    } else {
                        br#"{"token":"fixture-pull"}"#.to_vec()
                    },
                };
            }
            if !headers.iter().any(|(k, v)| {
                k == "authorization"
                    && v == if server_rotated.load(Ordering::SeqCst) {
                        "Bearer fixture-rotated"
                    } else {
                        "Bearer fixture-pull"
                    }
            }) {
                return Reply {status:401,headers:vec![("www-authenticate".into(),"Bearer realm=\"https://10.0.2.2:18464/token\",scope=\"repository:apps/demo:pull\"".into())],body:vec![]};
            }
            if path.ends_with("/manifests/tuf-timestamp") && server_hold.load(Ordering::SeqCst) {
                server_reached.store(true, Ordering::SeqCst);
                let until = std::time::Instant::now() + Duration::from_secs(30);
                while server_hold.load(Ordering::SeqCst) && std::time::Instant::now() < until {
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
            match responses.get(path) {
                Some(body) => Reply {
                    status: 200,
                    headers: vec![],
                    body: body.clone(),
                },
                None => Reply {
                    status: 404,
                    headers: vec![],
                    body: vec![],
                },
            }
        },
    );
    let mut device = QemuDevice::new(artifacts)?;
    let mut session =
        device.boot_with_debugd(&[b"appd: app lifecycle registry ready for debugd"])?;
    eprintln!("pkg-acceptance: launching preinstalled protocol probe");
    session
        .client
        .launch_app("bexos.test.pkg_protocol", "probe", 0, 0)
        .map_err(|e| {
            format!(
                "launch protocol probe: {e:?}\n{}",
                String::from_utf8_lossy(session.client.received_trace())
            )
        })?;
    session.wait_for_serial_markers(
        &[b"pkg-protocol-probe: malformed resources cancellation and drop verified"],
        Duration::from_secs(30),
    )?;
    eprintln!("pkg-acceptance: protocol resource probes passed; launching consumer");
    session
        .client
        .launch_app("bexos.platform.pkg_probe", "probe", 0, 0)
        .map_err(|e| {
            format!(
                "launch probe: {e:?}\n{}",
                String::from_utf8_lossy(session.client.received_trace())
            )
        })?;
    let consumers_deadline = std::time::Instant::now() + Duration::from_secs(900);
    let mut observed_bytes = 0;
    (|| {
        for marker in [
            b"pkg-probe: sealed credentials provisioned".as_slice(),
            b"pkg-probe: verified immutable artifact".as_slice(),
            b"pkg-probe: appd installed signed OCI application".as_slice(),
            b"pkg-probe: local and remote immutable fonts verified".as_slice(),
            b"pkg-probe: VMOs retained for replacement".as_slice(),
        ] {
            session.wait_for_serial_markers_observed(
                &[marker],
                consumers_deadline.saturating_duration_since(std::time::Instant::now()),
                |output| {
                    let Some(end) = output.iter().rposition(|byte| *byte == b'\n') else {
                        return;
                    };
                    if end + 1 <= observed_bytes {
                        return;
                    }
                    for line in String::from_utf8_lossy(&output[observed_bytes..=end]).lines() {
                        if line.contains("pkgd:") || line.contains("pkg-probe:") {
                            eprintln!("pkg-acceptance: guest {line}");
                        }
                    }
                    observed_bytes = end + 1;
                },
            )?;
            eprintln!(
                "pkg-acceptance: observed {}",
                String::from_utf8_lossy(marker)
            );
        }
        Ok::<(), String>(())
    })()
    .map_err(|error| {
        let trace = String::from_utf8_lossy(session.client.received_trace());
        let diagnostics = trace
            .lines()
            .filter(|line| {
                line.contains("pkgd:") || line.contains("pkg-probe:") || line.contains("netstackd:")
            })
            .collect::<Vec<_>>()
            .join("\n");
        format!("{error}\nPackage diagnostics:\n{diagnostics}")
    })?;
    eprintln!("pkg-acceptance: application and font resolution passed");
    let deadline = std::time::Instant::now() + Duration::from_secs(900);
    loop {
        let apps = session
            .client
            .list_apps()
            .map_err(|e| format!("list acquired packages: {e:?}"))?;
        if ["bexos.test.pkg_driver", "bexos.test.pkg_firmware"]
            .iter()
            .all(|package| {
                apps.iter()
                    .any(|app| app.package_id == *package && app.source.eq_ignore_ascii_case("oci"))
            })
        {
            break;
        }
        if std::time::Instant::now() >= deadline {
            return Err(format!(
                "configured driver/firmware acquisition did not finish: {apps:?}"
            ));
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    eprintln!("pkg-acceptance: configured driver and firmware acquisition passed");
    hold.store(true, Ordering::SeqCst);
    session
        .client
        .launch_app("bexos.platform.pkg_probe", "probe", 6, 0)
        .map_err(|e| format!("launch read for credential replacement: {e:?}"))?;
    wait_for_registry(&reached)?;
    rotated.store(true, Ordering::SeqCst);
    session
        .client
        .launch_app("bexos.platform.pkg_probe", "probe", 5, 0)
        .map_err(|e| format!("replace credentials during read: {e:?}"))?;
    session.wait_for_serial_markers(
        &[b"pkg-probe: credentials replaced during pending resolution"],
        Duration::from_secs(30),
    )?;
    hold.store(false, Ordering::SeqCst);
    session.wait_for_serial_markers(
        &[b"pkg-probe: pending resolution survived credential replacement"],
        Duration::from_secs(120),
    )?;
    eprintln!("pkg-acceptance: live credential replacement passed");
    let rejected = session
        .client
        .exec_command(
            "update.apply_stored_service",
            &[
                "bexos.service.pkgd".into(),
                "63".into(),
                "bexos.service.pkgd.rejected".into(),
            ],
        )
        .map_err(|e| format!("stage rejected pkgd: {e:?}"))?;
    if rejected.exit_code != 0 {
        return Err(format!("rejection fixture did not stage: {rejected:?}"));
    }
    session.wait_for_serial_markers(
        &[b"appd: migration task failed: migration rejected status=-8"],
        Duration::from_secs(30),
    )?;
    let status = session
        .client
        .exec_command("update.service.status", &["bexos.service.pkgd".into()])
        .map_err(|e| format!("rejected pkgd status: {e:?}"))?;
    if !status.stdout.contains("pending=false") || !status.stdout.contains("generation=0 ") {
        return Err(format!("rejected pkgd activated: {status:?}"));
    }
    reached.store(false, Ordering::SeqCst);
    hold.store(true, Ordering::SeqCst);
    session
        .client
        .launch_app("bexos.platform.pkg_probe", "probe", 3, 0)
        .map_err(|e| format!("launch pending read: {e:?}"))?;
    wait_for_registry(&reached)?;
    let result = session
        .client
        .exec_command(
            "update.apply_stored_service",
            &[
                "bexos.service.pkgd".into(),
                "64".into(),
                "bexos.service.pkgd.replacement".into(),
            ],
        )
        .map_err(|e| format!("replace pkgd: {e:?}"))?;
    if result.exit_code != 0 {
        return Err(format!("replace pkgd: {result:?}"));
    }
    session.wait_for_serial_markers(
        &[b"service-transplant: committed generation=64"],
        Duration::from_secs(25),
    )?;
    hold.store(false, Ordering::SeqCst);
    session.wait_for_serial_markers(
        &[
            b"pkg-probe: pending resolution resumed after replacement",
            b"service-transplant: committed generation=64",
        ],
        Duration::from_secs(240),
    )?;
    // Require a fresh read from the original consumer after commit, rather than
    // relying on its lifetime overlapping a fixed host-side delay.
    let retained_deadline = std::time::Instant::now() + Duration::from_secs(60);
    loop {
        session
            .client
            .health_check()
            .map_err(|e| format!("VMO survival health check: {e:?}"))?;
        let trace = String::from_utf8_lossy(session.client.received_trace());
        let committed = trace
            .find("service-transplant: committed generation=64")
            .ok_or("replacement commit marker missing")?;
        if trace
            .rfind("pkg-probe: original VMOs remain immutable")
            .is_some_and(|read| read > committed)
        {
            break;
        }
        if std::time::Instant::now() >= retained_deadline {
            return Err("original consumer did not verify retained VMOs after replacement".into());
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    session
        .client
        .launch_app("bexos.test.pkg_remote", "remote", 0, 0)
        .map_err(|e| format!("launch OCI app: {e:?}"))?;
    session.wait_for_serial_markers(
        &[b"pkg-remote-app: launched verified OCI installation"],
        Duration::from_secs(30),
    )?;
    let trace = String::from_utf8_lossy(session.client.received_trace());
    let committed = trace
        .find("service-transplant: committed generation=64")
        .ok_or("replacement commit marker missing")?;
    let survived = trace
        .rfind("pkg-probe: original VMOs remain immutable")
        .ok_or("consumer VMO survival marker missing")?;
    if committed > survived {
        return Err("consumer released VMOs before replacement committed".into());
    }
    drop(session);
    let mut session =
        device.boot_with_debugd(&[b"appd: app lifecycle registry ready for debugd"])?;
    session
        .client
        .launch_app("bexos.platform.pkg_probe", "probe", 1, 0)
        .map_err(|e| format!("post-reboot probe: {e:?}"))?;
    session.wait_for_serial_markers(
        &[
            b"pkg-probe: appd installed signed OCI application",
            b"pkg-probe: retained VMOs survived",
        ],
        Duration::from_secs(150),
    )?;
    session
        .client
        .launch_app("bexos.test.pkg_denied", "probe", 4, 0)
        .map_err(|e| format!("launch denied consumer: {e:?}"))?;
    session.wait_for_serial_markers(
        &[b"pkg-probe: unprivileged credentials and cache isolation verified"],
        Duration::from_secs(30),
    )?;
    drop(registry);
    session
        .client
        .launch_app("bexos.platform.pkg_probe", "probe", 2, 0)
        .map_err(|e| format!("offline probe: {e:?}"))?;
    session.wait_for_serial_markers(
        &[b"pkg-probe: offline cache acceptance complete"],
        Duration::from_secs(90),
    )?;
    Ok(())
}
