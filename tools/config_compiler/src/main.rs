use std::{env, fs};

use bexos_config_compiler::{Load, compile};

fn main() {
    if let Err(error) = run() {
        eprintln!("config_compiler: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    let source = required(&args, "--source")?;
    let entry = required(&args, "--entry")?;
    let output = required(&args, "--out")?;
    let source_text =
        fs::read_to_string(&source).map_err(|error| format!("read {source}: {error}"))?;
    let loads = repeated(&args, "--load")
        .into_iter()
        .map(|spec| {
            let (label, path) = spec
                .split_once('=')
                .ok_or_else(|| "--load requires LABEL=PATH".to_string())?;
            let source =
                fs::read_to_string(path).map_err(|error| format!("read {path}: {error}"))?;
            Ok(Load {
                label: label.to_owned(),
                path: path.to_owned(),
                source,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    fs::write(&output, compile(&source, &source_text, &entry, &loads)?)
        .map_err(|error| format!("write {output}: {error}"))
}

fn required(args: &[String], name: &str) -> Result<String, String> {
    args.windows(2)
        .find(|window| window[0] == name)
        .map(|window| window[1].clone())
        .ok_or_else(|| format!("missing {name}"))
}

fn repeated(args: &[String], name: &str) -> Vec<String> {
    args.windows(2)
        .filter(|window| window[0] == name)
        .map(|window| window[1].clone())
        .collect()
}
