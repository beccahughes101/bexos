use super::*;
pub(super) fn run(c: &mut UnixDebugClient, cmd: ConfigCommand, f: Format) -> Result {
    let (package, generation, message) = match cmd {
        ConfigCommand::Lock {
            package,
            expected_generation,
            names,
        } => return preferences::policy(c, package, expected_generation, names, 3, f),
        ConfigCommand::Unlock {
            package,
            expected_generation,
            names,
        } => return preferences::policy(c, package, expected_generation, names, 4, f),
        ConfigCommand::Policy { package } => {
            return preferences::policy(c, package, 0, Vec::new(), 5, f);
        }
        ConfigCommand::Get { package, output } => {
            let r = c.get_component_config(&package)?;
            if let Some(path) = output {
                write(&path, &r.config)?;
            }
            (package, r.generation, format!("{} bytes", r.config.len()))
        }
        ConfigCommand::Set {
            package,
            expected_generation,
            assignments,
        } => {
            let r = c.set_component_config(
                &package,
                expected_generation,
                &input::encode_cli_config(&assignments)?,
            )?;
            (package, r.generation, r.message)
        }
        ConfigCommand::Reset {
            package,
            expected_generation,
        } => {
            let r = c.reset_component_config(&package, expected_generation)?;
            (package, r.generation, r.message)
        }
    };
    output::records(
        f,
        &[
            ("package", "PACKAGE"),
            ("generation", "GENERATION"),
            ("message", "RESULT"),
        ],
        vec![json!({"package":package,"generation":generation,"message":message})],
    )?;
    Ok(())
}
