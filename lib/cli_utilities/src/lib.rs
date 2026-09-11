//! Reusable stream-oriented command implementations for storage-installed tools.
mod cat;
mod grep;
mod ls;
mod mkdir;
pub mod process;
use std::io::{Read, Write};
pub fn run(
    name: &str,
    args: &[String],
    input: &mut dyn Read,
    output: &mut dyn Write,
    error: &mut dyn Write,
) -> i32 {
    let result = match name {
        "cat" => cat::run(args, input, output),
        "grep" => grep::run(args, input, output),
        "ls" => ls::run(args, output),
        "mkdir" => mkdir::run(args),
        _ => {
            let _ = writeln!(error, "{name}: unknown utility");
            return 127;
        }
    };
    match result {
        Ok(status) => status,
        Err(e) => {
            let _ = writeln!(error, "{name}: {e}");
            if name == "grep" { 2 } else { 1 }
        }
    }
}
pub(crate) fn invalid(message: impl Into<String>) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidInput, message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn utilities_share_filesystem_and_stream_semantics() {
        let root = std::env::var("TEST_TMPDIR").unwrap();
        let root = std::path::Path::new(&root).join("cli-utilities");
        let directory = root.join("nested");
        let mut out = Vec::new();
        let mut error = Vec::new();
        assert_eq!(
            run(
                "mkdir",
                &["-p".into(), directory.display().to_string()],
                &mut io::empty(),
                &mut out,
                &mut error
            ),
            0
        );
        let file = directory.join("sample");
        std::fs::write(&file, b"alpha\nbeta\n").unwrap();
        assert_eq!(
            run(
                "cat",
                &[file.display().to_string()],
                &mut io::empty(),
                &mut out,
                &mut error
            ),
            0
        );
        assert_eq!(out, b"alpha\nbeta\n");
        out.clear();
        assert_eq!(
            run(
                "ls",
                &[directory.display().to_string()],
                &mut io::empty(),
                &mut out,
                &mut error
            ),
            0
        );
        assert_eq!(out, b"sample\n");
        out.clear();
        assert_eq!(
            run(
                "grep",
                &["^beta".into(), file.display().to_string()],
                &mut io::empty(),
                &mut out,
                &mut error
            ),
            0
        );
        assert_eq!(out, b"beta\n");
        assert!(error.is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }
    use std::io;
}
