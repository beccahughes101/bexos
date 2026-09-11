use bexos_debug_wire::ShellRequest;
use bexos_e2e::{BEXFS_MARKERS, E2eDevice, NVME_MARKERS, boot_markers};
use bexos_qemu_test::{QemuArtifacts, QemuDevice};
use std::time::{Duration, Instant};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (artifacts, _) = QemuArtifacts::from_env_or_args(&[])?;
    let mut markers = boot_markers();
    markers.extend_from_slice(NVME_MARKERS);
    markers.extend_from_slice(BEXFS_MARKERS);
    markers.push(b"appd: lifecycle dispatch ready");
    let mut device = QemuDevice::new(artifacts)?;
    let mut session = device.boot_with_debugd(&markers)?;
    #[cfg(target_os = "macos")]
    assert!(
        String::from_utf8_lossy(&session.boot_output)
            .contains("qemu: macOS background activity enabled"),
        "QEMU must retain its own activity while the host window is covered"
    );
    session.assert_debugd_ready()?;
    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        let c = &mut session.client;
        let opening = Instant::now();
        let opened = c.open_shell(&ShellRequest {
            system: true,
            rows: 24,
            cols: 80,
            ..Default::default()
        })?;
        eprintln!("Brush system shell opened in {:?}", opening.elapsed());
        assert_eq!(opened.uid, 0);
        assert_eq!(opened.provider, "bexos.app.brush_shell");
        assert!(
            String::from_utf8_lossy(c.received_trace())
                .contains("wasm_runtime: loading trusted precompiled component"),
            "the packaged component must select the Bazel-built artifact"
        );
        // An interactive frontend sends empty polls before the first command.
        // The prompt and status must remain responsive while Brush is idle.
        let mut prompt = Vec::new();
        for _ in 0..3 {
            let idle = c.exchange_shell(opened.session_id, &[], false)?;
            assert!(!idle.exited);
            prompt.extend(idle.stdout);
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(String::from_utf8_lossy(&prompt).contains("brush$ "));
        let script = b"value=41; echo BRUSH_RESULT_$((value+1)); echo BRUSH_ERROR_$((value+2)) >&2; exit 7\n";
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut sent = 0;
        let mut stdout = prompt;
        let mut stderr = Vec::new();
        loop {
            if Instant::now() > deadline {
                return Err(format!(
                    "Brush timeout: stdout={} stderr={}",
                    String::from_utf8_lossy(&stdout),
                    String::from_utf8_lossy(&stderr)
                )
                .into());
            }
            let r = c.exchange_shell(opened.session_id, &script[sent..], false)?;
            sent += r.consumed as usize;
            stdout.extend(r.stdout);
            stderr.extend(r.stderr);
            if r.exited {
                assert_eq!(sent, script.len());
                assert_eq!(
                    r.exit_code,
                    7,
                    "stderr={}",
                    String::from_utf8_lossy(&stderr)
                );
                assert!(String::from_utf8_lossy(&stdout).contains("BRUSH_RESULT_42"));
                assert!(String::from_utf8_lossy(&stdout).contains("brush$ "));
                assert!(String::from_utf8_lossy(&stderr).contains("BRUSH_ERROR_43"));
                break;
            }
        }
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
