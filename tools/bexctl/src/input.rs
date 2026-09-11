const CONFIG_MAGIC: &[u8; 8] = b"BEXCFG\0\0";
pub(crate) fn encode_cli_config(assignments: &[String]) -> Result<Vec<u8>, String> {
    let mut entries = Vec::new();
    for assignment in assignments {
        entries.push(parse_config_assignment(assignment)?);
    }
    let mut out = Vec::new();
    out.extend_from_slice(CONFIG_MAGIC);
    out.extend_from_slice(&1u32.to_le_bytes());
    out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    for (name, config_type, value) in entries {
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.push(config_type);
        out.push(0);
        out.extend_from_slice(&(value.len() as u32).to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(&value);
    }
    Ok(out)
}

pub(crate) fn parse_config_assignment(assignment: &str) -> Result<(String, u8, Vec<u8>), String> {
    let (name, typed) = assignment
        .split_once('=')
        .ok_or_else(|| "config assignment must be NAME=TYPE:VALUE".to_string())?;
    let (kind, value) = typed
        .split_once(':')
        .ok_or_else(|| "config assignment must be NAME=TYPE:VALUE".to_string())?;
    if name.len() > u16::MAX as usize
        || name.is_empty()
        || !name.bytes().all(|b| b == b'_' || b.is_ascii_alphanumeric())
    {
        return Err(format!("invalid config field name {name}"));
    }
    let bytes = match kind {
        "bool" => vec![match value {
            "true" | "1" => 1,
            "false" | "0" => 0,
            _ => return Err(format!("invalid bool value for {name}")),
        }],
        "u32" => value
            .parse::<u32>()
            .map_err(|_| format!("invalid u32 value for {name}"))?
            .to_le_bytes()
            .to_vec(),
        "u64" => value
            .parse::<u64>()
            .map_err(|_| format!("invalid u64 value for {name}"))?
            .to_le_bytes()
            .to_vec(),
        "string" => value.as_bytes().to_vec(),
        "bytes" => parse_hex(value).map_err(|error| format!("{name}: {error}"))?,
        _ => return Err(format!("unsupported config type {kind}")),
    };
    let config_type = match kind {
        "bool" => 1,
        "u32" => 2,
        "u64" => 3,
        "string" => 4,
        "bytes" => 5,
        _ => unreachable!(),
    };
    Ok((name.to_string(), config_type, bytes))
}

pub(crate) fn parse_hex(value: &str) -> Result<Vec<u8>, String> {
    let hex = value.strip_prefix("hex").unwrap_or(value);
    if hex.len() % 2 != 0 {
        return Err("hex bytes require an even number of digits".into());
    }
    let mut out = Vec::new();
    for pair in hex.as_bytes().chunks(2) {
        let high = hex_digit(pair[0])?;
        let low = hex_digit(pair[1])?;
        out.push((high << 4) | low);
    }
    Ok(out)
}

pub(crate) fn hex_digit(byte: u8) -> Result<u8, String> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err("invalid hex digit".into()),
    }
}

pub(crate) fn parse_uuid(value: &str) -> Result<Vec<u8>, String> {
    let hex = value.chars().filter(|ch| *ch != '-').collect::<String>();
    if hex.len() != 32 || !hex.is_ascii() {
        return Err("UUID must contain 16 bytes".into());
    }
    let mut out = Vec::with_capacity(16);
    for index in (0..hex.len()).step_by(2) {
        out.push(
            u8::from_str_radix(&hex[index..index + 2], 16)
                .map_err(|_| "UUID contains non-hex digits".to_string())?,
        );
    }
    Ok(out)
}

pub(crate) fn format_uuid(bytes: &[u8]) -> Result<String, String> {
    if bytes.len() != 16 {
        return Err("UUID must contain 16 bytes".into());
    }
    Ok(format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    ))
}
