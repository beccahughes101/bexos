use bexos_crypto::blake3_256;
use bexos_debug_client::{DebugClient, DebugClientError, DebugTransport};
use bexos_e2e::{
    BEXFS_MARKERS, DebugSession, E2eContext, E2eDevice, NVME_MARKERS, VIRTIO_NET_MARKERS,
    boot_markers,
};
use bexos_qemu_test::{QemuArtifacts, QemuDevice};
use bexos_update::{ArtifactKind, build_signed_manifest};
mod firmware_activation;
mod firmware_staging;

const KEY_ID: [u8; 32] = *b"bexos-qemu-test-ed25519-key-v001";
const SEED: [u8; 32] = [
    0x9d, 0x61, 0xb1, 0x9d, 0xef, 0xfd, 0x5a, 0x60, 0xba, 0x84, 0x4a, 0xf4, 0x92, 0xec, 0x2c, 0xc4,
    0x44, 0x49, 0xc5, 0x69, 0x7b, 0x32, 0x69, 0x19, 0x70, 0x3b, 0xac, 0x03, 0x1c, 0xae, 0x7f, 0x60,
];

const COLD_MARKERS: &[&str] = &[
    "appd: guest SYS_STATE durable generation=",
    "appd: process ready package=bexos.driver.pci_root",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ServiceFlag {
    Development,
    DebugdPartialUpload,
    NvmeDmaMarkers,
    UpdateEngineFaults,
}

#[derive(Clone, Debug)]
struct ServiceUpdate {
    package: String,
    generation: u64,
    archive: String,
}

#[derive(Debug)]
enum Scenario {
    AppPackage {
        archive: String,
    },
    KernelPlatform {
        update_kernel: String,
    },
    Service {
        target: ServiceUpdate,
        prerequisites: Vec<ServiceUpdate>,
        prelude_app_archive: Option<String>,
        prelude_tee: bool,
        flags: Vec<ServiceFlag>,
        fault_archives: Vec<String>,
    },
    TeeImage,
    FirmwareStaging {
        bundle: String,
    },
    FirmwareActivation {
        paths: Vec<String>,
        reboot: bool,
    },
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let context = E2eContext::from_env("update");
    eprintln!("e2e: update harness start");
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let (artifacts, extra) = QemuArtifacts::from_env_or_args(&args)?;
    let scenario = Scenario::parse(&extra)?;
    if let Scenario::FirmwareActivation { paths, reboot } = &scenario {
        return firmware_activation::run(artifacts, paths, *reboot);
    }
    if let Scenario::Service {
        flags, prelude_tee, ..
    } = &scenario
    {
        if flags.contains(&ServiceFlag::Development) != artifacts.development {
            return Err(
                "service fixture security profile does not match the selected product".into(),
            );
        }
        if artifacts.development {
            if *prelude_tee {
                return Err("secure setup is unavailable in the development fixture".into());
            }
            eprintln!(
                "e2e: explicit development fixture; protected storage and integrated Trusty acceptance are unavailable"
            );
        }
    }
    eprintln!("e2e: booting debug session");
    let mut session = context.run_phase("boot", || boot_debug_session(artifacts))?;
    eprintln!("e2e: debug session ready");
    if std::env::var("BEXOS_E2E_TIER").as_deref() == Ok("presubmit") {
        context.run_phase("platform-smoke", || verify_platform_smoke(&mut session))?;
    }
    let outcome = context.run_phase("scenario", || match scenario {
        Scenario::AppPackage { archive } => run_app_package_update(&mut session, &archive),
        Scenario::KernelPlatform { update_kernel } => {
            run_kernel_platform_update(&mut session, &update_kernel)
        }
        Scenario::Service {
            target,
            prerequisites,
            prelude_app_archive,
            prelude_tee,
            flags,
            fault_archives,
        } => run_service_update(
            &mut session,
            target,
            prerequisites,
            prelude_app_archive,
            prelude_tee,
            &flags,
            &fault_archives,
        ),
        Scenario::TeeImage => run_tee_image_update(&mut session),
        Scenario::FirmwareStaging { bundle } => firmware_staging::run(&mut session, &bundle),
        Scenario::FirmwareActivation { .. } => unreachable!(),
    });
    outcome.map_err(|error| {
        format!(
            "{error}\n{}",
            String::from_utf8_lossy(session.client.received_trace())
        )
    })
}

impl Scenario {
    fn parse(args: &[String]) -> Result<Self, String> {
        let scenario = args.first().ok_or("missing scenario name")?.as_str();
        match scenario {
            "app_package" => Ok(Self::AppPackage {
                archive: args.get(1).ok_or("missing app archive")?.clone(),
            }),
            "kernel_platform" => Ok(Self::KernelPlatform {
                update_kernel: args.get(1).ok_or("missing replacement kernel")?.clone(),
            }),
            "service" => {
                let target = parse_service_update(args, 1)?;
                let mut flags = Vec::new();
                let mut fault_archives = Vec::new();
                let mut prerequisites = Vec::new();
                let mut prelude_app_archive = None;
                let mut prelude_tee = false;
                let mut i = 4;
                while let Some(arg) = args.get(i) {
                    match arg.as_str() {
                        "--debugd-partial-upload" => flags.push(ServiceFlag::DebugdPartialUpload),
                        "--nvme-dma-markers" => flags.push(ServiceFlag::NvmeDmaMarkers),
                        "--prelude-app" => {
                            prelude_app_archive = Some(
                                args.get(i + 1)
                                    .ok_or("missing prelude app archive")?
                                    .clone(),
                            );
                            i += 1;
                        }
                        "--prelude-tee" => prelude_tee = true,
                        "--development" => flags.push(ServiceFlag::Development),
                        "--prerequisite" => {
                            prerequisites.push(parse_service_update(args, i + 1)?);
                            i += 3;
                        }
                        "--update-engine-faults" => {
                            flags.push(ServiceFlag::UpdateEngineFaults);
                            fault_archives = args
                                .get(i + 1..i + 5)
                                .ok_or("missing update engine fault archives")?
                                .to_vec();
                            i += 4;
                        }
                        other => return Err(format!("unknown service flag {other}")),
                    }
                    i += 1;
                }
                Ok(Self::Service {
                    target,
                    prerequisites,
                    prelude_app_archive,
                    prelude_tee,
                    flags,
                    fault_archives,
                })
            }
            "tee_image" => Ok(Self::TeeImage),
            "firmware_live" | "firmware_reboot" => Ok(Self::FirmwareActivation {
                paths: args[1..].to_vec(),
                reboot: args[0] == "firmware_reboot",
            }),
            "firmware_staging" => Ok(Self::FirmwareStaging {
                bundle: args.get(1).ok_or("missing signed firmware")?.clone(),
            }),
            other => Err(format!("unknown update scenario {other}")),
        }
    }
}

fn parse_service_update(args: &[String], offset: usize) -> Result<ServiceUpdate, String> {
    Ok(ServiceUpdate {
        package: args
            .get(offset)
            .ok_or("missing service package id")?
            .clone(),
        generation: args
            .get(offset + 1)
            .ok_or("missing service generation")?
            .parse()
            .map_err(|_| "invalid service generation")?,
        archive: args
            .get(offset + 2)
            .ok_or("missing service archive")?
            .clone(),
    })
}

fn boot_debug_session(
    artifacts: QemuArtifacts,
) -> Result<DebugSession<<QemuDevice as E2eDevice>::DebugTransport>, String> {
    let mut markers = boot_markers();
    markers.extend_from_slice(NVME_MARKERS);
    markers.extend_from_slice(BEXFS_MARKERS);
    markers.extend_from_slice(VIRTIO_NET_MARKERS);
    markers.push(b"teed: service ready");
    markers.push(b"appd: app lifecycle registry ready for debugd");
    markers.push(b"debugd: tee proxy connected");
    let mut device = QemuDevice::new(artifacts)?;
    let mut session = device.boot_with_debugd(&markers)?;
    session.assert_debugd_ready()?;
    session.client.clear_received_trace();
    Ok(session)
}

fn verify_platform_smoke<T: DebugTransport>(session: &mut DebugSession<T>) -> Result<(), String> {
    let apps = session
        .client
        .list_apps()
        .map_err(|error| format!("presubmit app registry: {error:?}"))?;
    for package in [
        "bexos.platform.storage_verify",
        "bexos.service.netstackd",
        "bexos.service.timed",
    ] {
        let app = apps
            .iter()
            .find(|app| app.package_id == package)
            .ok_or_else(|| format!("presubmit registry missing {package}"))?;
        if app.state != "Running" || !app.protected {
            return Err(format!("presubmit app had unexpected state: {app:?}"));
        }
    }
    let mut trace = session.traced_test("presubmit", bexos_trace::CATEGORY_DEBUG_SERVICE)?;
    trace
        .session()
        .client
        .health_check()
        .map_err(|error| format!("presubmit traced health: {error:?}"))?;
    let analysis = trace.finish()?;
    analysis.assert_event_present("debugd:health_check")
}

fn run_app_package_update<T: DebugTransport>(
    session: &mut DebugSession<T>,
    registry_test_archive: &str,
) -> Result<(), String> {
    let archive =
        std::fs::read(registry_test_archive).map_err(|e| format!("read test archive: {e}"))?;
    eprintln!("e2e: uploading app update");
    let app_manifest = build_signed_manifest(
        10,
        "com.example.registry_verify",
        ArtifactKind::AppPackage,
        &archive,
        KEY_ID,
        SEED,
    );
    session
        .client
        .upload_update(0x7570_6401, &app_manifest, &archive)
        .map_err(|e| format!("upload app update: {e:?}"))?;
    let response = session
        .client
        .exec_command("update.apply_app", &[])
        .map_err(|e| format!("apply app update: {e:?}"))?;
    if response.exit_code != 0 {
        return Err(format!("app update failed: {response:?}"));
    }
    eprintln!("e2e: app update applied");
    let apps = session.client.list_apps().map_err(|e| format!("{e:?}"))?;
    if !apps
        .iter()
        .any(|app| app.package_id == "com.example.registry_verify" && !app.protected)
    {
        return Err(format!("updated app missing from registry: {apps:?}"));
    }
    match session
        .client
        .uninstall_app("bexos.platform.storage_verify")
    {
        Err(DebugClientError::RemoteStatus(status)) if status.status == -10 => Ok(()),
        other => Err(format!(
            "protected storage verifier uninstall should be denied, got {other:?}"
        )),
    }
}

fn run_tee_image_update<T: DebugTransport>(session: &mut DebugSession<T>) -> Result<(), String> {
    eprintln!("e2e: verifying tee proxy");
    verify_tee_proxy(session)?;
    eprintln!("e2e: tee proxy verified");
    let tee_bad_artifact = b"untrusted tee replacement";
    let mut tee_bad_manifest = build_signed_manifest(
        6,
        &guest_platform_name("tee"),
        ArtifactKind::TeeImage,
        tee_bad_artifact,
        KEY_ID,
        SEED,
    );
    tee_bad_manifest[120] ^= 0x55;
    session
        .client
        .upload_update(0x7570_6402, &tee_bad_manifest, tee_bad_artifact)
        .map_err(|e| format!("bad tee signature upload: {e:?}"))?;
    let bad_tee = session
        .client
        .exec_command("update.apply_platform", &[])
        .map_err(|e| format!("bad tee signature apply: {e:?}"))?;
    if bad_tee.exit_code == 0 || !bad_tee.stderr.contains("BadSignature") {
        return Err(format!("bad TEE signature accepted: {bad_tee:?}"));
    }

    let tee_artifact = build_tee_update_bundle(9, &guest_platform_name("tee"));
    let tee_manifest = build_signed_manifest(
        9,
        &guest_platform_name("tee"),
        ArtifactKind::TeeImage,
        &tee_artifact,
        KEY_ID,
        SEED,
    );
    session
        .client
        .upload_update(0x7570_6403, &tee_manifest, &tee_artifact)
        .map_err(|e| format!("tee update upload: {e:?}"))?;
    let tee_applied = session
        .client
        .exec_command("update.apply_platform", &[])
        .map_err(|e| {
            format!(
                "tee update apply: {e:?}\n{}",
                String::from_utf8_lossy(session.client.received_trace())
            )
        })?;
    if tee_applied.exit_code != update_manager_fidl::UpdateStatus::ErrAccessDenied as i32
        || !tee_applied.stderr.contains("ErrAccessDenied")
    {
        return Err(format!(
            "unsupported secure firmware activation was not explicitly rejected: {tee_applied:?}"
        ));
    }
    let tee_status = session
        .client
        .tee_update_status()
        .map_err(|e| format!("tee update status: {e:?}"))?;
    if tee_status.update_status == "Completed" {
        return Err(format!(
            "uninstalled firmware reported completed: {tee_status:?}"
        ));
    }
    verify_tee_proxy(session)?;
    Ok(())
}

fn build_tee_update_bundle(generation: u64, target: &str) -> Vec<u8> {
    let slot_a = b"qemu-trusty-slot-a-image";
    let slot_b = b"qemu-trusty-slot-b-image";
    let mut out = Vec::new();
    out.extend_from_slice(b"BEXTEEAB");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&1u32.to_le_bytes());
    out.extend_from_slice(&generation.to_le_bytes());
    out.extend_from_slice(&(target.len() as u16).to_le_bytes());
    out.extend_from_slice(target.as_bytes());
    out.extend_from_slice(&blake3_256(b"qemu-trusty-bundle-metadata"));
    out.extend_from_slice(&(15u16).to_le_bytes());
    out.extend_from_slice(b"bexos-qemu-test");
    out.extend_from_slice(&blake3_256(slot_a));
    out.extend_from_slice(&0x0e10_0000u64.to_le_bytes());
    out.extend_from_slice(&(slot_a.len() as u64).to_le_bytes());
    out.extend_from_slice(&(slot_a.len() as u64).to_le_bytes());
    out.extend_from_slice(&blake3_256(slot_b));
    out.extend_from_slice(&0x0e88_0000u64.to_le_bytes());
    out.extend_from_slice(&(slot_b.len() as u64).to_le_bytes());
    out.extend_from_slice(&(slot_b.len() as u64).to_le_bytes());
    out.extend_from_slice(slot_a);
    out.extend_from_slice(slot_b);
    let signature = blake3_256(&out);
    out.extend_from_slice(&signature);
    out
}

