mod echo;
mod transplant;
use bexos_e2e::{E2eDevice, boot_markers};
use bexos_qemu_test::{QemuArtifacts, QemuDevice};
use echo::Echo;
fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
fn run() -> Result<(), String> {
    let (artifacts, extra) =
        QemuArtifacts::from_env_or_args(&std::env::args().skip(1).collect::<Vec<_>>())?;
    if extra.len() != 2 && extra.len() != 3 {
        return Err("expected library and executable archives".into());
    }
    let mut device = QemuDevice::new(artifacts)?;
    let mut session = device.boot_with_debugd(&boot_markers())?;
    session.assert_debugd_ready()?;
    for (index, path) in extra.iter().take(2).enumerate() {
        let bytes = std::fs::read(path).map_err(|e| format!("read ELF fixture: {e}"))?;
        session
            .client
            .install_app_bundle(0x454c4600 + index as u64, &bytes)
            .map_err(|e| {
                format!(
                    "install ELF fixture: {e:?}\n{}",
                    String::from_utf8_lossy(session.client.received_trace())
                )
            })?;
    }
    let transplant = extra.get(2).is_some_and(|arg| arg == "transplant");
    let echo = Echo::start(transplant)?;
    session
        .client
        .launch_app(
            "bexos.platform.elf_probe",
            "elf_probe",
            echo.port as u64 | (u64::from(transplant) << 32),
            0,
        )
        .map_err(|e| {
            format!(
                "launch ELF fixture: {e:?}\n{}",
                String::from_utf8_lossy(session.client.received_trace())
            )
        })?;
    session.wait_for_serial_markers(
        &[
            b"elf-probe: constructors and executable/library TLS across three threads verified",
            b"elf-probe: timer preemption and vector preservation verified",
            b"elf-probe: 32 KiB TCP echo over virtio-net verified",
        ],
        std::time::Duration::from_secs(120),
    )?;
    if transplant {
        transplant::replace_network(&mut session)?;
        echo.resume();
        session.wait_for_serial_markers(
            &[
                b"elf-probe: retained TCP stream survived network replacements",
                b"elf-probe: keychain secret and client survived replacement",
            ],
            std::time::Duration::from_secs(60),
        )?;
    }
    Ok(())
}
