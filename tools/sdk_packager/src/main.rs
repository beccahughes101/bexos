use bexos_sdk_packager::{Entry, archive, sha256_line};
use std::{env, fs, path::Path};

fn main() {
    if let Err(error) = run() {
        eprintln!("sdk_packager: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    match args.first().map(String::as_str) {
        Some("archive") => build_archive(&args[1..]),
        Some("sha256") => checksum(&args[1..]),
        _ => Err("usage: sdk_packager archive|sha256 ...".into()),
    }
}

fn build_archive(args: &[String]) -> Result<(), String> {
    let mut entries = Vec::new();
    for spec in repeated(args, "--file") {
        let (destination, source_mode) = spec
            .split_once('=')
            .ok_or_else(|| "--file requires destination=source[:executable]".to_string())?;
        let (source, executable) = source_mode
            .strip_suffix(":executable")
            .map_or((source_mode, false), |source| (source, true));
        entries.push(Entry {
            path: destination.to_string(),
            bytes: fs::read(source).map_err(|error| format!("read {source}: {error}"))?,
            executable,
        });
    }
    let bytes = archive(&required(args, "--root")?, &entries)?;
    fs::write(required(args, "--out")?, bytes).map_err(|error| error.to_string())
}

fn checksum(args: &[String]) -> Result<(), String> {
    let input = required(args, "--input")?;
    let bytes = fs::read(&input).map_err(|error| error.to_string())?;
    let name = Path::new(&input)
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "invalid input filename".to_string())?;
    fs::write(required(args, "--out")?, sha256_line(name, &bytes))
        .map_err(|error| error.to_string())
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