fn verify_tee_proxy<T: DebugTransport>(session: &mut DebugSession<T>) -> Result<(), String> {
    eprintln!("e2e: tee info");
    let tee_info = session
        .client
        .tee_info()
        .map_err(|e| format!("tee info: {e:?}"))?;
    if !tee_info.present {
        return Err(format!("TEE should be present in QEMU model: {tee_info:?}"));
    }
    let dynamic_ta = build_dynamic_ta_package([0x51; 16], 1, b"qemu tee app");
    match session.client.tee_install_app(dynamic_ta) {
        Err(DebugClientError::RemoteStatus(status))
            if status.status == tee_manager_fidl::TeeStatus::ErrAccessDenied as i32 => {}
        other => {
            return Err(format!(
                "non-executable TA was not explicitly rejected: {other:?}"
            ));
        }
    }
    for command in ["tee.orchestrator_smoke", "tee.keymint_smoke"] {
        eprintln!("e2e: {command}");
        let result = session
            .client
            .exec_command(command, &[])
            .map_err(|e| format!("{command}: {e:?}"))?;
        if result.exit_code != 0 {
            return Err(format!("{command} failed: {result:?}"));
        }
    }
    Ok(())
}

fn build_dynamic_ta_package(uuid: [u8; 16], version: u32, payload: &[u8]) -> Vec<u8> {
    let platform_name = guest_platform_name("trusty");
    let platform = platform_name.as_bytes();
    let entry = b"qemu_dynamic_ta";
    let mut out = Vec::new();
    out.extend_from_slice(b"BEXTA2\0\0");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&uuid);
    out.extend_from_slice(&version.to_le_bytes());
    out.extend_from_slice(&(platform.len() as u16).to_le_bytes());
    out.extend_from_slice(platform);
    out.extend_from_slice(&(entry.len() as u16).to_le_bytes());
    out.extend_from_slice(entry);
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(payload);
    let signature = blake3_256(&out);
    out.extend_from_slice(&signature);
    out
}

