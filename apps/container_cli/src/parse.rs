#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Create {
    pub id: String,
    pub host: String,
    pub repository: String,
    pub tag: String,
    pub digest: Vec<u8>,
    pub arguments: Vec<String>,
    pub environment: Vec<String>,
    pub cwd: String,
    pub uid: u32,
    pub gid: u32,
    pub hostname: String,
    pub cpu_shares: u32,
    pub memory: u64,
    pub pids: u32,
    pub readonly: bool,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Command {
    Create(Create),
    Start(String),
    Run(Create),
    Inspect(String),
    List,
    Signal(String, u32),
    Delete(String, bool),
    Help,
}

fn number<T: core::str::FromStr>(value: Option<&String>) -> Result<T, &'static str> {
    value
        .ok_or("missing option value")?
        .parse()
        .map_err(|_| "invalid number")
}
fn digest(value: &str) -> Result<Vec<u8>, &'static str> {
    let value = value.strip_prefix("sha256:").unwrap_or(value);
    if value.len() != 64 {
        return Err("digest must be 64 hex characters");
    }
    (0..32)
        .map(|i| u8::from_str_radix(&value[i * 2..i * 2 + 2], 16).map_err(|_| "invalid digest"))
        .collect()
}
fn create(args: &[String]) -> Result<Create, &'static str> {
    if args.len() < 4 {
        return Err("create/run requires ID HOST REPOSITORY TAG");
    }
    let mut out = Create {
        id: args[0].clone(),
        host: args[1].clone(),
        repository: args[2].clone(),
        tag: args[3].clone(),
        cwd: String::new(),
        ..Create::default()
    };
    let mut at = 4;
    while at < args.len() {
        match args[at].as_str() {
            "--" => {
                out.arguments.extend_from_slice(&args[at + 1..]);
                break;
            }
            "--digest" => {
                out.digest = digest(args.get(at + 1).ok_or("missing digest")?)?;
                at += 2;
            }
            "--env" => {
                let value = args.get(at + 1).ok_or("missing environment")?;
                if value.split_once('=').is_none() {
                    return Err("environment must be NAME=value");
                }
                out.environment.push(value.clone());
                at += 2;
            }
            "--cwd" => {
                out.cwd = args.get(at + 1).ok_or("missing cwd")?.clone();
                at += 2;
            }
            "--uid" => {
                out.uid = number(args.get(at + 1))?;
                at += 2;
            }
            "--gid" => {
                out.gid = number(args.get(at + 1))?;
                at += 2;
            }
            "--hostname" => {
                out.hostname = args.get(at + 1).ok_or("missing hostname")?.clone();
                at += 2;
            }
            "--cpu-shares" => {
                out.cpu_shares = number(args.get(at + 1))?;
                at += 2;
            }
            "--memory" => {
                out.memory = number(args.get(at + 1))?;
                at += 2;
            }
            "--pids" => {
                out.pids = number(args.get(at + 1))?;
                at += 2;
            }
            "--read-only" => {
                out.readonly = true;
                at += 1;
            }
            _ => return Err("unknown create option"),
        }
    }
    Ok(out)
}
pub fn parse(args: &[String]) -> Result<Command, &'static str> {
    match args.get(1).map(String::as_str) {
        None | Some("help" | "--help" | "-h") => Ok(Command::Help),
        Some("create") => create(&args[2..]).map(Command::Create),
        Some("run") => create(&args[2..]).map(Command::Run),
        Some("start") if args.len() == 3 => Ok(Command::Start(args[2].clone())),
        Some("inspect") if args.len() == 3 => Ok(Command::Inspect(args[2].clone())),
        Some("list") if args.len() == 2 => Ok(Command::List),
        Some("signal") if args.len() == 4 => Ok(Command::Signal(
            args[2].clone(),
            args[3].parse().map_err(|_| "invalid signal")?,
        )),
        Some("delete") if args.len() == 3 => Ok(Command::Delete(args[2].clone(), false)),
        Some("delete") if args.len() == 4 && args[2] == "--force" => {
            Ok(Command::Delete(args[3].clone(), true))
        }
        _ => Err("invalid command; use container help"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_bounded_run_options() {
        let a = [
            "container",
            "run",
            "id",
            "r.test",
            "a/b",
            "v1",
            "--read-only",
            "--memory",
            "4096",
            "--",
            "/bin/true",
        ]
        .map(String::from);
        let Command::Run(c) = parse(&a).unwrap() else {
            panic!()
        };
        assert!(c.readonly);
        assert_eq!(c.memory, 4096);
        assert_eq!(c.arguments, ["/bin/true"]);
    }
}
