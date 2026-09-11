use bexos_app_manifest::{Architecture, ManifestArchitecture, stamp, validate_payload};
use std::{
    env,
    io::{Read, Write},
};
fn main() {
    if let Err(error) = run() {
        eprintln!("manifest architecture: {error}");
        std::process::exit(1);
    }
}
fn run() -> Result<(), String> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    let validating = args.first().is_some_and(|s| s == "validate");
    let index = usize::from(validating);
    let arch = match args.get(index).map(String::as_str) {
        Some("AARCH64") => Architecture::Aarch64,
        Some("X86_64") => Architecture::X86_64,
        Some("MULTI") => Architecture::Multi,
        _ => return Err("expected AARCH64, X86_64, or MULTI".into()),
    };
    if validating {
        let path = args.get(2).ok_or("validate requires a manifest")?;
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        let declared = ManifestArchitecture::decode(&bytes)
            .and_then(|m| m.validate(Some(arch)))
            .map_err(|e| format!("{path}: {e:?}; rebuild/reinstall native packages"))?;
        for path in &args[3..] {
            let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
            validate_payload(declared, &bytes).map_err(|e| format!("{path}: {e:?}"))?;
        }
        return Ok(());
    }
    let mut input = Vec::new();
    std::io::stdin()
        .read_to_end(&mut input)
        .map_err(|e| e.to_string())?;
    let output = stamp(&input, arch).map_err(|e| format!("{e:?}"))?;
    std::io::stdout()
        .write_all(&output)
        .map_err(|e| e.to_string())
}