fn run_service_update<T: DebugTransport>(
    session: &mut DebugSession<T>,
    target: ServiceUpdate,
    prerequisites: Vec<ServiceUpdate>,
    prelude_app_archive: Option<String>,
    prelude_tee: bool,
    flags: &[ServiceFlag],
    fault_archives: &[String],
) -> Result<(), String> {
    // A started process is visible before appd accepts its Ready message and
    // installs the lifecycle endpoint needed to stage a replacement.
    for update in prerequisites.iter().chain(core::iter::once(&target)) {
        if update.package != "bexos.platform.appd" {
            let marker = format!("appd: process ready package={}", update.package);
            session.wait_for_serial_markers(
                &[marker.as_bytes()],
                std::time::Duration::from_secs(90),
            )?;
        }
    }
    if prelude_tee {
        run_tee_image_update(session)?;
    }
    if let Some(archive) = prelude_app_archive {
        run_app_package_update(session, &archive)?;
    }
    let apps = session
        .client
        .list_apps()
        .map_err(|e| format!("initial app registry: {e:?}"))?;
    let initial_probe = launch_persistent_client(session)?;
    for prerequisite in prerequisites {
        let prerequisite_flags = if flags.contains(&ServiceFlag::Development) {
            &[ServiceFlag::Development][..]
        } else {
            &[]
        };
        run_one_service_update(
            session,
            &apps,
            initial_probe,
            &prerequisite,
            prerequisite_flags,
            &[],
        )?;
    }
    if flags.contains(&ServiceFlag::DebugdPartialUpload) {
        begin_partial_debugd_upload(session)?;
    }
    if flags.contains(&ServiceFlag::UpdateEngineFaults) {
        run_updated_faults(session, &target.package, target.generation, fault_archives).map_err(
            |error| {
                format!(
                    "{error}\n{}",
                    String::from_utf8_lossy(session.client.received_trace()),
                )
            },
        )?;
    }
    run_one_service_update(
        session,
        &apps,
        initial_probe,
        &target,
        flags,
        fault_archives,
    )
}

