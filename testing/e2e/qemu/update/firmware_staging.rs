//! Copied, authenticated product staging, pending reboot and serialization.
use super::*;

pub fn run<T: DebugTransport>(session: &mut DebugSession<T>, path: &str) -> Result<(), String> {
    verify_tee_proxy(session)?;
    let firmware = std::fs::read(path).map_err(|e| format!("read signed Trusty candidate: {e}"))?;
    if firmware.len() <= 65536 {
        return Err("candidate must exceed one shared buffer".into());
    }
    for (index, tamper) in [true, false, false].into_iter().enumerate() {
        let mut artifact = firmware.clone();
        if tamper {
            *artifact.last_mut().unwrap() ^= 1;
        }
        let manifest = build_signed_manifest(
            2,
            "qemu-x86_64-tee",
            ArtifactKind::TeeImage,
            &artifact,
            KEY_ID,
            SEED,
        );
        eprintln!("e2e: uploading firmware candidate {index}, tampered={tamper}");
        session
            .client
            .upload_update(0x46570000 + index as u64, &manifest, &artifact)
            .map_err(|e| format!("candidate upload: {e:?}"))?;
        // Upload traffic can wrap the bounded diagnostic ring. Begin this
        // candidate's evidence window after the upload, without retaining a
        // byte offset into a ring whose earlier bytes can be discarded.
        session.client.clear_received_trace();
        let result = session
            .client
            .exec_command("update.apply_platform", &[])
            .map_err(|e| format!("candidate apply: {e:?}"))?;
        if (result.exit_code == 0) != (index == 1) {
            return Err(format!(
                "candidate {index}: expected only the first authentic upload to become pending, received {result:?}"
            ));
        }
        // Q35 console diagnostics arrive independently of the RPC socket.
        // The apply response can arrive before its console reader has copied
        // the authentication marker; collect that channel before asserting.
        let marker: &[u8] = if tamper {
            b"monitor-runtime: candidate authentication rejected"
        } else {
            b"tee-driver-trusty: signed firmware snapshot authenticated"
        };
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while index < 2 {
            session
                .client
                .drain_for(std::time::Duration::from_millis(100))
                .map_err(|e| format!("candidate diagnostics: {e:?}"))?;
            if bexos_e2e::contains(session.client.received_trace(), marker) {
                break;
            }
            if std::time::Instant::now() >= deadline {
                return Err(format!(
                    "candidate {index}: missing explicit evidence {}",
                    String::from_utf8_lossy(marker)
                ));
            }
        }
        let trace = session.client.received_trace();
        let sealed = bexos_e2e::contains(
            trace,
            b"tee-driver-trusty: signed firmware snapshot authenticated",
        );
        if index < 2 && sealed == tamper {
            return Err(format!(
                "candidate {index}: authentication evidence mismatch, tampered={tamper}, sealed={sealed}"
            ));
        }
        let progress = session
            .client
            .tee_update_status()
            .map_err(|e| format!("candidate status: {e:?}"))?;
        if progress.update_status == "Completed" {
            return Err("staging was reported as committed replacement".into());
        }
        if index > 0
            && (progress.phase != "RebootPending"
                || progress.generation != 2
                || !progress.reboot_required)
        {
            return Err(format!(
                "pending publication or serialization lost: {progress:?}"
            ));
        }
        verify_tee_proxy(session)?;
    }
    eprintln!(
        "e2e: signed multi-buffer Trusty staging, tamper rejection, serialized pending publication and continued services verified"
    );
    Ok(())
}
