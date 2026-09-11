use bexos_debug_client::DebugClientError;
use bexos_e2e::{E2eDevice, boot_markers};
use bexos_qemu_test::{QemuArtifacts, QemuDevice};

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let artifacts = QemuArtifacts::from_args(&args)?;
    let base_artifacts = artifacts.clone();
    let x86 = artifacts.x86_secure_firmware.is_some();
    let mut device = QemuDevice::new(artifacts)?;
    let mut secure_markers = boot_markers();
    secure_markers.push(b"kernel: secure boot evidence verified");
    let approval_markers: &[&[u8]] = if x86 {
        &[
            b"monitor-runtime: authenticated payload and Trusty rollback approval verified",
            b"kernel: RPMB anti-rollback backend verified",
        ]
    } else {
        &[
            b"bl33: authenticated payload and Trusty rollback approval verified",
            b"kernel: RPMB anti-rollback backend verified",
        ]
    };
    secure_markers.extend_from_slice(approval_markers);

    {
        let mut session = device.boot_with_debugd(&secure_markers)?;
        session.assert_debugd_ready()?;
        let apps = session
            .client
            .tee_apps()
            .map_err(|error| format!("list Trusty apps: {error:?}"))?;
        for package in [
            "bexos.ta.keymint",
            "bexos.ta.gatekeeper",
            "bexos.ta.avb",
            "bexos.ta.authmgr",
            "bexos.ta.storage",
            "bexos.orchestrator",
        ] {
            if !apps
                .iter()
                .any(|app| app.package_id == package && app.protected)
            {
                return Err(format!("missing protected Trusty application {package}"));
            }
        }
        let smoke = session
            .client
            .exec_command("tee.keymint_smoke", &[])
            .map_err(|error| format!("execute KeyMint smoke: {error:?}"))?;
        if smoke.exit_code != 0 {
            return Err(format!("KeyMint smoke failed: {}", smoke.stderr));
        }
        eprintln!("e2e: KeyMint algorithms and secure deletion verified");
        let orchestrator = session
            .client
            .exec_command("tee.orchestrator_smoke", &[])
            .map_err(|error| format!("execute orchestrator smoke: {error:?}"))?;
        if orchestrator.exit_code != 0 {
            return Err(format!(
                "orchestrator smoke failed: {}",
                orchestrator.stderr
            ));
        }
        let authmgr = session
            .client
            .exec_command("tee.authmgr_smoke", &[])
            .map_err(|error| format!("execute AuthMgr smoke: {error:?}"))?;
        if authmgr.exit_code != 0 {
            return Err(format!("AuthMgr BE smoke failed: {}", authmgr.stderr));
        }
        let queued = session
            .client
            .exec_command("tee.concurrent_smoke", &[])
            .map_err(|error| format!("queued secure requests: {error:?}"))?;
        if queued.exit_code != 0 {
            return Err(format!("queued secure requests failed: {}", queued.stderr));
        }
        eprintln!("e2e: orchestrator, AuthMgr availability, and queued secure requests verified");
        session
            .client
            .create_user(
                2001,
                "trusty-e2e",
                "Trusty E2E",
                "correct horse battery staple",
            )
            .map_err(|error| {
                format!(
                    "Gatekeeper enrollment: {error:?}\n{}",
                    String::from_utf8_lossy(session.client.received_trace())
                )
            })?;
        assert_access_denied(
            session
                .client
                .unlock_user(2001, "incorrect horse battery staple"),
            "incorrect Gatekeeper password",
        )?;
        session
            .client
            .unlock_user(2001, "correct horse battery staple")
            .map_err(|error| format!("initial Gatekeeper unlock: {error:?}"))?;
        session
            .client
            .replace_user_password(
                2001,
                "trusty-e2e",
                "Trusty E2E",
                false,
                "correct horse battery staple",
                "new correct horse battery staple",
            )
            .map_err(|error| format!("Gatekeeper password replacement: {error:?}"))?;
        assert_access_denied(
            session
                .client
                .unlock_user(2001, "correct horse battery staple"),
            "replaced Gatekeeper password",
        )?;
        session
            .client
            .unlock_user(2001, "new correct horse battery staple")
            .map_err(|error| format!("replacement Gatekeeper unlock: {error:?}"))?;
        session
            .client
            .lock_user(2001)
            .map_err(|error| format!("lock enrolled user: {error:?}"))?;
    }

    // QEMU and rpmb_dev are both restarted. The device object deliberately
    // retains the same writable disk and authenticated RPMB image.
    {
        let mut session = device.boot_with_debugd(&secure_markers)?;
        session.assert_debugd_ready()?;
        let orchestrator = session
            .client
            .exec_command("tee.orchestrator_smoke", &[])
            .map_err(|error| format!("post-reboot orchestrator smoke: {error:?}"))?;
        if orchestrator.exit_code != 0 {
            return Err(format!(
                "post-reboot orchestrator smoke failed: {}",
                orchestrator.stderr
            ));
        }
        session
            .client
            .unlock_user(2001, "new correct horse battery staple")
            .map_err(|error| format!("post-reboot Gatekeeper/KeyMint unlock: {error:?}"))?;
        session
            .client
            .delete_user(2001)
            .map_err(|error| format!("secure user deletion: {error:?}"))?;
    }
    unsafe { std::env::set_var("BEXOS_QEMU_TIMEOUT_SECONDS", "300") };
    assert_rejected_artifact(&base_artifacts, "kernel", |artifacts, path| {
        artifacts.kernel = path
    })?;
    assert_rejected_artifact(&base_artifacts, "BootFS", |artifacts, path| {
        artifacts.bootfs = path
    })?;
    assert_rejected_artifact(&base_artifacts, "policy", |artifacts, path| {
        artifacts.bootfs = path
    })?;
    assert_rejected_artifact(&base_artifacts, "vbmeta", |artifacts, path| {
        artifacts.vbmeta = path
    })?;
    assert_rejected_artifact(
        &base_artifacts,
        "unauthenticated verifier",
        |artifacts, path| {
            if let Some(firmware) = artifacts.x86_secure_firmware.as_mut() {
                firmware.loader = path;
            } else {
                artifacts.secure_firmware.as_mut().unwrap().bl33 = path;
            }
        },
    )?;

    let stale_artifacts = base_artifacts.clone();
    let mut fixture = base_artifacts.clone();
    let provisioner = std::env::var("BEXOS_QEMU_ROLLBACK_PROVISIONER")
        .map_err(|_| "missing authenticated rollback provisioner")?
        .into();
    if x86 {
        fixture.x86_secure_firmware.as_mut().unwrap().loader = provisioner;
    } else {
        let firmware = fixture.secure_firmware.as_mut().unwrap();
        firmware.bl33 = provisioner;
        firmware.bl33_certificate = Some(
            std::env::var("BEXOS_QEMU_ROLLBACK_CERTIFICATE")
                .map_err(|_| "missing authenticated rollback certificate")?
                .into(),
        );
    }
    let mut provisioned = QemuDevice::new(fixture)?;
    std::fs::copy(device.rpmb_path(), provisioned.rpmb_path())
        .map_err(|e| format!("seed rollback provisioner: {e}"))?;
    provisioned.assert_boot_rejected(
        &[if x86 {
            b"monitor-fixture: authenticated RPMB floor provisioned"
        } else {
            b"bl33-fixture: authenticated RPMB floor provisioned"
        }],
        &[b"kernel: boot kernel_main", b"pivot complete;"],
    )?;
    let mut stale_device = QemuDevice::new_without_host_preflight(stale_artifacts)?;
    std::fs::copy(provisioned.rpmb_path(), stale_device.rpmb_path())
        .map_err(|e| format!("seed stale rollback RPMB: {e}"))?;
    stale_device.assert_boot_rejected(
        &[if x86 {
            b"monitor-runtime: stale generation rejected by Trusty RPMB floor"
        } else {
            b"bl33: FATAL: stale generation rejected by Trusty RPMB floor"
        }],
        &[
            b"kernel: boot kernel_main",
            b"appd: RPMB anti-rollback backend verified",
            b"kernel: RPMB anti-rollback backend verified",
            b"pivot complete;",
        ],
    )?;

    let rpmb = device.rpmb_path().to_path_buf();
    let mut state = std::fs::read(&rpmb).map_err(|error| format!("read RPMB image: {error}"))?;
    // Preserve the helper's valid container, size, write counter, and
    // programmed flag. Corrupt its authentication key so the guest receives
    // frames with invalid MACs, rather than a helper startup/format error.
    if state.len() < 512 || state[7] != 1 {
        return Err("expected a provisioned RPMB image before corruption".into());
    }
    state[8] ^= 0x80;
    std::fs::write(&rpmb, &state)
        .map_err(|error| format!("corrupt RPMB authentication state: {error}"))?;
    let corruption_markers: &[&[u8]] = if x86 {
        &[
            b": Bad MAC",
            b"block_device_tipc_init_rpmb_key failed",
            b"monitor-runtime: Trusty rollback state unavailable; refusing execution",
        ]
    } else {
        &[
            b": Bad MAC",
            b"block_device_tipc_init_rpmb_key failed",
            b"): Unclean exit from critical app",
        ]
    };
    let forbidden: &[&[u8]] = &[
        b"appd: RPMB anti-rollback backend verified",
        b"kernel: RPMB anti-rollback backend verified",
        b"pivot complete;",
    ];
    device.assert_rpmb_authentication_rejected(corruption_markers, forbidden)?;
    Ok(())
}

