use bexos_trust_store::persistent::TrustStoreDb;
use bexos_trust_store::{
    AppSigningRootAnchor, TlsRootAnchor, TrustTier, root_id, validate_app_anchor,
};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Kind {
    Tls,
    App,
}

pub fn run(args: impl IntoIterator<Item = String>) -> Result<(), String> {
    let config = Config::parse(args)?;
    if config.out.exists() {
        fs::remove_file(&config.out)
            .map_err(|err| format!("failed to replace {}: {err}", config.out.display()))?;
    }
    match config.kind {
        Kind::Tls => {
            let roots = load_tls_roots(&config.input_dir)?;
            TrustStoreDb::create_tls(&config.out, &roots)
                .map_err(|err| format!("failed to write TLS root store: {err:?}"))?;
        }
        Kind::App => {
            let roots = load_app_roots(&config.input_dir)?;
            TrustStoreDb::create_app(&config.out, &roots)
                .map_err(|err| format!("failed to write app root store: {err:?}"))?;
        }
    }
    Ok(())
}

struct Config {
    kind: Kind,
    input_dir: PathBuf,
    out: PathBuf,
}

impl Config {
    fn parse(args: impl IntoIterator<Item = String>) -> Result<Self, String> {
        let mut kind = None;
        let mut input_dir = None;
        let mut out = None;
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--kind" => {
                    let value = args.next().ok_or_else(|| usage("missing --kind value"))?;
                    kind = Some(match value.as_str() {
                        "tls" => Kind::Tls,
                        "app" => Kind::App,
                        _ => return Err(usage("kind must be tls or app")),
                    });
                }
                "--input-dir" => {
                    input_dir = Some(PathBuf::from(
                        args.next()
                            .ok_or_else(|| usage("missing --input-dir value"))?,
                    ));
                }
                "--out" => {
                    out = Some(PathBuf::from(
                        args.next().ok_or_else(|| usage("missing --out value"))?,
                    ));
                }
                _ => return Err(usage(&format!("unexpected argument {arg}"))),
            }
        }
        Ok(Self {
            kind: kind.ok_or_else(|| usage("missing --kind"))?,
            input_dir: input_dir.ok_or_else(|| usage("missing --input-dir"))?,
            out: out.ok_or_else(|| usage("missing --out"))?,
        })
    }
}

fn usage(reason: &str) -> String {
    format!("{reason}\nusage: trust_roots --kind tls|app --input-dir DIR --out FILE.redb")
}

fn load_tls_roots(dir: &Path) -> Result<Vec<TlsRootAnchor>, String> {
    let mut roots = Vec::new();
    for path in cert_files(dir)? {
        if path.extension().and_then(|e| e.to_str()) == Some("prototxt") {
            return Err(format!(
                "TLS bundle contains app metadata: {}",
                path.display()
            ));
        }
        let der = canonical_cert_bytes(&path)?;
        roots.push(TlsRootAnchor {
            root_id: root_id(&der).map_err(|err| format!("{}: {err:?}", path.display()))?,
            source_path: path.display().to_string(),
            der_bytes: der,
        });
    }
    if roots.is_empty() {
        return Err(format!("no TLS roots found in {}", dir.display()));
    }
    roots.sort_by(|a, b| a.root_id.cmp(&b.root_id));
    Ok(roots)
}

fn load_app_roots(dir: &Path) -> Result<Vec<AppSigningRootAnchor>, String> {
    let mut roots = Vec::new();
    let mut seen = BTreeSet::new();
    for path in cert_files(dir)? {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if matches!(ext, "pem" | "crt" | "der") {
            return Err(format!(
                "app bundle roots must be declared by prototxt metadata, found {}",
                path.display()
            ));
        }
        let anchor = parse_app_anchor(
            &fs::read_to_string(&path)
                .map_err(|err| format!("failed to read {}: {err}", path.display()))?,
        )?;
        validate_app_anchor(&anchor).map_err(|err| format!("{}: {err:?}", path.display()))?;
        if !seen.insert(anchor.anchor_id.clone()) {
            return Err(format!("duplicate app anchor_id {}", anchor.anchor_id));
        }
        roots.push(anchor);
    }
    if roots.is_empty() {
        return Err(format!("no app roots found in {}", dir.display()));
    }
    roots.sort_by(|a, b| a.anchor_id.cmp(&b.anchor_id));
    Ok(roots)
}

fn cert_files(dir: &Path) -> Result<Vec<PathBuf>, String> {
    if !dir.is_dir() {
        return Err(format!("{} is not a directory", dir.display()));
    }
    let mut out = Vec::new();
    walk(dir, &mut out)?;
    out.sort();
    Ok(out)
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    for entry in
        fs::read_dir(dir).map_err(|err| format!("failed to read {}: {err}", dir.display()))?
    {
        let path = entry
            .map_err(|err| format!("failed to read {} entry: {err}", dir.display()))?
            .path();
        if path.is_dir() {
            walk(&path, out)?;
            continue;
        }
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        match ext {
            "pem" | "crt" | "der" | "prototxt" => out.push(path),
            _ => {
                return Err(format!(
                    "unsupported root file extension: {}",
                    path.display()
                ));
            }
        }
    }
    Ok(())
}

