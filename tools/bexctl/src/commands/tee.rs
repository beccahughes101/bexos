use super::*;
pub(super) fn run(c: &mut UnixDebugClient, cmd: TeeCommand, f: Format) -> Result {
    match cmd {
        TeeCommand::Info => {
            let v = c.tee_info()?;
            output::records(
                f,
                &[
                    ("present", "PRESENT"),
                    ("kind", "KIND"),
                    ("secure_os_version", "OS VERSION"),
                    ("anti_rollback_version", "ROLLBACK FLOOR"),
                ],
                vec![
                    json!({"present":v.present,"kind":v.kind,"secure_os_version":v.secure_os_version,"anti_rollback_version":v.anti_rollback_version}),
                ],
            )?;
        }
        TeeCommand::Apps | TeeCommand::ListPackages => {
            let packages = matches!(cmd, TeeCommand::ListPackages);
            let rows = c
                .tee_apps()?
                .into_iter()
                .filter(|app| !packages || app.package_managed)
                .map(|app| {
                    Ok(json!({
                        "uuid": input::format_uuid(&app.uuid)?,
                        "package": app.package_id,
                        "name": app.entry_point_name,
                        "version": app.version,
                        "sessions": app.active_sessions,
                        "storage_bytes": app.storage_bytes_used,
                        "protected": app.protected,
                        "package_managed": app.package_managed,
                    }))
                })
                .collect::<Result<Vec<_>>>()?;
            output::records(
                f,
                &[
                    ("uuid", "UUID"),
                    ("package", "PACKAGE"),
                    ("name", "NAME"),
                    ("version", "VERSION"),
                    ("sessions", "SESSIONS"),
                    ("storage_bytes", "STORAGE BYTES"),
                    ("protected", "PROTECTED"),
                ],
                rows,
            )?;
        }
        TeeCommand::InstallApp { file } => {
            let uuid = c.tee_install_app(read(&file)?)?;
            output::records(
                f,
                &[("uuid", "UUID")],
                vec![json!({"uuid":input::format_uuid(&uuid)?})],
            )?;
        }
        TeeCommand::UninstallApp { uuid } => c.tee_uninstall_app(input::parse_uuid(&uuid)?)?,
        TeeCommand::OpenSession { uuid } => {
            let id = c.tee_open_session(input::parse_uuid(&uuid)?)?;
            output::records(
                f,
                &[("session_id", "SESSION ID")],
                vec![json!({"session_id":id})],
            )?;
        }
        TeeCommand::CloseSession { session_id } => c.tee_close_session(session_id)?,
        TeeCommand::Invoke {
            session_id,
            command_id,
            payload,
        } => {
            use std::io::Write;
            let bytes = payload
                .as_deref()
                .map(read)
                .transpose()?
                .unwrap_or_default();
            std::io::stdout().write_all(&c.tee_invoke(session_id, command_id, bytes)?)?;
        }
        TeeCommand::UpdateCoreRaw {
            generation,
            target,
            image,
        } => {
            let image = read(&image)?;
            let hash = bexos_crypto::blake3_256(&image);
            let r = c.tee_update_core(generation, target, hash.to_vec(), image)?;
            output::records(
                f,
                &[("version", "VERSION"), ("message", "MESSAGE")],
                vec![json!({"version":r.new_version,"message":r.message})],
            )?;
        }
        TeeCommand::UpdateCore { manifest, image } => {
            updates::apply(c, UpdateKind::Platform, &manifest, &image)?
        }
        TeeCommand::UpdateStatus => {
            let s = c.tee_update_status()?;
            output::records(
                f,
                &[
                    ("status", "STATUS"),
                    ("generation", "GENERATION"),
                    ("phase", "PHASE"),
                    ("active_slot", "ACTIVE"),
                    ("pending_slot", "PENDING"),
                    ("reboot_required", "REBOOT"),
                    ("message", "MESSAGE"),
                ],
                vec![
                    json!({"status":s.update_status,"generation":s.generation,"phase":s.phase,"active_slot":s.active_slot,"pending_slot":s.pending_slot,"reboot_required":s.reboot_required,"message":s.message}),
                ],
            )?;
        }
        TeeCommand::Diagnostic { diagnostic } => exec(
            c,
            match diagnostic {
                TeeDiagnostic::Keymint => "tee.keymint_smoke",
                TeeDiagnostic::Orchestrator => "tee.orchestrator_smoke",
                TeeDiagnostic::Authmgr => "tee.authmgr_smoke",
                TeeDiagnostic::Concurrent => "tee.concurrent_smoke",
            },
            &[],
        )?,
    }
    Ok(())
}
