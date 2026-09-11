use bexos_component_config::{ConfigTable, ConfigType, encode_config};
use bexos_debug_wire::PreferencesRequest;
use bexos_e2e::{E2eDevice, boot_markers};
use bexos_qemu_test::{QemuArtifacts, QemuDevice};
use std::time::Duration;
const PACKAGE: &str = "com.example.preferences_probe";
fn request(uid: u64, operation: u32, generation: u64, config: Vec<u8>) -> PreferencesRequest {
    PreferencesRequest {
        package_id: PACKAGE.into(),
        uid,
        operation,
        expected_generation: generation,
        config,
        names: Vec::new(),
    }
}
fn main() {
    if let Err(e) = run() {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
fn run() -> Result<(), String> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let (mut artifacts, extra) = QemuArtifacts::from_env_or_args(&args)?;
    // These are fixture archives consumed below, not QEMU command-line flags.
    artifacts.extra_args.clear();
    let archive =
        std::fs::read(extra.first().ok_or("probe archive")?).map_err(|e| e.to_string())?;
    let mut markers = boot_markers();
    markers.push(b"appd: app lifecycle registry ready for debugd");
    let mut device = QemuDevice::new(artifacts)?;
    let mut session = device.boot_with_debugd(&markers)?;
    session.assert_debugd_ready()?;
    let result = (|| -> Result<(), String> {
        eprintln!("preferences: install");
        session
            .client
            .install_app_bundle(0x50524546, &archive)
            .map_err(|e| format!("install {e:?}"))?;
        eprintln!("preferences: initial read");
        let initial = preferences(&mut session.client, &request(0, 0, 0, vec![]))
            .map_err(|e| format!("get {e:?}"))?;
        assert_eq!(initial.generation, 0);
        assert_eq!(initial.locks, vec!["managed"]);
        let storage_only = std::env::var_os("BEXOS_PREFERENCES_STORAGE_ONLY").is_some();
        if !storage_only {
            assert!(
                preferences(
                    &mut session.client,
                    &PreferencesRequest {
                        package_id: PACKAGE.into(),
                        operation: 4,
                        names: vec!["managed".into()],
                        ..Default::default()
                    }
                )
                .is_err()
            );
            assert_eq!(
                ConfigTable::parse(&initial.config)
                    .unwrap()
                    .get_bool("dark"),
                Ok(false)
            );
            eprintln!("preferences: live launch");
            session
                .client
                .launch_app(PACKAGE, "probe", 0, 0)
                .map_err(|e| format!("launch {e:?}"))?;
            session
                .client
                .launch_app(PACKAGE, "probe", 2, 0)
                .map_err(|e| format!("legacy launch {e:?}"))?;
            session
                .client
                .launch_app(PACKAGE, "probe", 0, 0)
                .map_err(|e| format!("second receiver {e:?}"))?;
            eprintln!("preferences: live set");
            let set = preferences(
                &mut session.client,
                &request(0, 1, 0, encode_config(&[("dark", ConfigType::Bool, &[1])])),
            )
            .map_err(|e| format!("set {e:?}"))?;
            assert!(matches!(set.status, 0 | 1));
            assert_eq!(set.generation, 1);
            session.wait_for_serial_markers(
                &[
                    b"prefs-probe: committed dark=true cache=64",
                    b"prefs-probe: retained client generation=",
                ],
                Duration::from_secs(30),
            )?;
            assert!(
                preferences(
                    &mut session.client,
                    &request(0, 1, 0, encode_config(&[("dark", ConfigType::Bool, &[0])]))
                )
                .is_err()
            );
            assert!(
                preferences(
                    &mut session.client,
                    &request(
                        0,
                        1,
                        1,
                        encode_config(&[("system", ConfigType::Bool, &[1])])
                    )
                )
                .is_err()
            );
            assert!(
                preferences(
                    &mut session.client,
                    &request(
                        0,
                        1,
                        1,
                        encode_config(&[("cache", ConfigType::Uint32, &999u32.to_le_bytes())])
                    )
                )
                .is_err()
            );
            let current = preferences(&mut session.client, &request(0, 0, 0, vec![]))
                .map_err(|e| format!("after reject {e:?}"))?;
            assert_eq!(current.generation, 1);
            let policy = preferences(
                &mut session.client,
                &PreferencesRequest {
                    package_id: PACKAGE.into(),
                    operation: 3,
                    names: vec!["cache".into()],
                    ..Default::default()
                },
            )
            .map_err(|e| format!("lock {e:?}"))?;
            assert_eq!(policy.generation, 1);
            let current = preferences(&mut session.client, &request(0, 0, 0, vec![]))
                .map_err(|e| format!("after lock {e:?}"))?;
            assert_eq!(current.locks, vec!["cache", "managed"]);
            assert!(
                preferences(
                    &mut session.client,
                    &request(
                        0,
                        1,
                        current.generation,
                        encode_config(&[("cache", ConfigType::Uint32, &256u32.to_le_bytes())])
                    )
                )
                .is_err()
            );
            preferences(
                &mut session.client,
                &PreferencesRequest {
                    package_id: PACKAGE.into(),
                    operation: 4,
                    expected_generation: 1,
                    names: vec!["cache".into()],
                    ..Default::default()
                },
            )
            .map_err(|e| format!("unlock {e:?}"))?;
            let current = preferences(&mut session.client, &request(0, 0, 0, vec![]))
                .map_err(|e| format!("get for reset {e:?}"))?;
            preferences(
                &mut session.client,
                &request(0, 2, current.generation, vec![]),
            )
            .map_err(|e| format!("reset {e:?}"))?;
            eprintln!("preferences: compatible operator CLI");
            let operator = session
                .client
                .get_component_config(PACKAGE)
                .map_err(|e| format!("operator get {e:?}"))?;
            let changed = session
                .client
                .set_component_config(
                    PACKAGE,
                    operator.generation,
                    &encode_config(&[("system", ConfigType::Bool, &[1])]),
                )
                .map_err(|e| format!("operator set {e:?}"))?;
            let changed = session
                .client
                .set_component_config(
                    PACKAGE,
                    changed.generation,
                    &encode_config(&[("cache", ConfigType::Uint32, &256u32.to_le_bytes())]),
                )
                .map_err(|e| format!("operator overlay {e:?}"))?;
            let overlay = session
                .client
                .get_component_config(PACKAGE)
                .map_err(|e| format!("operator overlay get {e:?}"))?;
            let table = ConfigTable::parse(&overlay.config).unwrap();
            assert_eq!(table.get_bool("system"), Ok(true));
            assert_eq!(table.get_u32("cache"), Ok(256));
            session
                .client
                .reset_component_config(PACKAGE, changed.generation)
                .map_err(|e| format!("operator reset {e:?}"))?;
        } else {
            eprintln!("preferences: focused encrypted-storage run");
            session
                .client
                .set_component_config(
                    PACKAGE,
                    0,
                    &encode_config(&[("system", ConfigType::Bool, &[1])]),
                )
                .map_err(|e| format!("storage operator revision {e:?}"))?;
        }
        eprintln!("preferences: encrypted user storage");
        session
            .client
            .create_user(1000, "prefuser", "Preferences", "test-password")
            .map_err(|e| format!("create {e:?}"))?;
        session
            .client
            .unlock_user(1000, "test-password")
            .map_err(|e| format!("unlock user {e:?}"))?;
        let user = preferences(&mut session.client, &request(1000, 0, 0, vec![]))
            .map_err(|e| format!("user get {e:?}"))?;
        preferences(
            &mut session.client,
            &request(
                1000,
                1,
                user.generation,
                encode_config(&[("dark", ConfigType::Bool, &[1])]),
            ),
        )
        .map_err(|e| format!("user set {e:?}"))?;
        let system = preferences(&mut session.client, &request(0, 0, 0, vec![]))
            .map_err(|e| format!("system get {e:?}"))?;
        assert_eq!(
            ConfigTable::parse(&system.config).unwrap().get_bool("dark"),
            Ok(false)
        );
        session
            .client
            .lock_user(1000)
            .map_err(|e| format!("lock user {e:?}"))?;
        assert!(preferences(&mut session.client, &request(1000, 0, 0, vec![])).is_err());
        session
            .client
            .unlock_user(1000, "test-password")
            .map_err(|e| format!("reunlock {e:?}"))?;
        let restored = preferences(&mut session.client, &request(1000, 0, 0, vec![]))
            .map_err(|e| format!("restored {e:?}"))?;
        assert_eq!(
            ConfigTable::parse(&restored.config)
                .unwrap()
                .get_bool("dark"),
            Ok(true)
        );

        if storage_only {
            return Ok(());
        }
        // Both providers must retain the original observer connections across cutover.
        for (package, generation, archive) in [
            ("bexos.service.prefsd", 70, "prefsd-70"),
            ("bexos.platform.appd", 71, "appd-71"),
        ] {
            eprintln!("preferences: transplant {package}");
            session.client.clear_received_trace();
            let result = session
                .client
                .exec_command(
                    "update.apply_stored_service",
                    &[package.into(), generation.to_string(), archive.into()],
                )
                .map_err(|e| format!("transplant {package}: {e:?}"))?;
            if result.exit_code != 0 {
                return Err(format!("transplant {package}: {result:?}"));
            }
            let marker =
                format!("service-transplant: committed generation={generation} cutover_ms=");
            session.wait_for_serial_markers(&[marker.as_bytes()], Duration::from_secs(180))?;
            let current = preferences(&mut session.client, &request(0, 0, 0, vec![]))
                .map_err(|e| format!("get after {package}: {e:?}"))?;
            session.client.clear_received_trace();
            let applied = preferences(
                &mut session.client,
                &request(
                    0,
                    1,
                    current.generation,
                    encode_config(&[("dark", ConfigType::Bool, &[1])]),
                ),
            )
            .map_err(|e| format!("set after {package}: {e:?}"))?;
            let retained = format!(
                "prefs-probe: retained client generation={}",
                applied.generation
            );
            session.wait_for_serial_markers(
                &[
                    b"prefs-probe: committed dark=true cache=64",
                    retained.as_bytes(),
                ],
                Duration::from_secs(300),
            )?;
        }
        // A registered receiver that stops polling causes the five-second prepare deadline.
        session
            .client
            .launch_app(PACKAGE, "probe", 1, 0)
            .map_err(|e| format!("timeout launch {e:?}"))?;
        let before = preferences(&mut session.client, &request(0, 0, 0, vec![]))
            .map_err(|e| format!("before timeout {e:?}"))?;
        let failed = preferences(
            &mut session.client,
            &request(
                0,
                1,
                before.generation,
                encode_config(&[("dark", ConfigType::Bool, &[0])]),
            ),
        );
        assert!(format!("{failed:?}").contains("Timeout"), "{failed:?}");
        let after = preferences(&mut session.client, &request(0, 0, 0, vec![]))
            .map_err(|e| format!("after timeout {e:?}"))?;
        assert_eq!(before.generation, after.generation);
        assert_eq!(before.config, after.config);
        Ok(())
    })();
    result.map_err(|e| {
        format!(
            "{e}\n{}",
            String::from_utf8_lossy(session.client.received_trace())
        )
    })?;
    let before = preferences(&mut session.client, &request(0, 0, 0, vec![]))
        .map_err(|e| format!("before reboot {e:?}"))?;
    eprintln!("preferences: reboot");
    drop(session);
    let mut session = device.boot_with_debugd(&markers)?;
    session.assert_debugd_ready()?;
    let after = preferences(&mut session.client, &request(0, 0, 0, vec![]))
        .map_err(|e| format!("after reboot {e:?}"))?;
    assert_eq!(after.config, before.config);
    assert_eq!(after.generation, before.generation);
    assert!(preferences(&mut session.client, &request(1000, 0, 0, vec![])).is_err());
    session
        .client
        .unlock_user(1000, "test-password")
        .map_err(|e| format!("reboot user unlock {e:?}"))?;
    let user = preferences(&mut session.client, &request(1000, 0, 0, vec![]))
        .map_err(|e| format!("reboot user get {e:?}"))?;
    assert_eq!(
        ConfigTable::parse(&user.config).unwrap().get_bool("dark"),
        Ok(true)
    );
    Ok(())
}

fn preferences<T: bexos_debug_client::DebugTransport>(
    client: &mut bexos_debug_client::DebugClient<T>,
    request: &PreferencesRequest,
) -> Result<bexos_debug_wire::PreferencesResponse, bexos_debug_client::DebugClientError> {
    let deadline = std::time::Instant::now() + Duration::from_secs(300);
    loop {
        match client.preferences(request) {
            Err(bexos_debug_client::DebugClientError::RemoteStatus(status))
                if status.status == -11 && std::time::Instant::now() < deadline =>
            {
                client.drain_for(Duration::from_millis(50))?;
            }
            result => return result,
        }
    }
}