fn assert_rejected_artifact(
    base: &QemuArtifacts,
    name: &str,
    replace: impl FnOnce(&mut QemuArtifacts, std::path::PathBuf),
) -> Result<(), String> {
    let root = std::env::var("TEST_TMPDIR").unwrap_or_else(|_| "/tmp".into());
    let source = match name {
        "kernel" => &base.kernel,
        "BootFS" | "policy" => &base.bootfs,
        "vbmeta" => &base.vbmeta,
        _ => match &base.x86_secure_firmware {
            Some(firmware) => &firmware.loader,
            None => &base.secure_firmware.as_ref().unwrap().bl33,
        },
    };
    let path = std::path::PathBuf::from(root).join(format!("tampered-{}", name.replace(' ', "-")));
    let mut bytes = std::fs::read(source).map_err(|e| format!("read {name}: {e}"))?;
    if bytes.is_empty() {
        return Err(format!("empty {name}"));
    }
    let index = if name == "policy" {
        let bootfs = bexos_kernel_core::bootfs::Bootfs::parse(&bytes)
            .map_err(|_| "invalid source BootFS")?;
        let policy = bootfs
            .find("/boot/platform.pcfg")
            .map_err(|_| "invalid policy entry")?
            .ok_or("missing source policy")?;
        if policy.bytes.is_empty() {
            return Err("empty source policy".into());
        }
        policy.bytes.as_ptr() as usize - bytes.as_ptr() as usize
    } else {
        bytes.len() / 2
    };
    bytes[index] ^= 0x80;
    std::fs::write(&path, bytes).map_err(|e| format!("write tampered {name}: {e}"))?;
    let mut artifacts = base.clone();
    replace(&mut artifacts, path);
    let mut candidate = QemuDevice::new_without_host_preflight(artifacts)?;
    let rejection: &[u8] = if base.x86_secure_firmware.is_some() {
        match name {
            "kernel" | "BootFS" | "policy" | "vbmeta" => {
                b"monitor-runtime: boot payload authentication rejected"
            }
            _ => b": Access Denied\r\n",
        }
    } else {
        match name {
            "kernel" => b"bl33: FATAL: kernel digest failed",
            "BootFS" | "policy" => b"bl33: FATAL: BootFS digest failed",
            "vbmeta" => b"bl33: FATAL: vbmeta authentication failed",
            _ => b"BL2: Failed to load image id 5 (-80)",
        }
    };
    candidate.assert_boot_rejected(
        &[rejection],
        &[
            b"userspace: entering el0 appd",
            b"userspace: entering ring3 appd",
            b"kernel: RPMB anti-rollback backend verified",
            b"appd: RPMB anti-rollback backend verified",
        ],
    )
}

fn assert_access_denied(result: Result<(), DebugClientError>, context: &str) -> Result<(), String> {
    match result {
        Err(DebugClientError::RemoteStatus(status)) if status.status == -4 => Ok(()),
        other => Err(format!("{context} should be rejected, got {other:?}")),
    }
}
