use super::*;
use bexos_debug_wire::PreferencesRequest;
pub(super) fn run(c: &mut UnixDebugClient, cmd: PreferencesCommand, f: Format) -> Result {
    let (package, uid, operation, expected_generation, config, output) = match cmd {
        PreferencesCommand::Get {
            package,
            uid,
            output,
        } => (package, uid, 0, 0, Vec::new(), output),
        PreferencesCommand::Set {
            package,
            uid,
            expected_generation,
            assignments,
        } => (
            package,
            uid,
            1,
            expected_generation,
            input::encode_cli_config(&assignments)?,
            None,
        ),
        PreferencesCommand::Reset {
            package,
            uid,
            expected_generation,
        } => (package, uid, 2, expected_generation, Vec::new(), None),
    };
    let r = c.preferences(&PreferencesRequest {
        package_id: package.clone(),
        uid,
        operation,
        expected_generation,
        config,
        names: Vec::new(),
    })?;
    if let Some(path) = output {
        write(&path, &r.config)?;
    }
    let values = if r.config.is_empty() {
        serde_json::Value::Null
    } else {
        let t = bexos_component_config::ConfigTable::parse(&r.config)
            .map_err(|e| format!("invalid preference table: {e:?}"))?;
        let mut values = serde_json::Map::new();
        for (name, ty) in t.entries() {
            use bexos_component_config::ConfigType;
            let value = match ty {
                ConfigType::Bool => json!(t.get_bool(name).unwrap()),
                ConfigType::Uint32 => json!(t.get_u32(name).unwrap()),
                ConfigType::Uint64 => json!(t.get_u64(name).unwrap()),
                ConfigType::String => json!(t.get_string(name).unwrap()),
                ConfigType::Bytes => json!(
                    t.get_bytes(name)
                        .unwrap()
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect::<String>()
                ),
            };
            values.insert(name.into(), value);
        }
        serde_json::Value::Object(values)
    };
    output::records(
        f,
        &[
            ("package", "PACKAGE"),
            ("uid", "UID"),
            ("generation", "GENERATION"),
            ("values", "VALUES"),
            ("locks", "LOCKS"),
            ("message", "RESULT"),
        ],
        vec![
            json!({"package":package,"uid":uid,"generation":r.generation,"values":values,"locks":r.locks,"message":r.message}),
        ],
    )?;
    Ok(())
}
pub(super) fn policy(
    c: &mut UnixDebugClient,
    package: String,
    expected_generation: u64,
    names: Vec<String>,
    operation: u32,
    f: Format,
) -> Result {
    let r = c.preferences(&PreferencesRequest {
        package_id: package.clone(),
        operation,
        expected_generation,
        names,
        ..Default::default()
    })?;
    output::records(
        f,
        &[
            ("package", "PACKAGE"),
            ("generation", "GENERATION"),
            ("locks", "LOCKS"),
            ("message", "RESULT"),
        ],
        vec![
            json!({"package":package,"generation":r.generation,"locks":r.locks,"message":r.message}),
        ],
    )?;
    Ok(())
}
