use std::env;
use std::fs;
use std::path::PathBuf;

use bexos_app_archive::{BuildEntry, Compression, OpenArchive, TrustedKey, build_archive};

fn main() {
    if let Err(error) = run() {
        eprintln!("bex_archive: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    match args.first().map(String::as_str) {
        Some("create") => create(&args[1..]),
        Some("verify") | Some("inspect") => verify(&args[1..]),
        _ => Err("usage: bex_archive create|verify ...".to_string()),
    }
}

fn create(args: &[String]) -> Result<(), String> {
    let out = required(args, "--out")?;
    let manifest = required(args, "--manifest")?;
    let key = parse_private_key(
        &fs::read_to_string(required(args, "--key")?)
            .map_err(|error| format!("read key: {error}"))?,
    )?;
    let compression = match optional(args, "--compression").as_deref() {
        None | Some("none") => Compression::None,
        Some("zstd") => Compression::Zstd,
        Some(other) => return Err(format!("unsupported compression {other}")),
    };

    let mut owned = Vec::<(String, Vec<u8>, u32)>::new();
    owned.push((
        "package.bexmanifest".to_string(),
        fs::read(&manifest).map_err(|error| format!("read manifest {manifest}: {error}"))?,
        0o444,
    ));
    for entry in repeated(args, "--entry") {
        let (path, source) = entry
            .split_once('=')
            .ok_or_else(|| "--entry requires archive/path=source".to_string())?;
        let bytes = fs::read(source).map_err(|error| format!("read {source}: {error}"))?;
        owned.push((path.to_string(), bytes, mode_for(path)));
    }
    let architecture = bexos_app_manifest::ManifestArchitecture::decode(&owned[0].1)
        .and_then(|m| m.validate(None)).map_err(|e| format!("manifest architecture: {e:?}; rebuild native packages with an explicit architecture"))?;
    for (path, bytes, _) in &owned[1..] {
        bexos_app_manifest::validate_payload(architecture, bytes)
            .map_err(|e| format!("payload {path}: {e:?}"))?;
    }
    let borrowed = owned
        .iter()
        .map(|(path, bytes, mode)| BuildEntry {
            path,
            bytes,
            mode: *mode,
        })
        .collect::<Vec<_>>();
    let built = build_archive(&borrowed, compression, key.key_id, key.seed)
        .map_err(|error| format!("build archive: {error:?}"))?;
    fs::write(PathBuf::from(out), built.bytes).map_err(|error| format!("write archive: {error}"))
}

fn verify(args: &[String]) -> Result<(), String> {
    let archive_path = required(args, "--archive")?;
    let public = parse_public_key(
        &fs::read_to_string(required(args, "--public-key")?)
            .map_err(|error| format!("read public key: {error}"))?,
    )?;
    let bytes = fs::read(&archive_path).map_err(|error| format!("read archive: {error}"))?;
    let trusted = [TrustedKey {
        key_id: public.key_id,
        public_key: &public.public_key,
    }];
    let archive = OpenArchive::parse_and_verify(&bytes, &trusted)
        .map_err(|error| format!("verify archive: {error:?}"))?;
    println!(
        "bex_archive: verified {} entries content_root={}",
        archive.entries().len(),
        hex(&archive.content_root())
    );
    for entry in archive.entries() {
        println!(
            "{} size={} stored={} compression={:?}",
            entry.path, entry.uncompressed_size, entry.stored_len, entry.compression
        );
    }
    Ok(())
}

fn mode_for(path: &str) -> u32 {
    if path.starts_with("bin/") {
        0o555
    } else {
        0o444
    }
}

struct PrivateKey {
    key_id: [u8; 32],
    seed: [u8; 32],
}

struct PublicKey {
    key_id: [u8; 32],
    public_key: [u8; 32],
}

fn parse_private_key(text: &str) -> Result<PrivateKey, String> {
    Ok(PrivateKey {
        key_id: parse_hex_field(text, "key_id_hex")?,
        seed: parse_hex_field(text, "seed_hex")?,
    })
}

fn parse_public_key(text: &str) -> Result<PublicKey, String> {
    Ok(PublicKey {
        key_id: parse_hex_field(text, "key_id_hex")?,
        public_key: parse_hex_field(text, "public_key_hex")?,
    })
}

fn parse_hex_field<const N: usize>(text: &str, name: &str) -> Result<[u8; N], String> {
    let value = text
        .lines()
        .find_map(|line| line.trim().strip_prefix(&format!("{name}=")))
        .ok_or_else(|| format!("missing {name}"))?;
    if value.len() != N * 2 {
        return Err(format!("{name} must be {} hex chars", N * 2));
    }
    let mut out = [0; N];
    for index in 0..N {
        out[index] = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)
            .map_err(|_| format!("invalid {name}"))?;
    }
    Ok(out)
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::new();
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn required(args: &[String], name: &str) -> Result<String, String> {
    optional(args, name).ok_or_else(|| format!("missing {name}"))
}

fn optional(args: &[String], name: &str) -> Option<String> {
    args.windows(2)
        .find(|window| window[0] == name)
        .map(|window| window[1].clone())
}

fn repeated(args: &[String], name: &str) -> Vec<String> {
    args.windows(2)
        .filter(|window| window[0] == name)
        .map(|window| window[1].clone())
        .collect()
}
