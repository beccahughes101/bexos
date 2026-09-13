mod checks;
mod qmp;
use bexos_component_config::{ConfigTable, ConfigType, encode_config};
use bexos_debug_client::{DebugClient, DebugTransport};
use bexos_debug_wire::PreferencesRequest;
use bexos_e2e::{E2eDevice, boot_markers};
use bexos_qemu_test::{QemuArtifacts, QemuDevice};
use std::time::{Duration, Instant};
const PREFS: &str = "bexos.locale.preferences";
fn main() {
    if let Err(e) = run() {
        eprintln!("localization acceptance: {e}");
        std::process::exit(1);
    }
}
fn run() -> Result<(), String> {
    let (mut artifacts, _) =
        QemuArtifacts::from_env_or_args(&std::env::args().skip(1).collect::<Vec<_>>())?;
    let output = std::env::var_os("TEST_UNDECLARED_OUTPUTS_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let qmp_path = std::env::temp_dir().join(format!("locale-{}.sock", std::process::id()));
    // Q35 secure-product policy authenticates the workstation devices at
    // slots 7-9. The AArch64 harness already reserves slot 7 for its RPMB
    // serial bridge, so keep the same device identities at free virt slots.
    let (gpu_slot, keyboard_slot, tablet_slot) =
        if std::env::var("BEXOS_QEMU_ARCH").as_deref() == Ok("x86_64") {
            (7, 8, 9)
        } else {
            (10, 11, 12)
        };
    artifacts.extra_args = vec![
        "-vga".into(),
        "none".into(),
        "-device".into(),
        format!(
            "virtio-gpu-pci,addr={gpu_slot},disable-legacy=on,disable-modern=off,iommu_platform=on,xres=800,yres=600"
        ),
        "-device".into(),
        format!(
            "virtio-keyboard-pci,addr={keyboard_slot},disable-legacy=on,disable-modern=off,iommu_platform=on"
        ),
        "-device".into(),
        format!(
            "virtio-tablet-pci,addr={tablet_slot},disable-legacy=on,disable-modern=off,iommu_platform=on"
        ),
        "-qmp".into(),
        format!("unix:{},server=on,wait=off", qmp_path.display()),
    ];
    let mut device = QemuDevice::new(artifacts)?;
    let mut markers = boot_markers();
    markers.push(b"appd: app lifecycle registry ready for debugd");
    markers.push(b"fontd: ready");
    markers.push(b"localed: ready");
    markers.push(b"scened: ready background presented");
    let mut session = device.boot_with_debugd(&markers)?;
    session.assert_debugd_ready()?;

    let result = (|| -> Result<(), String> {
        let mut q = qmp::Qmp::connect(&qmp_path, output.clone())?;
        checks::cold_process(&mut session.client, "bexos.app.sysui")?;
        session
            .client
            .create_user(1000, "alice", "Alice", "testpass")
            .map_err(|e| format!("create user {e:?}"))?;
        session
            .client
            .drain_for(Duration::from_secs(3))
            .map_err(|e| format!("user settle {e:?}"))?;
        checks::login(&mut session.client, &mut q, 1000)?;
        session
            .client
            .launch_app("bexos.test.locale", "locale_fixture", 0, 1000)
            .map_err(|e| format!("fixture launch {e:?}"))?;
        wait(
            &mut session.client,
            "locale-fixture: frame language=en-US generation=0 counter=17 text=retained",
        )?;
        checks::cold_screen(
            &mut session.client,
            &mut q,
            "locale-window",
            50,
            35,
            [60, 92, 136],
        )?;
        q.click(150, 240)?;
        q.type_text("a")?;
        wait(&mut session.client, "counter=18 text=retaineda")?;
        session.client.clear_received_trace();
        q.wheel_down()?;
        wait(&mut session.client, "counter=18 text=retaineda scroll=")?;
        let scroll = String::from_utf8_lossy(session.client.received_trace())
            .lines().rev().find_map(|line| line.split_once("text=retaineda scroll=")
                .and_then(|(_, rest)| rest.split_whitespace().next()).map(str::to_owned))
            .ok_or("missing scroll state")?;
        if scroll.parse::<f32>().map_err(|_| "invalid scroll state")? == 0.0 {
            return Err("wheel input did not change scroll state".into());
        }
        let english = q.screenshot("locale-en")?;
        session.client.clear_received_trace();
        set(&mut session.client, 1000, "de-DE", Some("de-DE"))?;
        wait(
            &mut session.client,
            "locale-fixture: frame language=de-DE generation=1 counter=18 text=retaineda",
        )?;
        wait(&mut session.client, "title=Hallo 18 region=1.234,50")?;
        wait(&mut session.client, &format!("counter=18 text=retaineda scroll={scroll} "))?;
        let german = q.screenshot("locale-de")?;
        if english == german {
            return Err("locale change did not change visible pixels".into());
        }
        // Keyboard focus remains in the same window after document invalidation.
        q.type_text("b")?;
        wait(&mut session.client, "counter=18 text=retainedab")?;
        session
            .client
            .launch_app("bexos.test.locale_native", "native", 0, 1000)
            .map_err(|e| format!("native launch {e:?}"))?;
        wait(
            &mut session.client,
            "locale-native: number=1.234,50 plural=few readonly=true generation=1",
        )?;
        session
            .client
            .launch_app("bexos.test.locale_native", "native", 0, 0)
            .map_err(|e| format!("isolated native launch {e:?}"))?;
        wait(
            &mut session.client,
            "locale-native: number=1,234.50 plural=few readonly=true generation=0",
        )?;
        let prior = preferences(&mut session.client, 1000)?;
        let rejected = session.client.preferences(&PreferencesRequest {
            package_id: PREFS.into(),
            uid: 1000,
            operation: 1,
            expected_generation: prior.generation,
            config: encode_config(&[("language_priority", ConfigType::String, b"en_US")]),
            ..Default::default()
        });
        if rejected.is_ok_and(|r| matches!(r.status, 0 | 1)) {
            return Err("malformed locale transaction was accepted".into());
        }
        assert_eq!(
            preferences(&mut session.client, 1000)?.generation,
            prior.generation
        );
        for (package, generation, archive) in [
            ("bexos.service.localed", 201, "locale-localed"),
            ("bexos.service.prefsd", 202, "locale-prefsd"),
            ("bexos.platform.appd", 203, "locale-appd"),
            ("bexos.test.locale", 204, "locale-fixture"),
        ] {
            checks::transplant(&mut session.client, package, generation, archive, true)?;
        }
        session.client.clear_received_trace();
        set(&mut session.client, 1000, "ru", None)?;
        wait(
            &mut session.client,
            "locale-fixture: frame language=ru generation=2 counter=18 text=retainedab",
        )?;
        wait(&mut session.client, "title=Привет 18 region=1.234,50")?;
        wait(&mut session.client, &format!("counter=18 text=retainedab scroll={scroll} "))?;
        set(&mut session.client, 1000, "ar", None)?;
        wait(
            &mut session.client,
            "locale-fixture: frame language=ar generation=3 counter=18 text=retainedab",
        )?;
        wait(&mut session.client, "title=مرحبا 18 region=1.234,50")?;
        let arabic = q.screenshot("locale-ar")?;
        if german == arabic {
            return Err("RTL translation did not change visible pixels".into());
        }
        // Another edit after app migration checks retained keyboard delivery.
        q.type_text("c")?;
        wait(&mut session.client, "counter=18 text=retainedabc")?;
        // A separate account exercises cached-snapshot revocation without
        // terminating the foreground fixture's session.
        session.client.create_user(1001, "bob", "Bob", "testpass")
            .map_err(|e| format!("create isolated user {e:?}"))?;
        session.client.unlock_user(1001, "testpass")
            .map_err(|e| format!("unlock isolated user {e:?}"))?;
        set(&mut session.client, 1001, "de-DE", Some("de-DE"))?;
        native(&mut session.client, 1001, "1.234,50", 1)?;
        session.client.lock_user(1001).map_err(|e| format!("lock isolated user {e:?}"))?;
        if preferences(&mut session.client, 1001).is_ok_and(|r| r.status == 0) {
            return Err("locked user's locale preferences remained readable".into());
        }
        session.client.unlock_user(1001, "testpass")
            .map_err(|e| format!("re-unlock isolated user {e:?}"))?;
        set(&mut session.client, 1001, "en-US", Some("en-US"))?;
        native(&mut session.client, 1001, "1,234.50", 2)?;
        session.client.delete_user(1001).map_err(|e| format!("delete isolated user {e:?}"))?;
        session.client.create_user(1001, "bob", "Bob", "testpass")
            .map_err(|e| format!("recreate isolated user {e:?}"))?;
        session.client.unlock_user(1001, "testpass")
            .map_err(|e| format!("unlock recreated user {e:?}"))?;
        native(&mut session.client, 1001, "1,234.50", 0)?;
        session.client.delete_user(1001).map_err(|e| format!("clean isolated user {e:?}"))?;
        Ok(())
    })();
    if result.is_err() {
        eprintln!(
            "{}",
            String::from_utf8_lossy(session.client.received_trace())
        );
    }
    result?;
    drop(session);
    let mut session = device.boot_with_debugd(&markers)?;
    session.assert_debugd_ready()?;
    let mut q = qmp::Qmp::connect(&qmp_path, output)?;
    checks::login(&mut session.client, &mut q, 1000)?;
    let saved = preferences(&mut session.client, 1000)?;
    let table = ConfigTable::parse(&saved.config).map_err(|e| format!("saved locale {e:?}"))?;
    assert_eq!(table.get_string("language_priority"), Ok("ar"));
    assert_eq!(table.get_string("override_region"), Ok("de-DE"));
    eprintln!(
        "localization: translations, retained input, parity, read-only mappings, migration and persistence passed"
    );
    Ok(())
}
fn native(client: &mut DebugClient<impl DebugTransport>, uid: u64, number: &str, generation: u64) -> Result<(), String> {
    client.clear_received_trace();
    client.launch_app("bexos.test.locale_native", "native", 0, uid)
        .map_err(|e| format!("isolated native launch {e:?}"))?;
    wait(client, &format!("locale-native: number={number} plural=few readonly=true generation={generation}"))
}
fn wait(client: &mut DebugClient<impl DebugTransport>, marker: &str) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(360);
    loop {
        client
            .drain_for(Duration::from_millis(250))
            .map_err(|e| format!("trace {e:?}"))?;
        let trace = String::from_utf8_lossy(client.received_trace());
        if trace.contains(marker) {
            return Ok(());
        }
        if trace.contains("locale-fixture: failed") || trace.contains("wasm_runner: failed:") {
            return Err(trace.into_owned());
        }
        if Instant::now() >= deadline {
            return Err(format!("missing {marker}: {trace}"));
        }
    }
}
fn preferences(
    client: &mut DebugClient<impl DebugTransport>,
    uid: u64,
) -> Result<bexos_debug_wire::PreferencesResponse, String> {
    client
        .preferences(&PreferencesRequest {
            package_id: PREFS.into(),
            uid,
            ..Default::default()
        })
        .map_err(|e| format!("preferences {e:?}"))
}
fn set(
    client: &mut DebugClient<impl DebugTransport>,
    uid: u64,
    language: &str,
    region: Option<&str>,
) -> Result<(), String> {
    let current = preferences(client, uid)?;
    let mut values = vec![("language_priority", ConfigType::String, language.as_bytes())];
    if let Some(region) = region {
        values.push(("override_region", ConfigType::String, region.as_bytes()));
    }
    let result = client
        .preferences(&PreferencesRequest {
            package_id: PREFS.into(),
            uid,
            operation: 1,
            expected_generation: current.generation,
            config: encode_config(&values),
            ..Default::default()
        })
        .map_err(|e| format!("set locale {e:?}"))?;
    if !matches!(result.status, 0 | 1) {
        return Err(format!("set locale rejected {result:?}"));
    }
    Ok(())
}