fn canonical_cert_bytes(path: &Path) -> Result<Vec<u8>, String> {
    let bytes =
        fs::read(path).map_err(|err| format!("failed to read {}: {err}", path.display()))?;
    match path.extension().and_then(|e| e.to_str()) {
        Some("der") => Ok(bytes),
        Some("pem") | Some("crt") => pem_body(&bytes)
            .ok_or_else(|| format!("failed to parse PEM certificate {}", path.display())),
        _ => Err(format!("unsupported certificate file {}", path.display())),
    }
}

fn pem_body(bytes: &[u8]) -> Option<Vec<u8>> {
    let text = std::str::from_utf8(bytes).ok()?;
    let mut in_cert = false;
    let mut body = String::new();
    for line in text.lines() {
        if line == "-----BEGIN CERTIFICATE-----" {
            in_cert = true;
            continue;
        }
        if line == "-----END CERTIFICATE-----" {
            return decode_base64(&body);
        }
        if in_cert {
            body.push_str(line.trim());
        }
    }
    None
}

fn parse_app_anchor(text: &str) -> Result<AppSigningRootAnchor, String> {
    let mut anchor = AppSigningRootAnchor {
        anchor_id: String::new(),
        tier: TrustTier::Tier4WebOrigin,
        algorithm: String::new(),
        public_key_bytes: Vec::new(),
        certificate_der: Vec::new(),
        permitted_package_prefixes: Vec::new(),
        valid_from: 0,
        valid_until: 0,
        is_hardware_anchored: false,
        immutable: false,
        enterprise: false,
    };
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            return Err(format!("invalid prototxt line: {line}"));
        };
        let key = key.trim();
        let value = value.trim().trim_matches('"');
        match key {
            "anchor_id" => anchor.anchor_id = value.to_string(),
            "tier" => {
                anchor.tier = match value {
                    "TIER0_BASE_SYSTEM" => TrustTier::Tier0BaseSystem,
                    "TIER1_PLATFORM_APP" => TrustTier::Tier1PlatformApp,
                    "TIER2_VERIFIED_ECO" => TrustTier::Tier2VerifiedEco,
                    "TIER3_ENTERPRISE" => TrustTier::Tier3Enterprise,
                    "TIER4_WEB_ORIGIN" => TrustTier::Tier4WebOrigin,
                    _ => return Err(format!("unknown trust tier {value}")),
                };
            }
            "algorithm" => anchor.algorithm = value.to_string(),
            "public_key_hex" => anchor.public_key_bytes = decode_hex(value)?,
            "certificate_der_hex" => anchor.certificate_der = decode_hex(value)?,
            "certificate_der_file" => {
                anchor.certificate_der =
                    fs::read(value).map_err(|err| format!("failed to read {value}: {err}"))?
            }
            "permitted_package_prefixes" => {
                anchor.permitted_package_prefixes.push(value.to_string())
            }
            "valid_from" => {
                anchor.valid_from = value
                    .parse()
                    .map_err(|_| format!("invalid valid_from {value}"))?
            }
            "valid_until" => {
                anchor.valid_until = value
                    .parse()
                    .map_err(|_| format!("invalid valid_until {value}"))?
            }
            "is_hardware_anchored" => {
                anchor.is_hardware_anchored = match value {
                    "true" => true,
                    "false" => false,
                    _ => return Err(format!("invalid bool {value}")),
                }
            }
            "immutable" => {
                anchor.immutable = match value {
                    "true" => true,
                    "false" => false,
                    _ => return Err(format!("invalid bool {value}")),
                }
            }
            "enterprise" => {
                anchor.enterprise = match value {
                    "true" => true,
                    "false" => false,
                    _ => return Err(format!("invalid bool {value}")),
                }
            }
            _ => return Err(format!("unknown app root field {key}")),
        }
    }
    Ok(anchor)
}

fn decode_hex(value: &str) -> Result<Vec<u8>, String> {
    if !value.len().is_multiple_of(2) {
        return Err("hex value has odd length".into());
    }
    let mut out = Vec::with_capacity(value.len() / 2);
    for i in (0..value.len()).step_by(2) {
        out.push(
            u8::from_str_radix(&value[i..i + 2], 16)
                .map_err(|_| format!("invalid hex byte {}", &value[i..i + 2]))?,
        );
    }
    Ok(out)
}

fn decode_base64(value: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut acc = 0u32;
    let mut bits = 0u8;
    for byte in value.bytes() {
        let val = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            b'\r' | b'\n' | b'\t' | b' ' => continue,
            _ => return None,
        } as u32;
        acc = (acc << 6) | val;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::{decode_hex, pem_body};

    #[test]
    fn decodes_hex() {
        assert_eq!(decode_hex("00ff10").unwrap(), vec![0, 255, 16]);
    }

    #[test]
    fn decodes_pem_body() {
        let pem = b"-----BEGIN CERTIFICATE-----\nAQIDBA==\n-----END CERTIFICATE-----\n";
        assert_eq!(pem_body(pem).unwrap(), vec![1, 2, 3, 4]);
    }
}
