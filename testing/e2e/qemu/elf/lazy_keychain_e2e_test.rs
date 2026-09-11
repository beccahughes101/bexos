mod echo;

use bexos_e2e::E2eDevice;
use bexos_qemu_test::{QemuArtifacts, QemuDevice};
use echo::Echo;
use std::time::{Duration, Instant};

const KEYCHAIND_PACKAGE: &str = "bexos.service.keychaind";

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let (artifacts, extra) =
        QemuArtifacts::from_env_or_args(&std::env::args().skip(1).collect::<Vec<_>>())?;
    if extra.len() != 2 {
        return Err("expected library and executable archives".into());
    }
    let mut device = QemuDevice::new(artifacts)?;
    let mut session = device.boot_with_debugd(&lazy_boot_markers())?;
    assert_debugd_basics(&mut session)?;
    assert_keychaind_absent(&mut session, "before demand")?;
    for (index, path) in extra.iter().enumerate() {
        let bytes = std::fs::read(path).map_err(|e| format!("read ELF fixture: {e}"))?;
        session
            .client
            .install_app_bundle(0x4c_415a_5900 + index as u64, &bytes)
            .map_err(|e| {
                format!(
                    "install lazy keychain fixture: {e:?}\n{}",
                    String::from_utf8_lossy(session.client.received_trace())
                )
            })?;
    }
    let echo = Echo::start(false)?;
    session
        .client
        .launch_app(
            "bexos.platform.elf_probe",
            "elf_probe",
            echo.port as u64 | (2u64 << 32),
            0,
        )
        .map_err(|e| {
            format!(
                "launch lazy keychain probe: {e:?}\n{}",
                String::from_utf8_lossy(session.client.received_trace())
            )
        })?;
    assert_single_keychaind_running(&mut session, "after startup service connection")?;
    session.wait_for_serial_markers(
        &[
            b"elf-probe: 32 KiB TCP echo over virtio-net verified",
            b"elf-probe: lazy keychain two clients connected",
        ],
        Duration::from_secs(120),
    )?;
    assert_single_keychaind_running(&mut session, "with two clients")?;
    session.wait_for_serial_markers(
        &[b"elf-probe: lazy keychain clients released"],
        Duration::from_secs(30),
    )?;
    wait_for_keychaind_absent(&mut session, Duration::from_secs(20), "after idle release")?;
    session.wait_for_serial_markers(
        &[b"elf-probe: lazy keychain reconnected after idle stop"],
        Duration::from_secs(30),
    )?;
    assert_single_keychaind_running(&mut session, "after reconnect")?;
    wait_for_keychaind_absent(&mut session, Duration::from_secs(20), "after probe exit")?;
    Ok(())
}

fn lazy_boot_markers() -> Vec<&'static [u8]> {
    let appd_entry: &'static [u8] = if std::env::var("BEXOS_QEMU_ARCH").as_deref() == Ok("x86_64") {
        b"userspace: entering ring3 appd"
    } else {
        b"userspace: entering el0 appd"
    };
    vec![appd_entry, b"appd: app lifecycle registry ready for debugd"]
}

fn assert_debugd_basics<T: bexos_debug_client::DebugTransport>(
    session: &mut bexos_e2e::DebugSession<T>,
) -> Result<(), String> {
    let health = session.client.health_check().map_err(|e| {
        format!(
            "debugd health check: {e:?}\n{}",
            String::from_utf8_lossy(session.client.received_trace())
        )
    })?;
    if health.service_name != "debugd" || health.status != "SERVING" {
        return Err(format!("unexpected debugd health response: {health:?}"));
    }
    let processes = session.client.list_processes().map_err(|e| {
        format!(
            "debugd process list: {e:?}\n{}",
            String::from_utf8_lossy(session.client.received_trace())
        )
    })?;
    if processes.is_empty() {
        return Err("debugd returned an empty process list".into());
    }
    Ok(())
}

fn assert_keychaind_absent<T: bexos_debug_client::DebugTransport>(
    session: &mut bexos_e2e::DebugSession<T>,
    phase: &str,
) -> Result<(), String> {
    let processes = session
        .client
        .list_processes()
        .map_err(|e| format!("process list {phase}: {e:?}"))?;
    if processes
        .iter()
        .any(|process| process.package_id == KEYCHAIND_PACKAGE && process.state == "Running")
    {
        return Err(format!(
            "keychaind unexpectedly running {phase}: {processes:?}"
        ));
    }
    Ok(())
}

fn assert_single_keychaind_running<T: bexos_debug_client::DebugTransport>(
    session: &mut bexos_e2e::DebugSession<T>,
    phase: &str,
) -> Result<(), String> {
    let processes = session
        .client
        .list_processes()
        .map_err(|e| format!("process list {phase}: {e:?}"))?;
    let count = processes
        .iter()
        .filter(|process| process.package_id == KEYCHAIND_PACKAGE && process.state == "Running")
        .count();
    if count != 1 {
        return Err(format!(
            "expected one running keychaind {phase}, saw {count}: {processes:?}"
        ));
    }
    Ok(())
}

fn wait_for_keychaind_absent<T: bexos_debug_client::DebugTransport>(
    session: &mut bexos_e2e::DebugSession<T>,
    timeout: Duration,
    phase: &str,
) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    loop {
        if assert_keychaind_absent(session, phase).is_ok() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            let processes = session
                .client
                .list_processes()
                .map_err(|e| format!("process list {phase}: {e:?}"))?;
            return Err(format!(
                "keychaind remained running {phase} for {timeout:?}: {processes:?}\n{}",
                String::from_utf8_lossy(session.client.received_trace())
            ));
        }
        session
            .client
            .drain_for(Duration::from_millis(25))
            .map_err(|e| format!("trace drain {phase}: {e:?}"))?;
    }
}
