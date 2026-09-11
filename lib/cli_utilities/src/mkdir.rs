use std::{fs, io};
pub(super) fn run(args: &[String]) -> io::Result<i32> {
    let mut parents = false;
    let mut paths = Vec::new();
    let mut options = true;
    for arg in args {
        if options && arg == "--" {
            options = false;
        } else if options && arg == "-p" {
            parents = true;
        } else if options && arg.starts_with('-') {
            return Err(crate::invalid(format!("unsupported option {arg}")));
        } else {
            paths.push(arg);
        }
    }
    if paths.is_empty() {
        return Err(crate::invalid("missing directory operand"));
    }
    for path in paths {
        if parents {
            fs::create_dir_all(path)
        } else {
            fs::create_dir(path)
        }
        .map_err(|e| io::Error::new(e.kind(), format!("{path}: {e}")))?;
    }
    Ok(0)
}