fn run_one_service_update<T: DebugTransport>(
    session: &mut DebugSession<T>,
    apps: &[bexos_debug_wire::AppInfo],
    initial_probe: u64,
    update: &ServiceUpdate,
    flags: &[ServiceFlag],
    _fault_archives: &[String],
) -> Result<(), String> {
    let package = update.package.as_str();
    let generation = update.generation;

    let secure_transport = !flags.contains(&ServiceFlag::Development)
        && matches!(
            package,
            "bexos.service.teed" | "bexos.driver.serial.virtio_console"
        );
    let secure_uid = 2200 + generation;
    if secure_transport {
        session
            .client
            .create_user(
                secure_uid,
                "migration",
                "RPMB migration",
                "migration-password",
            )
            .map_err(|e| format!("enroll before {package} migration: {e:?}"))?;
        // Establish a durable user volume before cutover so the post-update
        // check exercises reopening existing protected storage.
        session
            .client
            .unlock_user(secure_uid, "migration-password")
            .map_err(|e| format!("unlock baseline before {package} migration: {e:?}"))?;
        session
            .client
            .lock_user(secure_uid)
            .map_err(|e| format!("lock baseline before {package} migration: {e:?}"))?;
    }
    eprintln!("e2e: replacing {package} generation={generation}");
    let before = session
        .client
        .list_processes()
        .map_err(|e| format!("{e:?}"))?;
    let matching: Vec<_> = before
        .iter()
        .filter(|process| process.package_id == package && process.state == "Running")
        .collect();
    if matching.is_empty() {
        return Err(format!(
            "expected a running process for {package}: {before:?}"
        ));
    }
    let before_probe = initial_probe;
    let archive_id = stored_archive_id(package, generation, None);
    session.client.clear_received_trace();
    let applied = session
        .client
        .exec_command(
            "update.apply_stored_service",
            &[package.to_string(), generation.to_string(), archive_id],
        )
        .map_err(|e| {
            format!(
                "service apply {e:?}\n{}",
                String::from_utf8_lossy(session.client.received_trace())
            )
        })?;
    if applied.exit_code != 0 {
        return Err(format!(
            "{package} staging failed: {applied:?}\n{}",
            String::from_utf8_lossy(session.client.received_trace())
        ));
    }
    let deadline_secs = if matches!(
        package,
        "bexos.driver.storage.bexfs" | "bexos.driver.storage.archivefs"
    ) {
        660
    } else {
        180
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(deadline_secs);
    let mut last_progress = String::new();
    loop {
        let trace = String::from_utf8_lossy(session.client.received_trace());
        if let Some(progress) = trace.lines().rev().find(|line| {
            line.starts_with("migration: target adopting record")
                || line.starts_with("migration: target bulk adoption complete")
                || line.starts_with("migration: live catch-up")
                || line.starts_with("migration: final dirty records=")
        }) {
            if progress != last_progress {
                eprintln!("e2e: {package}: {progress}");
                last_progress = progress.to_string();
            }
        }
        if trace.contains("appd: migration task failed:") {
            return Err(format!("{package} transplant failed\n{trace}"));
        }
        if service_cutover_ms(session.client.received_trace(), generation).is_some() {
            break;
        }
        if std::time::Instant::now() >= deadline {
            return Err(format!(
                "{package} transplant did not commit\n{}",
                String::from_utf8_lossy(session.client.received_trace())
            ));
        }
        session
            .client
            .drain_for(std::time::Duration::from_millis(25))
            .map_err(|e| format!("service trace drain {e:?}"))?;
    }
    session
        .client
        .drain_for(std::time::Duration::from_secs(2))
        .map_err(|e| format!("service post-commit trace drain {e:?}"))?;
    session.client.health_check().map_err(|e| {
        format!(
            "service health after commit {e:?}\n{}",
            String::from_utf8_lossy(session.client.received_trace())
        )
    })?;
    let live_apps = session
        .client
        .list_apps()
        .map_err(|e| format!("live registry {e:?}"))?;
    if !live_apps.is_empty() && live_apps != apps {
        return Err(format!(
            "app registry changed during migration\nbefore={apps:?}\nafter={live_apps:?}"
        ));
    }
    let after_probe = probe_until(&mut session.client, deadline)?;
    if after_probe <= before_probe {
        return Err(format!(
            "persistent client made no progress during {package} transplant ({before_probe} -> {after_probe})\n{}",
            String::from_utf8_lossy(session.client.received_trace())
        ));
    }
    let cutover = service_cutover_ms(session.client.received_trace(), generation)
        .ok_or_else(|| format!("missing cutover measurement for {package}"))?;
    if cutover > 150 {
        return Err(format!(
            "{package} committed after cutover deadline: {cutover}ms"
        ));
    }
    if flags.contains(&ServiceFlag::DebugdPartialUpload) {
        finish_partial_debugd_upload(session)?;
    }
    if flags.contains(&ServiceFlag::NvmeDmaMarkers) {
        // Kernel commit precedes appd's activation response and DMA evidence.
        // Read that evidence after activation, never a synthesized commit line.
        let committed_message = loop {
            if let Some(message) = service_completion_message(session.client.received_trace()) {
                break message;
            }
            if std::time::Instant::now() >= deadline {
                return Err(format!("{package} activation DMA evidence missing"));
            }
            session
                .client
                .drain_for(std::time::Duration::from_millis(25))
                .map_err(|e| format!("DMA evidence trace drain {e:?}"))?;
        };
        assert_nvme_dma_markers_preserved(&committed_message)?;
    }
    assert_process_replaced(session, package, &before)?;
    let reclaimed_deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let commit_marker =
        format!("service-transplant: committed generation={generation} cutover_ms=");
    loop {
        let trace = String::from_utf8_lossy(session.client.received_trace());
        if trace.split_once(&commit_marker).is_some_and(|(_, after)| {
            after.contains("kernel: retired private memory fully reclaimed")
        }) {
            break;
        }
        if std::time::Instant::now() >= reclaimed_deadline {
            return Err(format!(
                "{package} retired memory was not fully reclaimed after commit"
            ));
        }
        session
            .client
            .drain_for(std::time::Duration::from_millis(25))
            .map_err(|e| format!("reclamation trace drain {e:?}"))?;
    }
    if probe_until(
        &mut session.client,
        std::time::Instant::now() + std::time::Duration::from_secs(10),
    )? <= initial_probe
    {
        return Err("persistent client stalled".into());
    }
    if secure_transport {
        session
            .client
            .unlock_user(secure_uid, "migration-password")
            .map_err(|e| format!("secure storage unavailable after {package} migration: {e:?}"))?;
        session
            .client
            .lock_user(secure_uid)
            .map_err(|e| format!("lock migrated user: {e:?}"))?;
        session
            .client
            .delete_user(secure_uid)
            .map_err(|e| format!("delete migrated user: {e:?}"))?;
    }
    eprintln!("e2e: {package} replacement completed; health/registry/progress verified");
    eprintln!("e2e: service cutover measurement {package}={cutover}ms");
    Ok(())
}

fn stored_archive_id(package: &str, generation: u64, suffix: Option<&str>) -> String {
    match suffix {
        Some(suffix) => format!("{package}-{generation}-{suffix}"),
        None => format!("{package}-{generation}"),
    }
}

fn launch_persistent_client<T: DebugTransport>(
    session: &mut DebugSession<T>,
) -> Result<u64, String> {
    session
        .client
        .launch_app("bexos.platform.storage_verify", "storage_verify", 0xbeef, 0)
        .map_err(|e| {
            format!(
                "persistent client launch {e:?}\n{}",
                String::from_utf8_lossy(session.client.received_trace())
            )
        })?;
    // Launch is acknowledged before appd finishes its persistent registry sync.
    // Wait for startup readiness within one bounded budget before measuring any
    // transplant. The short post-cutover progress checks remain separate.
    let baseline_deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
    let initial_probe = probe_until(&mut session.client, baseline_deadline)?;
    eprintln!("e2e: persistent client initial completed={initial_probe}");
    loop {
        let current = probe_until(&mut session.client, baseline_deadline)?;
        if current > initial_probe {
            eprintln!("e2e: persistent client baseline completed={current}");
            return Ok(initial_probe);
        }
        if std::time::Instant::now() >= baseline_deadline {
            return Err(format!(
                "persistent client stalled before transplant at {current}\n{}",
                String::from_utf8_lossy(session.client.received_trace())
            ));
        }
    }
}

fn begin_partial_debugd_upload<T: DebugTransport>(
    session: &mut DebugSession<T>,
) -> Result<(), String> {
    let partial_manifest = b"manifest-across-debugd-transplant";
    let partial_elf = b"elf-bytes-across-debugd-transplant";
    session
        .client
        .begin_test_app_upload(
            0xd06d,
            "com.example.partial",
            partial_manifest.len() as u64,
            partial_elf.len() as u64,
        )
        .map_err(|e| format!("partial upload begin {e:?}"))?;
    session
        .client
        .write_test_app_stream_from(0xd06d, 1, 0, &partial_manifest[..9])
        .map_err(|e| format!("partial manifest {e:?}"))?;
    session
        .client
        .write_test_app_stream_from(0xd06d, 2, 0, &partial_elf[..7])
        .map_err(|e| format!("partial elf {e:?}"))
}

fn finish_partial_debugd_upload<T: DebugTransport>(
    session: &mut DebugSession<T>,
) -> Result<(), String> {
    let partial_manifest = b"manifest-across-debugd-transplant";
    let partial_elf = b"elf-bytes-across-debugd-transplant";
    session
        .client
        .write_test_app_stream_from(0xd06d, 1, 9, &partial_manifest[9..])
        .map_err(|e| format!("resumed manifest {e:?}"))?;
    session
        .client
        .write_test_app_stream_from(0xd06d, 2, 7, &partial_elf[7..])
        .map_err(|e| format!("resumed elf {e:?}"))?;
    session
        .client
        .commit_test_app_upload(0xd06d)
        .map_err(|e| format!("partial upload did not survive debugd transplant {e:?}"))?;
    eprintln!("e2e: partial debugd upload completed on original host connection");
    Ok(())
}

fn run_updated_faults<T: DebugTransport>(
    session: &mut DebugSession<T>,
    package: &str,
    generation: u64,
    fault_archives: &[String],
) -> Result<(), String> {
    if fault_archives.len() != 4 {
        return Err(format!(
            "expected four update engine fault archives, got {}",
            fault_archives.len()
        ));
    }
    let secure = matches!(
        package,
        "bexos.service.teed" | "bexos.driver.serial.virtio_console"
    );
    let uid = 2100 + generation;
    if secure {
        session
            .client
            .create_user(uid, "abort", "RPMB abort", "abort-password")
            .map_err(|e| format!("enroll before migration aborts: {e:?}"))?;
    }
    for (n, mode) in [
        "rejection",
        "incompatible state",
        "candidate failure",
        "timeout",
    ]
    .iter()
    .enumerate()
    {
        let archive_id = fault_archives[n].clone();
        let before = probe(&mut session.client)?;
        // The diagnostic trace is a bounded ring; clear inspected history so
        // evidence belongs to this candidate even if earlier traffic filled it.
        session.client.clear_received_trace();
        let applied = session
            .client
            .exec_command(
                "update.apply_stored_service",
                &[package.to_string(), generation.to_string(), archive_id],
            )
            .map_err(|e| format!("fault apply {e:?}"))?;
        if applied.exit_code != 0 {
            return Err(format!("fault candidate did not stage: {applied:?}"));
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let status = session
                .client
                .exec_command("update.service.status", &[package.to_string()])
                .map_err(|e| format!("fault status {e:?}"))?;
            if status.stdout.contains("pending=false") {
                if !status.stdout.contains("generation=0 ") {
                    return Err(format!("aborted generation activated: {status:?}"));
                }
                break;
            }
            if std::time::Instant::now() >= deadline {
                return Err(format!("{mode} did not roll back: {status:?}"));
            }
        }
        let rejection = match *mode {
            "incompatible state" => "migration: target record adoption failed",
            "timeout" => "appd: migration task failed: migration preparation timeout",
            _ => "appd: migration task failed: migration rejected status=-8",
        };
        if !String::from_utf8_lossy(session.client.received_trace()).contains(rejection) {
            return Err(format!(
                "{mode} rolled back without expected evidence: {rejection}"
            ));
        }
        if probe(&mut session.client)? <= before {
            return Err(format!("client stalled after {mode}"));
        }
        if secure {
            session
                .client
                .unlock_user(uid, "abort-password")
                .map_err(|e| format!("RPMB unavailable after {package} {mode}: {e:?}"))?;
            session
                .client
                .lock_user(uid)
                .map_err(|e| format!("lock after {package} {mode}: {e:?}"))?;
        }
        eprintln!("e2e: {mode} rolled back; old endpoints still serving");
    }
    if secure {
        session
            .client
            .delete_user(uid)
            .map_err(|e| format!("cleanup abort-test user: {e:?}"))?;
    }
    Ok(())
}

fn assert_nvme_dma_markers_preserved(committed_message: &str) -> Result<(), String> {
    let before_text = committed_message
        .split_once("before=")
        .and_then(|(_, v)| v.split_once(" markers=").map(|(v, _)| v))
        .ok_or("missing initial NVMe DMA addresses")?;
    let before_values = before_text
        .split(',')
        .map(str::parse::<u64>)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "invalid initial NVMe markers")?;
    let marker_text = committed_message
        .split_once("markers=")
        .map(|(_, v)| v.trim())
        .ok_or("missing NVMe adoption markers")?;
    let values = marker_text
        .split(',')
        .map(str::parse::<u64>)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "invalid NVMe adoption markers")?;
    if values != before_values {
        return Err(format!(
            "NVMe DMA addresses changed: {before_values:?} -> {values:?}"
        ));
    }
    Ok(())
}

