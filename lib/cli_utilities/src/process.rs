//! Process tools share parsing and formatting; hosts provide identity-scoped control.
use std::io::{self, Write};
pub struct Process {
    pub id: u64,
    pub package: String,
    pub name: String,
    pub exited: bool,
    pub suspended: bool,
    pub status: i32,
}
pub trait ProcessManager {
    fn list(&mut self) -> io::Result<Vec<Process>>;
    fn signal(&mut self, id: u64, signal: u32) -> io::Result<()>;
}
pub fn run(
    name: &str,
    args: &[String],
    manager: &mut impl ProcessManager,
    out: &mut dyn Write,
) -> io::Result<i32> {
    match name {
        "ps" => {
            if !args.is_empty() {
                return Err(crate::invalid("ps takes no options"));
            }
            writeln!(out, "PID\tSTATE\tPROCESS\tPACKAGE")?;
            for p in manager.list()? {
                writeln!(
                    out,
                    "{}\t{}\t{}\t{}",
                    p.id,
                    if p.exited {
                        "exited"
                    } else if p.suspended {
                        "stopped"
                    } else {
                        "running"
                    },
                    p.name,
                    p.package
                )?;
            }
        }
        "kill" => {
            let mut signal = 15;
            let mut index = 0;
            if args.first().is_some_and(|a| a == "-s") {
                signal = parse_signal(
                    args.get(1)
                        .ok_or_else(|| crate::invalid("missing signal"))?,
                )?;
                index = 2;
            } else if let Some(arg) = args
                .first()
                .filter(|a| a.starts_with('-') && a.as_str() != "--")
            {
                signal = parse_signal(&arg[1..])?;
                index = 1;
            }
            if args.get(index).is_some_and(|a| a == "--") {
                index += 1;
            }
            let ids = args[index..]
                .iter()
                .map(|arg| {
                    arg.parse::<u64>()
                        .ok()
                        .filter(|id| *id != 0)
                        .ok_or_else(|| crate::invalid(format!("invalid process ID: {arg}")))
                })
                .collect::<io::Result<Vec<_>>>()?;
            if ids.is_empty() {
                return Err(crate::invalid("missing process ID"));
            }
            for id in ids {
                manager.signal(id, signal)?;
            }
        }
        _ => return Err(crate::invalid("unknown process utility")),
    }
    Ok(0)
}
fn parse_signal(value: &str) -> io::Result<u32> {
    match value.strip_prefix("SIG").unwrap_or(value) {
        "INT" | "2" => Ok(2),
        "KILL" | "9" => Ok(9),
        "TERM" | "15" => Ok(15),
        "CONT" | "18" => Ok(18),
        "STOP" | "19" => Ok(19),
        "TSTP" | "20" => Ok(20),
        _ => Err(crate::invalid(format!("unsupported signal: {value}"))),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default)]
    struct Manager(Vec<(u64, u32)>);
    impl ProcessManager for Manager {
        fn list(&mut self) -> io::Result<Vec<Process>> {
            Ok(vec![Process {
                id: 42,
                package: "example.app".into(),
                name: "work".into(),
                exited: false,
                suspended: true,
                status: 0,
            }])
        }
        fn signal(&mut self, id: u64, signal: u32) -> io::Result<()> {
            self.0.push((id, signal));
            Ok(())
        }
    }
    #[test]
    fn parses_before_signalling_and_formats_stopped_processes() {
        let mut m = Manager::default();
        let mut out = Vec::new();
        assert!(run("kill", &["42".into(), "bad".into()], &mut m, &mut out).is_err());
        assert!(m.0.is_empty());
        run(
            "kill",
            &["-s".into(), "CONT".into(), "42".into()],
            &mut m,
            &mut out,
        )
        .unwrap();
        assert_eq!(m.0, [(42, 18)]);
        run("ps", &[], &mut m, &mut out).unwrap();
        assert!(
            String::from_utf8(out)
                .unwrap()
                .contains("42\tstopped\twork\texample.app")
        );
    }
}
