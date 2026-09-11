use std::{
    fs,
    io::{self, Write},
    path::Path,
};
pub(super) fn run(args: &[String], out: &mut dyn Write) -> io::Result<i32> {
    let mut all = false;
    let mut long = false;
    let mut paths = Vec::new();
    let mut options = true;
    for arg in args {
        if options && arg == "--" {
            options = false;
            continue;
        }
        if options && arg.starts_with('-') {
            for flag in arg[1..].chars() {
                match flag {
                    'a' => all = true,
                    'l' => long = true,
                    '1' => (),
                    _ => return Err(crate::invalid(format!("unsupported option -{flag}"))),
                };
            }
            continue;
        }
        paths.push(arg.as_str());
    }
    if paths.is_empty() {
        paths.push(".");
    }
    for (i, path) in paths.iter().enumerate() {
        let metadata =
            fs::metadata(path).map_err(|e| io::Error::new(e.kind(), format!("{path}: {e}")))?;
        if !metadata.is_dir() {
            write_entry(out, path, &metadata, long)?;
            continue;
        }
        if paths.len() > 1 {
            if i > 0 {
                writeln!(out)?;
            }
            writeln!(out, "{path}:")?;
        }
        let mut names = fs::read_dir(path)?
            .map(|e| e.map(|e| e.file_name()))
            .collect::<io::Result<Vec<_>>>()?;
        names.sort();
        for name in names {
            let text = name.to_string_lossy();
            if !all && text.starts_with('.') {
                continue;
            }
            let metadata = fs::symlink_metadata(Path::new(path).join(&name))?;
            write_entry(out, &text, &metadata, long)?;
        }
    }
    Ok(0)
}
fn write_entry(
    out: &mut dyn Write,
    name: &str,
    metadata: &fs::Metadata,
    long: bool,
) -> io::Result<()> {
    if long {
        writeln!(
            out,
            "{} {:>10} {name}",
            if metadata.is_dir() {
                'd'
            } else if metadata.file_type().is_symlink() {
                'l'
            } else {
                '-'
            },
            metadata.len()
        )
    } else {
        writeln!(out, "{name}")
    }
}