fn assert_process_replaced<T: DebugTransport>(
    session: &mut DebugSession<T>,
    package: &str,
    before: &[bexos_debug_wire::ProcessInfo],
) -> Result<(), String> {
    let after = session
        .client
        .list_processes()
        .map_err(|e| format!("{e:?}"))?;
    // Appd migrates one managed instance per request. Integrated x86 has
    // separate debug and RPMB instances of the same console package. Identify
    // the retired instance from actual process state and require its peer to
    // remain unchanged, instead of assuming package IDs identify one process.
    let retired = before
        .iter()
        .filter(|p| p.package_id == package && p.state == "Running")
        .filter(|p| !after.iter().any(|q| q.pid == p.pid && q.state == "Running"))
        .collect::<Vec<_>>();
    if retired.len() != 1 {
        return Err(format!(
            "{package} did not retire exactly one instance: {before:?} -> {after:?}"
        ));
    }
    let old_pid = retired[0].pid;
    for p in before.iter().filter(|p| p.pid != old_pid) {
        if !after.contains(p) {
            return Err(format!("unaffected process changed: {p:?}"));
        }
    }
    if after
        .iter()
        .any(|p| p.pid == old_pid && p.state == "Running")
        || after.len() != before.len() + 1
        || after
            .iter()
            .filter(|p| {
                p.package_id == package
                    && p.state == "Running"
                    && !before.iter().any(|old| old.pid == p.pid)
            })
            .count()
            != 1
    {
        return Err(format!(
            "{package} process not replaced: {before:?} -> {after:?}"
        ));
    }
    Ok(())
}

