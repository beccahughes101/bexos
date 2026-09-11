//! Actual product uploads, resident execution, retained provider connection,
//! protected commitment and reboot selection against persistent device state.
use super::*;
use std::io::{Seek, SeekFrom, Write};
const ORCHESTRATOR: [u8; 16] = [
    0x2b, 0x45, 0x58, 0x4f, 0x53, 6, 0x40, 2, 0x80, 0, 0, 0, 0, 0, 0, 6,
];

fn error(e: impl std::fmt::Debug) -> String {
    format!("firmware product: {e:?}")
}
fn invoke<T: DebugTransport>(
    session: &mut DebugSession<T>,
    retained: u64,
) -> Result<Vec<u8>, String> {
    session
        .client
        .tee_invoke(retained, 0x202, request())
        .map_err(error)
}
fn request() -> Vec<u8> {
    let mut request = vec![0; 16];
    request[..4].copy_from_slice(&1u32.to_le_bytes());
    request[4..8].copy_from_slice(&0x202u32.to_le_bytes());
    request[8..12].copy_from_slice(&1u32.to_le_bytes());
    request
}
fn stage<T: DebugTransport>(
    session: &mut DebugSession<T>,
    image: &[u8],
    generation: u64,
    trusty: bool,
) -> Result<(), String> {
    eprintln!(
        "e2e: firmware upload generation={generation} trusty={trusty} bytes={}",
        image.len()
    );
    let manifest = build_signed_manifest(
        generation,
        if trusty {
            "qemu-x86_64-tee"
        } else {
            "qemu-x86_64-monitor"
        },
        if trusty {
            ArtifactKind::TeeImage
        } else {
            ArtifactKind::Hypervisor
        },
        image,
        KEY_ID,
        SEED,
    );
    session
        .client
        .upload_update(0x46574100 + generation, &manifest, image)
        .map_err(error)?;
    Ok(())
}
fn upload<T: DebugTransport>(
    session: &mut DebugSession<T>,
    image: &[u8],
    generation: u64,
    trusty: bool,
    mode: &str,
) -> Result<bexos_debug_wire::ExecResponse, String> {
    stage(session, image, generation, trusty)?;
    eprintln!("e2e: firmware upload complete; activate {mode}");
    session
        .client
        .exec_command("update.apply_firmware", &[mode.into()])
        .map_err(error)
}
fn live_with_pending<T: DebugTransport>(
    session: &mut DebugSession<T>,
    image: &[u8],
    generation: u64,
    retained: u64,
    expected: &[u8],
) -> Result<bexos_debug_wire::ExecResponse, String> {
    use bexos_debug_wire::*;
    stage(session, image, generation, false)?;
    let mut activation = Vec::new();
    encode_exec_request(
        &ExecRequest {
            component_id: "update.apply_firmware".into(),
            args: vec!["live".into()],
        },
        &mut activation,
    );
    let mut invocation = Vec::new();
    encode_tee_invoke_request(
        &TeeInvokeRequest {
            session_id: retained,
            command_id: 0x202,
            payload: request(),
        },
        &mut invocation,
    );
    // Both frames enter the retained transport before activation completes.
    // The queued client request must finish after either commitment or rollback.
    let replies = session
        .client
        .call_batch(&[
            (METHOD_EXEC_COMMAND, activation),
            (METHOD_TEE_INVOKE, invocation),
        ])
        .map_err(error)?;
    let invoked = decode_tee_invoke_response(&replies[1].payload).map_err(error)?;
    if invoked.status != 0 || invoked.response != expected {
        return Err(format!(
            "in-flight secure request changed across cutover: {invoked:?}"
        ));
    }
    eprintln!("e2e: retained in-flight client request completed across cutover");
    decode_exec_response(&replies[0].payload).map_err(error)
}
fn progress<T: DebugTransport>(
    session: &mut DebugSession<T>,
    phase: &str,
    generation: u64,
) -> Result<(), String> {
    let status = session.client.tee_update_status().map_err(error)?;
    eprintln!("e2e: firmware status {status:?}");
    if status.phase != phase || status.generation != generation {
        return Err(format!(
            "expected {phase} generation {generation}: {status:?}"
        ));
    }
    if phase == "RebootPending" && (status.update_status == "Completed" || !status.reboot_required)
    {
        return Err(format!("pending firmware misreported: {status:?}"));
    }
    Ok(())
}
fn boot(
    device: &mut QemuDevice,
) -> Result<DebugSession<<QemuDevice as E2eDevice>::DebugTransport>, String> {
    let mut markers = boot_markers();
    markers.extend_from_slice(BEXFS_MARKERS);
    markers.extend_from_slice(&[
        b"teed: service ready",
        b"debugd: tee proxy connected",
        b"appd: app lifecycle registry ready for debugd",
        b"monitor-runtime: authenticated firmware selection complete; recovery instance discarded",
        b"kernel: cpu3 scheduler idle ready",
    ]);
    let mut session = device.boot_with_debugd(&markers)?;
    session.assert_debugd_ready()?;
    Ok(session)
}
pub fn run(artifacts: QemuArtifacts, paths: &[String], reboot: bool) -> Result<(), String> {
    if paths.len() != 6 {
        return Err(
            "expected fault, hang, candidate, successor, Trusty and successor-fault bundles".into(),
        );
    }
    let bundles = paths
        .iter()
        .map(std::fs::read)
        .collect::<Result<Vec<_>, _>>()
        .map_err(error)?;
    let mut device = QemuDevice::new(artifacts)?;
    let mut session = boot(&mut device)?;
    verify_tee_proxy(&mut session)?;
    let retained = session
        .client
        .tee_open_session(ORCHESTRATOR.to_vec())
        .map_err(error)?;
    let response = invoke(&mut session, retained)?;
    let identities = session
        .client
        .list_processes()
        .map_err(error)?
        .into_iter()
        .filter(|p| {
            matches!(
                p.package_id.as_str(),
                "bexos.service.teed" | "bexos.service.updated" | "bexos.driver.debugd"
            )
        })
        .map(|p| (p.package_id, p.pid, p.main_thread_id))
        .collect::<Vec<_>>();
    if identities.len() != 3 {
        return Err(format!(
            "missing retained product processes: {identities:?}"
        ));
    }
    let mut tampered = bundles[2].clone();
    *tampered.last_mut().unwrap() ^= 1;
    for (bytes, generation, trusty, mode) in [
        (&tampered, 2, false, "live"),
        (&bundles[2], 3, false, "live"),
        (&bundles[2], 2, true, "on-reboot"),
    ] {
        let result = upload(&mut session, bytes, generation, trusty, mode)?;
        if result.exit_code == 0 {
            return Err(format!(
                "invalid candidate accepted: {generation} trusty={trusty}"
            ));
        }
        if invoke(&mut session, retained)? != response {
            return Err("rejection changed retained secure session".into());
        }
    }
    if reboot {
        let result = upload(&mut session, &bundles[0], 2, false, "on-reboot")?;
        if result.exit_code != 0 {
            return Err(format!("stage cold fault: {result:?}"));
        }
        progress(&mut session, "RebootPending", 2)?;
        drop(session);
        session = boot(&mut device)?;
        progress(&mut session, "RolledBack", 2)?;
        let result = upload(&mut session, &bundles[2], 2, false, "on-reboot")?;
        if result.exit_code != 0 {
            return Err(format!("stage cold monitor: {result:?}"));
        }
        progress(&mut session, "RebootPending", 2)?;
        drop(session);
        session = boot(&mut device)?;
        progress(&mut session, "Completed", 2)?;
        let result = upload(&mut session, &bundles[5], 3, false, "on-reboot")?;
        if result.exit_code != 0 {
            return Err(format!("stage later monitor fault: {result:?}"));
        }
        progress(&mut session, "RebootPending", 3)?;
        drop(session);
        session = boot(&mut device)?;
        progress(&mut session, "RolledBack", 3)?;
        session.wait_for_serial_markers(
            &[b"monitor-runtime: selected monitor generation=\n0000000000000002"],
            std::time::Duration::from_secs(5),
        )?;
        verify_tee_proxy(&mut session)?;
        let result = upload(&mut session, &bundles[3], 3, false, "on-reboot")?;
        if result.exit_code != 0 {
            return Err(format!("stage later monitor retry: {result:?}"));
        }
        progress(&mut session, "RebootPending", 3)?;
        drop(session);
        session = boot(&mut device)?;
        progress(&mut session, "Completed", 3)?;
    } else {
        for (index, bytes) in bundles[..4].iter().enumerate() {
            let generation = if index == 3 { 3 } else { 2 };
            let result = live_with_pending(&mut session, bytes, generation, retained, &response)?;
            if (result.exit_code == 0) != (index >= 2) {
                return Err(format!("live candidate {index}: {result:?}"));
            }
            progress(
                &mut session,
                if index < 2 { "RolledBack" } else { "Completed" },
                generation,
            )?;
            if invoke(&mut session, retained)? != response {
                return Err("live trial changed retained secure session".into());
            }
            let after = session.client.list_processes().map_err(error)?;
            for (package, pid, thread) in &identities {
                if !after.iter().any(|p| {
                    &p.package_id == package && p.pid == *pid && p.main_thread_id == *thread
                }) {
                    return Err(format!("guest process identity changed: {package}"));
                }
            }
            // The orchestrator intentionally admits one client. Its retained
            // session was just exercised above; opening a second smoke client
            // would test that limit instead of replacement continuity.
            let keymint = session
                .client
                .exec_command("tee.keymint_smoke", &[])
                .map_err(error)?;
            if keymint.exit_code != 0 {
                return Err(format!("KeyMint progress after replacement: {keymint:?}"));
            }
        }
        session.client.tee_close_session(retained).map_err(error)?;
        session.wait_for_serial_markers(
            &[b"monitor-runtime: product old monitor code data and stack reclaimed"],
            std::time::Duration::from_secs(5),
        )?;
        drop(session);
        session = boot(&mut device)?;
        progress(&mut session, "Completed", 3)?;
    }
    let result = upload(&mut session, &bundles[4], 2, true, "live")?;
    if result.exit_code == 0 {
        return Err("live Trusty activation was accepted".into());
    }
    verify_tee_proxy(&mut session)?;
    // Reuse the retained authenticated upload after the unsupported mode was
    // rejected; a mode error must not make clients upload the image again.
    let result = session
        .client
        .exec_command("update.apply_firmware", &["on-reboot".into()])
        .map_err(error)?;
    if result.exit_code != 0 {
        return Err(format!("stage Trusty reboot: {result:?}"));
    }
    progress(&mut session, "RebootPending", 2)?;
    verify_tee_proxy(&mut session)?;
    drop(session);
    session = boot(&mut device)?;
    progress(&mut session, "Completed", 2)?;
    let info = session.client.tee_info().map_err(error)?;
    if info.secure_os_version != 2 {
        return Err(format!(
            "selected Trusty code generation not observed: {info:?}"
        ));
    }
    verify_tee_proxy(&mut session)?;
    drop(session);
    // Ordinary disk contents cannot select a fallback to the embedded image.
    let mut disk = std::fs::OpenOptions::new()
        .write(true)
        .open(device.firmware_disk_path())
        .map_err(error)?;
    disk.seek(SeekFrom::Start(
        bexos_secure_firmware::store::slot_sector(
            bexos_secure_firmware::Component::Trusty,
            bexos_secure_firmware::selection::Slot::B,
        ) * 512,
    ))
    .map_err(error)?;
    disk.write_all(&[0; 512]).map_err(error)?;
    disk.sync_all().map_err(error)?;
    drop(disk);
    device.assert_boot_rejected(
        &[b"monitor-runtime: firmware recovery required; no older image admitted"],
        &[
            b"userspace: entering ring3 appd",
            b"kernel: boot kernel_main",
        ],
    )?;
    eprintln!(
        "e2e: product firmware activation, retained clients, reboot persistence and committed-image fail-closed verified"
    );
    Ok(())
}
