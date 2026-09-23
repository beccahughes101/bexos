//! Actual product uploads, resident execution, retained provider connection,
//! protected commitment and reboot selection against persistent device state.
use super::*;
use bexos_qemu_test::Architecture;
use std::io::{Seek, SeekFrom, Write};
const ORCHESTRATOR: [u8; 16] = [
    0x2b, 0x45, 0x58, 0x4f, 0x53, 6, 0x40, 2, 0x80, 0, 0, 0, 0, 0, 0, 6,
];

fn error(e: impl std::fmt::Debug) -> String {
    format!("firmware product: {e:?}")
}
fn print_serial_tail<T: DebugTransport>(session: &mut DebugSession<T>) {
    if let Ok(output) = session.wait_for_serial_markers(&[], std::time::Duration::from_millis(10)) {
        let start = output.len().saturating_sub(16 * 1024);
        eprintln!(
            "e2e: guest diagnostics tail:\n{}",
            String::from_utf8_lossy(&output[start..])
        );
    }
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
    aarch64: bool,
) -> Result<(), String> {
    eprintln!(
        "e2e: firmware upload generation={generation} trusty={trusty} bytes={}",
        image.len()
    );
    let manifest = build_signed_manifest(
        generation,
        if trusty {
            if aarch64 {
                "qemu-aarch64-tee"
            } else {
                "qemu-x86_64-tee"
            }
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
    aarch64: bool,
    mode: &str,
) -> Result<bexos_debug_wire::ExecResponse, String> {
    stage(session, image, generation, trusty, aarch64)?;
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
    trusty: bool,
    aarch64: bool,
    retained: u64,
    expected: &[u8],
) -> Result<bexos_debug_wire::ExecResponse, String> {
    use bexos_debug_wire::*;
    stage(session, image, generation, trusty, aarch64)?;
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
    aarch64: bool,
) -> Result<DebugSession<<QemuDevice as E2eDevice>::DebugTransport>, String> {
    let mut markers = boot_markers();
    markers.extend_from_slice(BEXFS_MARKERS);
    markers.extend_from_slice(&[
        b"teed: service ready",
        b"debugd: tee proxy connected",
        b"appd: app lifecycle registry ready for debugd",
        b"kernel: cpu3 scheduler idle ready",
    ]);
    if !aarch64 {
        markers.push(
            b"monitor-runtime: authenticated firmware selection complete; recovery instance discarded",
        );
    }
    let mut session = device.boot_with_debugd(&markers)?;
    session.assert_debugd_ready()?;
    Ok(session)
}
pub fn run(artifacts: QemuArtifacts, paths: &[String], reboot: bool) -> Result<(), String> {
    let aarch64 = artifacts.architecture == Architecture::Aarch64;
    if paths.len() != if aarch64 { 5 } else { 10 } {
        return Err(
            "expected architecture-specific Trusty generation/fault/incompatibility bundles".into(),
        );
    }
    let bundles = paths
        .iter()
        .map(std::fs::read)
        .collect::<Result<Vec<_>, _>>()
        .map_err(error)?;
    let mut device = QemuDevice::new(artifacts)?;
    let mut session = boot(&mut device, aarch64)?;
    verify_tee_proxy(&mut session)?;
    let mut retained = session
        .client
        .tee_open_session(ORCHESTRATOR.to_vec())
        .map_err(error)?;
    let mut response = invoke(&mut session, retained)?;
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
    if aarch64 {
        let mut tampered = bundles[0].clone();
        *tampered.last_mut().unwrap() ^= 1;
        let rejected = upload(&mut session, &tampered, 2, true, true, "live")?;
        if rejected.exit_code == 0 || invoke(&mut session, retained)? != response {
            return Err("tampered ARM Trusty candidate changed the retained session".into());
        }
        if reboot {
            for (bundle, generation) in [(&bundles[0], 2), (&bundles[1], 3)] {
                let result = upload(&mut session, bundle, generation, true, true, "on-reboot")?;
                if result.exit_code != 0 {
                    return Err(format!(
                        "stage ARM Trusty reboot generation {generation}: {result:?}"
                    ));
                }
                progress(&mut session, "RebootPending", generation)?;
                drop(session);
                session = boot(&mut device, aarch64)?;
                progress(&mut session, "Completed", generation)?;
                verify_tee_proxy(&mut session)?;
            }
        } else {
            if std::env::var_os("BEXOS_TRUSTY_VALID_ONLY").is_none() {
                for (bundle, label) in [
                    (&bundles[2], "incompatible migration"),
                    (&bundles[3], "fault"),
                    (&bundles[4], "hang"),
                ] {
                    let result = live_with_pending(
                        &mut session,
                        bundle,
                        2,
                        true,
                        true,
                        retained,
                        &response,
                    )?;
                    if result.exit_code == 0 {
                        return Err(format!("live ARM Trusty {label} candidate committed"));
                    }
                    progress(&mut session, "RolledBack", 2)?;
                    if invoke(&mut session, retained)? != response {
                        return Err(format!(
                            "ARM Trusty {label} rejection changed retained session"
                        ));
                    }
                }
            }
            for (bundle, generation) in [(&bundles[0], 2), (&bundles[1], 3)] {
                let result = live_with_pending(
                    &mut session,
                    bundle,
                    generation,
                    true,
                    true,
                    retained,
                    &response,
                )?;
                if result.exit_code != 0 {
                    print_serial_tail(&mut session);
                    return Err(format!(
                        "live ARM Trusty generation {generation}: {result:?}"
                    ));
                }
                progress(&mut session, "Completed", generation)?;
                if invoke(&mut session, retained)? != response {
                    return Err(format!(
                        "retained ARM session failed after generation {generation}"
                    ));
                }
                verify_tee_proxy(&mut session)?;
                let after = session.client.list_processes().map_err(error)?;
                for (package, pid, thread) in &identities {
                    if !after.iter().any(|p| {
                        &p.package_id == package && p.pid == *pid && p.main_thread_id == *thread
                    }) {
                        return Err(format!("ARM guest process identity changed: {package}"));
                    }
                }
            }
        }
        session.client.tee_close_session(retained).map_err(error)?;
        eprintln!(
            "e2e: ARM Trusty activation retained normal-world processes and public session identity"
        );
        return Ok(());
    }
    let mut tampered = bundles[2].clone();
    *tampered.last_mut().unwrap() ^= 1;
    for (bytes, generation, trusty, mode) in [
        (&tampered, 2, false, "live"),
        (&bundles[2], 3, false, "live"),
        (&bundles[2], 2, true, "on-reboot"),
    ] {
        let result = upload(&mut session, bytes, generation, trusty, false, mode)?;
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
        let result = upload(&mut session, &bundles[0], 2, false, false, "on-reboot")?;
        if result.exit_code != 0 {
            return Err(format!("stage cold fault: {result:?}"));
        }
        progress(&mut session, "RebootPending", 2)?;
        drop(session);
        session = boot(&mut device, aarch64)?;
        progress(&mut session, "RolledBack", 2)?;
        let result = upload(&mut session, &bundles[2], 2, false, false, "on-reboot")?;
        if result.exit_code != 0 {
            return Err(format!("stage cold monitor: {result:?}"));
        }
        progress(&mut session, "RebootPending", 2)?;
        drop(session);
        session = boot(&mut device, aarch64)?;
        progress(&mut session, "Completed", 2)?;
        let result = upload(&mut session, &bundles[5], 3, false, false, "on-reboot")?;
        if result.exit_code != 0 {
            return Err(format!("stage later monitor fault: {result:?}"));
        }
        progress(&mut session, "RebootPending", 3)?;
        drop(session);
        session = boot(&mut device, aarch64)?;
        progress(&mut session, "RolledBack", 3)?;
        session.wait_for_serial_markers(
            &[b"monitor-runtime: selected monitor generation=\n0000000000000002"],
            std::time::Duration::from_secs(5),
        )?;
        verify_tee_proxy(&mut session)?;
        let result = upload(&mut session, &bundles[3], 3, false, false, "on-reboot")?;
        if result.exit_code != 0 {
            return Err(format!("stage later monitor retry: {result:?}"));
        }
        progress(&mut session, "RebootPending", 3)?;
        drop(session);
        session = boot(&mut device, aarch64)?;
        progress(&mut session, "Completed", 3)?;
    } else {
        for (index, bytes) in bundles[..4].iter().enumerate() {
            let generation = if index == 3 { 3 } else { 2 };
            let result = live_with_pending(
                &mut session,
                bytes,
                generation,
                false,
                false,
                retained,
                &response,
            )?;
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
        session = boot(&mut device, aarch64)?;
        progress(&mut session, "Completed", 3)?;
    }
    retained = session
        .client
        .tee_open_session(ORCHESTRATOR.to_vec())
        .map_err(error)?;
    response = invoke(&mut session, retained)?;
    if reboot {
        for (bundle, generation) in [(&bundles[4], 2), (&bundles[6], 3)] {
            let result = upload(&mut session, bundle, generation, true, false, "on-reboot")?;
            if result.exit_code != 0 {
                return Err(format!(
                    "stage Trusty reboot generation {generation}: {result:?}"
                ));
            }
            progress(&mut session, "RebootPending", generation)?;
            drop(session);
            session = boot(&mut device, aarch64)?;
            progress(&mut session, "Completed", generation)?;
            let info = session.client.tee_info().map_err(error)?;
            if info.secure_os_version != generation as u32 {
                return Err(format!(
                    "selected Trusty code generation {generation} not observed: {info:?}"
                ));
            }
            verify_tee_proxy(&mut session)?;
        }
    } else {
        for (bundle, label) in [
            (&bundles[7], "incompatible migration"),
            (&bundles[8], "fault"),
            (&bundles[9], "hang"),
        ] {
            let result =
                live_with_pending(&mut session, bundle, 2, true, false, retained, &response)?;
            if result.exit_code == 0 {
                return Err(format!("live Trusty {label} candidate committed"));
            }
            progress(&mut session, "RolledBack", 2)?;
            if invoke(&mut session, retained)? != response {
                return Err(format!(
                    "live Trusty {label} rollback changed retained session"
                ));
            }
        }
        for (bundle, generation) in [(&bundles[4], 2), (&bundles[6], 3)] {
            let result = live_with_pending(
                &mut session,
                bundle,
                generation,
                true,
                false,
                retained,
                &response,
            )?;
            if result.exit_code != 0 {
                return Err(format!("live Trusty generation {generation}: {result:?}"));
            }
            progress(&mut session, "Completed", generation)?;
            if invoke(&mut session, retained)? != response {
                return Err(format!(
                    "retained session failed after Trusty generation {generation}"
                ));
            }
            verify_tee_proxy(&mut session)?;
            let info = session.client.tee_info().map_err(error)?;
            if info.secure_os_version != generation as u32 {
                return Err(format!(
                    "live Trusty generation {generation} not observed: {info:?}"
                ));
            }
        }
        session.wait_for_serial_markers(
            &[
                b"monitor-runtime: distinct Trusty candidate executed with compatible migration and required services",
                b"monitor-runtime: live Trusty committed; retired private bank reclaimed",
                b"tee-driver-trusty: public sessions rebound to committed transport generation",
            ],
            std::time::Duration::from_secs(5),
        )?;
    }
    session.client.tee_close_session(retained).map_err(error)?;
    drop(session);
    // Ordinary disk contents cannot select a fallback to the embedded image.
    let mut disk = std::fs::OpenOptions::new()
        .write(true)
        .open(device.firmware_disk_path())
        .map_err(error)?;
    disk.seek(SeekFrom::Start(
        bexos_secure_firmware::store::slot_sector(
            bexos_secure_firmware::Component::Trusty,
            bexos_secure_firmware::selection::Slot::A,
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
