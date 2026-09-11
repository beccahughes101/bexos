use std::{
    fs::File,
    io::{self, Read, Write},
};
pub(super) fn run(
    args: &[String],
    input: &mut dyn Read,
    output: &mut dyn Write,
) -> io::Result<i32> {
    if args.is_empty() {
        io::copy(input, output)?;
        return Ok(0);
    }
    let mut options = true;
    for path in args {
        if options && path == "--" {
            options = false;
            continue;
        }
        if options && path.starts_with('-') && path != "-" {
            return Err(crate::invalid(format!("unsupported option {path}")));
        }
        if path == "-" {
            io::copy(input, output)?;
        } else {
            io::copy(
                &mut File::open(path)
                    .map_err(|e| io::Error::new(e.kind(), format!("{path}: {e}")))?,
                output,
            )?;
        }
    }
    Ok(0)
}
