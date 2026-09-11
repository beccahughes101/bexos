use super::*;

pub(super) fn run(c: &mut UnixDebugClient, cmd: Command, format: Format) -> Result {
    match cmd {
        Command::Apps => {
            let rows = c
                .list_apps()?
                .into_iter()
                .map(|app| {
                    json!({
                        "package": app.package_id,
                        "state": app.state,
                        "source": app.source,
                        "protected": app.protected,
                        "name": app.name,
                    })
                })
                .collect();
            output::records(
                format,
                &[
                    ("package", "PACKAGE"),
                    ("state", "STATE"),
                    ("source", "SOURCE"),
                    ("protected", "PROTECTED"),
                    ("name", "NAME"),
                ],
                rows,
            )?;
        }
        Command::Install { bundle } => c.install_app_bundle(1, &read(&bundle)?)?,
        Command::InstallUrl { url } => c.install_app_from_url(&url)?,
        Command::ReloadWellKnown { domain } => c.reload_well_known(&domain)?,
        Command::Uninstall { package } => c.uninstall_app(&package)?,
        Command::Launch {
            package,
            process,
            arg0,
            uid,
        } => {
            c.launch_app(&package, &process, arg0, uid)?;
        }
        Command::App {
            command: AppCommand::Progress { package },
        } => {
            diagnostic_record(c, "app.progress", &[package], format)?;
        }
        Command::TestApp {
            command:
                TestAppCommand::Install {
                    package,
                    manifest,
                    artifact,
                },
        } => {
            c.install_test_app(1, &package, &read(&manifest)?, &read(&artifact)?)?;
        }
        Command::TestApp {
            command:
                TestAppCommand::Launch {
                    package,
                    process,
                    arg0,
                },
        } => {
            c.launch_test_app(&package, &process, arg0)?;
        }
        _ => unreachable!("app dispatch"),
    }
    Ok(())
}
