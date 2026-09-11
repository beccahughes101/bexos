use super::*;
pub(super) fn apply(
    c: &mut UnixDebugClient,
    kind: UpdateKind,
    manifest: &Path,
    artifact: &Path,
) -> Result {
    let m = read(manifest)?;
    let a = read(artifact)?;
    c.upload_update(1, &m, &a)?;
    exec(c, method(kind), &[])
}
fn method(kind: UpdateKind) -> &'static str {
    match kind {
        UpdateKind::App => "update.apply_app",
        UpdateKind::Service => "update.apply_service",
        UpdateKind::Platform => "update.apply_platform",
    }
}
pub(super) fn run(c: &mut UnixDebugClient, cmd: UpdateCommand, f: Format) -> Result {
    match cmd {
        UpdateCommand::Firmware {
            manifest,
            artifact,
            activation,
        } => {
            c.upload_update(1, &read(&manifest)?, &read(&artifact)?)?;
            exec(
                c,
                "update.apply_firmware",
                &[match activation {
                    FirmwareActivation::Live => "live",
                    FirmwareActivation::OnReboot => "on-reboot",
                }
                .into()],
            )?;
        }
        UpdateCommand::Check {
            target,
            all,
            stage,
            apply,
            activation,
        } => {
            let (kind, target) = match target.as_deref() {
                None => (1, ""),
                Some("KERNEL" | "kernel") => (3, ""),
                Some("TEE" | "tee") => (4, ""),
                Some("HYPERVISOR" | "hypervisor") => (5, ""),
                Some(p) => (2, p),
            };
            if let Some(activation) = activation {
                if all || !matches!(kind, 4 | 5) {
                    return Err("--activation requires a TEE or HYPERVISOR selector".into());
                }
                exec(
                    c,
                    "update.apply_firmware_from_feed",
                    &[
                        if kind == 4 { "tee" } else { "hypervisor" }.into(),
                        match activation {
                            FirmwareActivation::Live => "live",
                            FirmwareActivation::OnReboot => "on-reboot",
                        }
                        .into(),
                    ],
                )?;
                return Ok(());
            }
            let r = c.check_updates(kind, target, all, stage || apply, apply)?;
            if stage || apply {
                output::records(
                    f,
                    &[("message", "RESULT")],
                    vec![json!({"message":r.message})],
                )?;
            } else {
                let rows = r.candidates.into_iter().map(|candidate| json!({
                    "kind": match candidate.kind {
                        1 => "APP_PACKAGE", 2 => "MICROKERNEL", 3 => "TEE_IMAGE", 4 => "HYPERVISOR", _ => "UNKNOWN",
                    },
                    "target": candidate.target,
                    "generation": candidate.generation,
                    "bytes": candidate.length,
                    "source": if candidate.url.is_empty() { candidate.path } else { candidate.url },
                })).collect();
                output::records(
                    f,
                    &[
                        ("kind", "KIND"),
                        ("target", "TARGET"),
                        ("generation", "GENERATION"),
                        ("bytes", "BYTES"),
                        ("source", "SOURCE"),
                    ],
                    rows,
                )?;
            }
        }
        UpdateCommand::App { manifest, artifact } => {
            apply(c, UpdateKind::App, &manifest, &artifact)?
        }
        UpdateCommand::Service { manifest, artifact } => {
            apply(c, UpdateKind::Service, &manifest, &artifact)?
        }
        UpdateCommand::Platform { manifest, artifact } => {
            apply(c, UpdateKind::Platform, &manifest, &artifact)?
        }
        UpdateCommand::Status => diagnostic_record(c, "update.status", &[], f)?,
        UpdateCommand::Apply { kind } => exec(c, method(kind), &[])?,
        UpdateCommand::StoredService {
            target,
            generation,
            archive_id,
        } => {
            let archive = archive_id.unwrap_or_else(|| target.clone());
            exec(
                c,
                "update.apply_stored_service",
                &[target, generation.to_string(), archive],
            )?;
        }
        UpdateCommand::ServiceStatus { target } => {
            diagnostic_record(c, "update.service.status", &[target], f)?
        }
    }
    Ok(())
}
