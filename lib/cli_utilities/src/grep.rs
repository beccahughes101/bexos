use regex::bytes::RegexBuilder;
use std::{
    fs::File,
    io::{self, BufRead, BufReader, Read, Write},
};
pub(super) fn run(args: &[String], input: &mut dyn Read, out: &mut dyn Write) -> io::Result<i32> {
    let (mut insensitive, mut invert, mut number, mut fixed) = (false, false, false, false);
    let mut operands = Vec::new();
    let mut options = true;
    for arg in args {
        if options && arg == "--" {
            options = false;
            continue;
        }
        if options && arg.starts_with('-') && arg != "-" {
            for flag in arg[1..].chars() {
                match flag {
                    'i' => insensitive = true,
                    'v' => invert = true,
                    'n' => number = true,
                    'F' => fixed = true,
                    'E' => (),
                    _ => return Err(crate::invalid(format!("unsupported option -{flag}"))),
                };
            }
            continue;
        }
        operands.push(arg);
    }
    let pattern = operands
        .first()
        .ok_or_else(|| crate::invalid("missing pattern"))?;
    let pattern = if fixed {
        regex::escape(pattern)
    } else {
        (*pattern).clone()
    };
    let matcher = RegexBuilder::new(&pattern)
        .case_insensitive(insensitive)
        .size_limit(1 << 20)
        .build()
        .map_err(|e| crate::invalid(e.to_string()))?;
    let mut matched = false;
    let mut scan = |reader: &mut dyn Read, name: Option<&str>| -> io::Result<()> {
        let mut reader = BufReader::new(reader);
        let mut line = Vec::new();
        let mut index = 0u64;
        loop {
            line.clear();
            if reader
                .by_ref()
                .take((1 << 20) + 1)
                .read_until(b'\n', &mut line)?
                == 0
            {
                break;
            }
            if line.len() > 1 << 20 {
                return Err(crate::invalid("line exceeds 1 MiB limit"));
            }
            index += 1;
            let bytes = line.strip_suffix(b"\n").unwrap_or(&line);
            if matcher.is_match(bytes) != invert {
                matched = true;
                if let Some(name) = name {
                    write!(out, "{name}:")?;
                }
                if number {
                    write!(out, "{index}:")?;
                }
                out.write_all(&line)?;
                if !line.ends_with(b"\n") {
                    out.write_all(b"\n")?;
                }
            }
        }
        Ok(())
    };
    if operands.len() == 1 {
        scan(input, None)?;
    } else {
        for path in &operands[1..] {
            let label = (operands.len() > 2).then_some(path.as_str());
            if path.as_str() == "-" {
                scan(input, label)?;
            } else {
                scan(
                    &mut File::open(path)
                        .map_err(|e| io::Error::new(e.kind(), format!("{path}: {e}")))?,
                    label,
                )?;
            }
        }
    }
    Ok(if matched { 0 } else { 1 })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn matching_uses_bytes_and_conventional_statuses() {
        let mut out = Vec::new();
        assert_eq!(
            run(
                &["-in".into(), "^a".into()],
                &mut &b"Apple\npear\napricot\n"[..],
                &mut out
            )
            .unwrap(),
            0
        );
        assert_eq!(out, b"1:Apple\n3:apricot\n");
        assert_eq!(
            run(&["nomatch".into()], &mut &b"abc\n"[..], &mut Vec::new()).unwrap(),
            1
        );
        assert!(run(&["[".into()], &mut &b""[..], &mut Vec::new()).is_err());
    }
}
