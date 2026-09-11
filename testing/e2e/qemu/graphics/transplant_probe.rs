use bexos_debug_client::{DebugClient, UnixSocketTransport};
use std::time::{Duration, Instant};
#[cfg(all(fixture_linux, not(target_env = "musl")))]
compile_error!("The Linux fixture probe must use the isolated static musl toolchain");

#[derive(Clone, Copy)]
struct Replacement {
    package: &'static str,
    generation: u64,
    archive_id: &'static str,
    reject_archive_id: Option<&'static str>,
}

fn main() {
    let socket = std::env::args().nth(1).expect("debug socket");
    let mut client = DebugClient::new(UnixSocketTransport::connect(socket).unwrap());
    println!("graphics probe: connected to debug relay");
    client
        .health_check()
        .expect("debug health before transplant");
    println!("graphics probe: health ok");
    let mut replacements = vec![Replacement {
        package: "bexos.driver.display.virtio_gpu",
        generation: 91,
        archive_id: "bexos.driver.display.virtio_gpu.replacement",
        reject_archive_id: Some("bexos.driver.display.virtio_gpu.rejected"),
    }];
    if std::env::var_os("BOOT_UI_VALIDATE_SPLASH").is_some() {
        replacements.push(Replacement {
            package: "bexos.service.splashd",
            generation: 93,
            archive_id: "bexos.service.splashd.replacement",
            reject_archive_id: None,
        });
    }
    if std::env::var_os("BOOT_UI_INPUT").is_some() {
        replacements.push(Replacement {
            package: "bexos.driver.pci_root",
            generation: 96,
            archive_id: "bexos.driver.pci_root.replacement",
            reject_archive_id: None,
        });
        replacements.push(Replacement {
            package: "bexos.driver.input.virtio",
            generation: 94,
            archive_id: "bexos.driver.input.virtio.replacement",
            reject_archive_id: None,
        });
    }
    replacements.push(Replacement {
        package: "bexos.service.scened",
        generation: 92,
        archive_id: "bexos.service.scened.replacement",
        reject_archive_id: Some("bexos.service.scened.rejected"),
    });
    if std::env::var_os("BOOT_UI_INPUT").is_some() {
        replacements.push(Replacement {
            package: "bexos.testing.input_fixture",
            generation: 95,
            archive_id: "bexos.testing.input_fixture.replacement",
            reject_archive_id: None,
        });
    }
    for replacement in replacements {
        let package = replacement.package;
        let generation = replacement.generation;
        println!("graphics probe: applying stored replacement package={package}");
        let response = client
            .exec_command(
                "update.apply_stored_service",
                &[
                    package.into(),
                    generation.to_string(),
                    replacement.archive_id.into(),
                ],
            )
            .unwrap();
        assert_eq!(response.exit_code, 0, "{response:?}");
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            let status = client
                .exec_command("update.service.status", &[package.into()])
                .unwrap();
            assert_eq!(status.exit_code, 0, "{status:?}");
            if status.stdout.contains("migration completed") {
                println!("graphics probe: replacement running package={package}");
                break;
            }
            assert!(
                !status.stdout.contains("pending=false"),
                "transplant failed {status:?}"
            );
            assert!(Instant::now() < deadline, "transplant timeout {status:?}");
            // Status is sufficient during cutover. Repeated process inventories
            // also serialize the entire list to the emulated debug console.
            client.drain_for(Duration::from_millis(500)).unwrap();
        }
        // The completion branch already verified both the replacement PID and
        // source retirement. Avoid a redundant full process inventory over the
        // emulated UART before moving to the next replacement.
        client.health_check().unwrap();
        if std::env::var_os("BOOT_UI_INPUT").is_some()
            && matches!(
                package,
                "bexos.service.scened" | "bexos.driver.display.virtio_gpu"
            )
        {
            reject_candidate(
                &mut client,
                package,
                generation,
                replacement.reject_archive_id,
            );
        }
    }
    println!("graphics probe: transplants verified");
}

fn reject_candidate(
    client: &mut DebugClient<UnixSocketTransport>,
    package: &str,
    generation: u64,
    reject_archive_id: Option<&str>,
) {
    let before = client.list_processes().unwrap();
    let source = before
        .iter()
        .find(|p| p.package_id == package && p.state == "Running")
        .unwrap()
        .pid;
    let reject_archive_id = reject_archive_id.expect("missing reject archive id");
    let response = client
        .exec_command(
            "update.apply_stored_service",
            &[
                package.into(),
                (generation + 100).to_string(),
                reject_archive_id.into(),
            ],
        )
        .unwrap();
    assert_eq!(
        response.exit_code, 0,
        "candidate was not admitted: {response:?}"
    );
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let status = client
            .exec_command("update.service.status", &[package.into()])
            .unwrap();
        assert_eq!(status.exit_code, 0, "{status:?}");
        if status.stdout.contains("pending=false") {
            // The incompatible receiver exits when logical state adoption
            // rejects its version. The initialization-reject fixture instead
            // sends an explicit negative response. Both must preserve source
            // ownership; the guest log separately checks UnsupportedVersion.
            let failure = if package == "bexos.service.scened" {
                "migration peer closed"
            } else {
                "migration rejected status=-8"
            };
            assert!(
                status.stdout.contains(&format!("generation={generation} "))
                    && status.stdout.contains(failure),
                "{status:?}"
            );
            let live = client.list_processes().unwrap();
            assert!(
                live.iter()
                    .any(|p| p.package_id == package && p.pid == source && p.state == "Running"),
                "source was lost on rejected candidate"
            );
            assert_eq!(
                live.iter()
                    .filter(|p| p.package_id == package && p.state == "Running")
                    .count(),
                1,
                "candidate leaked"
            );
            client.health_check().unwrap();
            println!("graphics probe: rejected candidate preserved source package={package}");
            break;
        }
        assert!(
            Instant::now() < deadline,
            "candidate rejection timeout: {status:?}"
        );
        client.drain_for(Duration::from_millis(250)).unwrap();
    }
}