fn run_kernel_platform_update<T: DebugTransport>(
    session: &mut DebugSession<T>,
    update_kernel: &str,
) -> Result<(), String> {
    let initial_probe = launch_persistent_client(session)?;
    session
        .client
        .launch_app("bexos.platform.storage_verify", "storage_verify", 1, 0)
        .map_err(|e| {
            format!(
                "appd launch before transplant {e:?}\n{}",
                String::from_utf8_lossy(session.client.received_trace())
            )
        })?;
    let bad_artifact = b"untrusted replacement";
    let mut bad_manifest = build_signed_manifest(
        5,
        &guest_platform_name("kernel"),
        ArtifactKind::Microkernel,
        bad_artifact,
        KEY_ID,
        SEED,
    );
    bad_manifest[120] ^= 0x55;
    session
        .client
        .upload_update(0x7570_6400, &bad_manifest, bad_artifact)
        .map_err(|e| format!("bad signature upload: {e:?}"))?;
    let bad = session
        .client
        .exec_command("update.apply_platform", &[])
        .map_err(|e| format!("bad signature apply: {e:?}"))?;
    if bad.exit_code == 0 || !bad.stderr.contains("BadSignature") {
        return Err(format!("bad signature accepted: {bad:?}"));
    }
    let idle = session
        .client
        .exec_command("kernel.update.status", &[])
        .map_err(|e| format!("pre-update status: {e:?}"))?;
    if !idle.stdout.contains("Idle") {
        return Err(format!("bad signature reached kernel staging: {idle:?}"));
    }

    eprintln!("e2e: reading replacement kernel");
    let before_processes = session
        .client
        .list_processes()
        .map_err(|e| format!("pre-transplant ps: {e:?}"))?;
    let before_apps = session
        .client
        .list_apps()
        .map_err(|e| format!("pre-transplant apps: {e:?}"))?;
    let platform_artifact =
        std::fs::read(update_kernel).map_err(|e| format!("read replacement kernel: {e}"))?;
    let rejected_artifact = b"not an ELF replacement";
    let rejected_manifest = build_signed_manifest(
        20,
        &guest_platform_name("kernel"),
        ArtifactKind::Microkernel,
        rejected_artifact,
        KEY_ID,
        SEED,
    );
    session
        .client
        .upload_update(0x7570_6402, &rejected_manifest, rejected_artifact)
        .map_err(|e| format!("upload rejected kernel candidate {e:?}"))?;
    let rejected = session
        .client
        .exec_command("update.apply_platform", &[])
        .map_err(|e| format!("reject kernel candidate transport {e:?}"))?;
    if rejected.exit_code == 0 {
        return Err("invalid replacement kernel reached preparation".into());
    }
    session
        .client
        .health_check()
        .map_err(|e| format!("old kernel did not serve after candidate rejection {e:?}"))?;
    eprintln!("e2e: invalid kernel candidate rejected; old kernel still serving");
    let platform_manifest = build_signed_manifest(
        20,
        &guest_platform_name("kernel"),
        ArtifactKind::Microkernel,
        &platform_artifact,
        KEY_ID,
        SEED,
    );
    session
        .client
        .upload_update(0x7570_6404, &platform_manifest, &platform_artifact)
        .map_err(|e| format!("upload platform update: {e:?}"))?;
    eprintln!("e2e: applying platform update");
    use bexos_debug_wire::{
        ExecRequest, METHOD_EXEC_COMMAND, METHOD_HEALTH_CHECK, METHOD_LIST_APPS, decode_app_list,
        decode_exec_response, decode_health_response, encode_empty, encode_exec_request,
    };
    let command = |name: &str| {
        let mut payload = Vec::new();
        encode_exec_request(
            &ExecRequest {
                component_id: name.into(),
                args: vec![],
            },
            &mut payload,
        );
        (METHOD_EXEC_COMMAND, payload)
    };
    let mut empty = Vec::new();
    encode_empty(&mut empty);
    // Queue activation and retained-client requests in one transport write.
    // Require every response across the transition. A fast bulk transfer may
    // finish before the first status sample; observing that transient phase is
    // not a prerequisite for continuity. The trace below proves bulk execution.
    let mut requests = vec![
        command("update.apply_platform"),
        command("kernel.update.status"),
        (METHOD_HEALTH_CHECK, empty.clone()),
        (METHOD_LIST_APPS, empty),
        command("kernel.update.status"),
    ];
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(45);
    loop {
        let frames = session
            .client
            .call_batch(&requests)
            .map_err(|e| format!("live update batch: {e:?}"))?;
        let offset = if requests.len() == 5 {
            let response = decode_exec_response(&frames[0].payload)
                .map_err(|e| format!("apply response: {e:?}"))?;
            eprintln!("e2e: platform response {response:?}");
            if response.exit_code != 0 {
                return Err(format!("platform update failed: {response:?}"));
            }
            1
        } else {
            0
        };
        let before = decode_exec_response(&frames[offset].payload)
            .map_err(|e| format!("bulk status: {e:?}"))?;
        let health = decode_health_response(&frames[offset + 1].payload)
            .map_err(|e| format!("bulk health: {e:?}"))?;
        let apps = decode_app_list(&frames[offset + 2].payload)
            .map_err(|e| format!("bulk registry: {e:?}"))?;
        let after = decode_exec_response(&frames[offset + 3].payload)
            .map_err(|e| format!("bulk status: {e:?}"))?;
        if before.exit_code != 0 || after.exit_code != 0 {
            return Err(format!(
                "queued update status failed: {before:?}, {after:?}"
            ));
        }
        if health.status != "SERVING" || apps != before_apps {
            return Err(format!(
                "health or registry changed during transplant: {health:?}, {apps:?}"
            ));
        }
        eprintln!(
            "e2e: live request interval {:?} -> {:?}",
            before.stdout, after.stdout
        );
        if after.stdout.contains("Completed") && after.stdout.contains("generation=20") {
            break;
        }
        if after.stdout.contains("Failed") || std::time::Instant::now() >= deadline {
            return Err(format!(
                "transplant did not complete: {after:?}\n{}",
                String::from_utf8_lossy(session.client.received_trace())
            ));
        }
        if offset == 1 {
            requests.remove(0);
        }
    }
    if probe(&mut session.client)? <= initial_probe {
        return Err("persistent client made no progress across kernel transplant".into());
    }
    let after_processes = session
        .client
        .list_processes()
        .map_err(|e| format!("post-transplant ps: {e:?}"))?;
    let after_apps = session
        .client
        .list_apps()
        .map_err(|e| format!("post-transplant apps: {e:?}"))?;
    if before_processes != after_processes || before_apps != after_apps {
        return Err(format!(
            "preserved process/registry identities changed: {before_processes:?} -> {after_processes:?}, {before_apps:?} -> {after_apps:?}\n{}",
            String::from_utf8_lossy(session.client.received_trace())
        ));
    }
    bexos_e2e::assert_markers(
        session.client.received_trace(),
        &[
            b"heart-transplant: live bulk sync started; userspace remains scheduled",
            b"heart-transplant: live bulk sync complete; catching up mutations",
            b"heart-transplant: SWITCH snapshot sealed; jumping to replacement",
            b"heart-transplant: replacement kernel entered",
            b"heart-transplant: old kernel reclaimed; allocation from old range verified",
            b"debugd: resumed after kernel transplant",
        ],
    )?;
    let trace = String::from_utf8_lossy(session.client.received_trace());
    let kernel_cutover = trace
        .lines()
        .find_map(|line| {
            line.split_once("heart-transplant: precommit cutover_ms=")
                .and_then(|(_, v)| v.trim().parse::<u64>().ok())
        })
        .ok_or("missing kernel cutover measurement")?;
    if kernel_cutover > 150 {
        return Err(format!(
            "kernel precommit exceeded deadline: {kernel_cutover}ms"
        ));
    }
    eprintln!("e2e: kernel cutover measurement {kernel_cutover}ms");
    for marker in COLD_MARKERS {
        if trace.contains(marker) {
            return Err(format!("cold initialization repeated: {marker}"));
        }
    }
    eprintln!("e2e: replacement takeover, preserved processes/apps, and reclamation verified");

    let rollback_manifest = build_signed_manifest(
        19,
        &guest_platform_name("kernel"),
        ArtifactKind::Microkernel,
        &platform_artifact,
        KEY_ID,
        SEED,
    );
    session
        .client
        .upload_update(0x7570_6403, &rollback_manifest, &platform_artifact)
        .map_err(|e| format!("upload rollback update: {e:?}"))?;
    let rollback = session
        .client
        .exec_command("update.apply_platform", &[])
        .map_err(|e| format!("apply rollback update transport: {e:?}"))?;
    if rollback.exit_code == 0 || !rollback.stderr.contains("Rollback") {
        return Err(format!("rollback update was not rejected: {rollback:?}"));
    }
    let final_status = session
        .client
        .exec_command("kernel.update.status", &[])
        .map_err(|e| format!("final kernel status: {e:?}"))?;
    if !final_status.stdout.contains("Completed") || !final_status.stdout.contains("generation=20")
    {
        return Err(format!("rollback changed live kernel: {final_status:?}"));
    }
    let switch_marker = b"heart-transplant: SWITCH snapshot sealed; jumping to replacement";
    if session
        .client
        .received_trace()
        .windows(switch_marker.len())
        .filter(|bytes| *bytes == switch_marker)
        .count()
        != 1
    {
        return Err("expected exactly one kernel switch".into());
    }
    Ok(())
}

