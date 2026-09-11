use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    if let Err(error) = run(env::args().skip(1).collect()) {
        eprintln!("ecosystem_bundle: {error}");
        std::process::exit(1);
    }
}

fn run(args: Vec<String>) -> Result<(), String> {
    let profile = required(&args, "--profile")?;
    let deployable = bool_arg(required(&args, "--deployable")?)?;
    let policy_path = required(&args, "--policy")?;
    let direct_keys_path = required(&args, "--direct-update-keys")?;
    let tuf_root_path = required(&args, "--tuf-root")?;
    let tls_roots_path = required(&args, "--tls-roots")?;
    let app_roots_path = required(&args, "--app-roots")?;
    let out = required(&args, "--out")?;

    let policy = read_text(&policy_path)?;
    validate_policy(&profile, deployable, &policy)?;
    let direct_keys = read_text(&direct_keys_path)?;
    validate_direct_keys(&profile, deployable, &direct_keys)?;
    let tuf_root = read_text(&tuf_root_path)?;
    validate_tuf_root(&tuf_root)?;
    let tls_roots = fs::read(&tls_roots_path).map_err(|e| format!("read TLS roots: {e}"))?;
    let app_roots = fs::read(&app_roots_path).map_err(|e| format!("read app roots: {e}"))?;
    if tls_roots.is_empty() || app_roots.is_empty() {
        return Err("root stores must not be empty".into());
    }
    if tls_roots.len() > 8 * 1024 * 1024 || app_roots.len() > 8 * 1024 * 1024 {
        return Err("root stores exceed ecosystem bundle size limits".into());
    }

    let mut bundle = Vec::new();
    put_section(&mut bundle, b"BEXECO2");
    put_section(&mut bundle, profile.as_bytes());
    put_section(
        &mut bundle,
        if deployable {
            b"deployable"
        } else {
            b"template"
        },
    );
    put_section(&mut bundle, policy.as_bytes());
    put_section(&mut bundle, direct_keys.as_bytes());
    put_section(&mut bundle, tuf_root.as_bytes());
    put_section(&mut bundle, &tls_roots);
    put_section(&mut bundle, &app_roots);
    fs::write(PathBuf::from(out), bundle).map_err(|e| format!("write bundle: {e}"))
}

fn validate_policy(profile: &str, deployable: bool, policy: &str) -> Result<(), String> {
    if !contains_field(policy, "profile", profile) {
        return Err("policy profile identity does not match target profile".into());
    }
    if deployable && contains_field(policy, "deployable", "false") {
        return Err("deployable target uses non-deployable policy".into());
    }
    if profile == "prod" && deployable && policy.contains("template_only") {
        return Err("deployable prod profile cannot be a template".into());
    }
    for key in [
        "metadata_base_url",
        "targets_base_url",
        "allowed_tuf_origins",
    ] {
        for value in string_fields(policy, key) {
            if !value.starts_with("https://") || value.contains('@') || value.contains('#') {
                return Err(format!("{key} must be a credential-free HTTPS URL"));
            }
        }
    }
    if !policy.contains("revocation_target") || !policy.contains("minimum_trusted_unix_time") {
        return Err("policy must configure revocation target and trusted-time floor".into());
    }
    Ok(())
}

fn validate_direct_keys(profile: &str, deployable: bool, text: &str) -> Result<(), String> {
    if profile == "prod" && !deployable {
        return if text.contains("template_only") {
            Ok(())
        } else {
            Err("prod template keys must be marked template_only".into())
        };
    }
    if !text.contains("key_id") || !text.contains("public_key_hex") {
        return Err("direct update keys must include key identifiers and public keys".into());
    }
    Ok(())
}

fn validate_tuf_root(text: &str) -> Result<(), String> {
    if !(text.contains("\"_type\"") && text.contains("\"root\"") && text.contains("\"version\"")) {
        return Err("TUF root metadata must look like root.json".into());
    }
    if text.len() > 1024 * 1024 {
        return Err("TUF root metadata exceeds size limit".into());
    }
    Ok(())
}

fn required(args: &[String], name: &str) -> Result<String, String> {
    args.windows(2)
        .find(|window| window[0] == name)
        .map(|window| window[1].clone())
        .ok_or_else(|| format!("missing {name}"))
}

fn bool_arg(value: String) -> Result<bool, String> {
    match value.as_str() {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(format!("invalid bool {value}")),
    }
}

fn read_text(path: &str) -> Result<String, String> {
    fs::read_to_string(path).map_err(|e| format!("read {path}: {e}"))
}

fn contains_field(text: &str, field: &str, expected: &str) -> bool {
    string_fields(text, field)
        .iter()
        .any(|value| value == expected)
}

fn string_fields(text: &str, field: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| line.trim().strip_prefix(&format!("{field}:")))
        .map(|value| value.trim().trim_matches('"').to_string())
        .collect()
}

fn put_section(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
    out.extend_from_slice(bytes);
}