fn service_cutover_ms(trace: &[u8], generation: u64) -> Option<u64> {
    let trace = String::from_utf8_lossy(trace);
    let prefix = format!("service-transplant: committed generation={generation} cutover_ms=");
    trace.lines().find_map(|line| {
        line.split_once(&prefix)
            .and_then(|(_, v)| v.trim().parse::<u64>().ok())
    })
}

fn service_completion_message(trace: &[u8]) -> Option<String> {
    let trace = String::from_utf8_lossy(trace);
    trace.lines().rev().find_map(|line| {
        line.split_once("appd: migration completed ")
            .map(|(_, v)| v.into())
    })
}

fn probe<T: DebugTransport>(client: &mut DebugClient<T>) -> Result<u64, String> {
    probe_until(
        client,
        std::time::Instant::now() + std::time::Duration::from_secs(2),
    )
}

fn probe_until<T: DebugTransport>(
    client: &mut DebugClient<T>,
    deadline: std::time::Instant,
) -> Result<u64, String> {
    let r = loop {
        let r = client.exec_command("app.progress", &[]).map_err(|e| {
            format!(
                "progress {e:?}\n{}",
                String::from_utf8_lossy(client.received_trace())
            )
        })?;
        if !(r.exit_code == -6 && r.stderr.trim() == "progress transport")
            || std::time::Instant::now() >= deadline
        {
            break r;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    };
    if r.exit_code != 0 || !r.stdout.contains("errors=0") {
        return Err(format!(
            "persistent client failed: {r:?}\n{}",
            String::from_utf8_lossy(client.received_trace())
        ));
    }
    r.stdout
        .split_whitespace()
        .find_map(|s| s.strip_prefix("completed="))
        .ok_or("progress counter missing")?
        .parse()
        .map_err(|_| "invalid progress counter".into())
}

fn guest_platform_name(component: &str) -> String {
    let architecture = std::env::var("BEXOS_QEMU_ARCH").unwrap_or_else(|_| "aarch64".into());
    format!("qemu-{architecture}-{component}")
}
