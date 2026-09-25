#![no_main]
mod client;
mod parse;
use bexos_userspace::{Channel, Memory, Socket, Startup};

const HELP: &str = "container - offline OCI lifecycle\n\n  create ID HOST REPOSITORY TAG [--digest SHA256] [--env NAME=value] [--cwd PATH] [--uid N] [--gid N] [--hostname NAME] [--cpu-shares N] [--memory BYTES] [--pids N] [--read-only] [--] COMMAND...\n  run    ID HOST REPOSITORY TAG [options] [--] COMMAND...\n  start ID\n  inspect ID\n  list\n  signal ID SIGNAL\n  delete [--force] ID\n";

fn write(socket: u64, text: &str) {
    let mut bytes = text.as_bytes();
    while !bytes.is_empty() {
        match Socket(socket).write(bytes) {
            Ok(0) | Err(_) => break,
            Ok(n) => bytes = &bytes[n as usize..],
        }
    }
}
fn hex(value: &[u8]) -> String {
    const H: &[u8; 16] = b"0123456789abcdef";
    value
        .iter()
        .flat_map(|b| [H[(b >> 4) as usize] as char, H[(b & 15) as usize] as char])
        .collect()
}
fn stdio(startup: &Startup) -> Result<[u64; 3], ()> {
    let mut out = [0; 3];
    for i in 0..3 {
        let source = match startup.resources.get(i).copied() {
            Some(source) => source,
            None => {
                close(&out);
                return Err(());
            }
        };
        let (_, rights) = match Memory::object_info(source) {
            Ok(info) => info,
            Err(_) => {
                close(&out);
                return Err(());
            }
        };
        out[i] = match Memory::duplicate(source, rights) {
            Ok(handle) => handle,
            Err(_) => {
                close(&out);
                return Err(());
            }
        };
    }
    Ok(out)
}
fn close(handles: &[u64]) {
    for handle in handles {
        if *handle != 0 {
            let _ = Memory::close(*handle);
        }
    }
}
fn run(channel: u64) -> i32 {
    let startup = match Startup::receive(Channel(channel)) {
        Ok(s) => s,
        Err(_) => return 125,
    };
    let err = startup.resources.get(2).copied().unwrap_or(0);
    let options = match bexos_userspace::command::from_startup(&startup) {
        Ok(Some(o)) => o,
        _ => {
            write(err, "container: invalid command startup\n");
            return 125;
        }
    };
    let grant = match startup
        .service_grants
        .iter()
        .find(|g| g.protocol == "ContainerManager")
    {
        Some(g) => g.endpoint,
        None => {
            write(err, "container: service unavailable\n");
            return 125;
        }
    };
    let command = match parse::parse(&options.arguments) {
        Ok(c) => c,
        Err(e) => {
            write(err, &format!("container: {e}\n"));
            return 2;
        }
    };
    let client = client::Client(Channel(grant));
    let result: Result<i32, container_fidl::ContainerStatus> = match command {
        parse::Command::Help => {
            write(startup.resources[1], HELP);
            Ok(0)
        }
        parse::Command::Create(c) => client.create(&c).map(|_| 0),
        parse::Command::Start(id) => start_wait(&client, &startup, &id),
        parse::Command::Run(c) => {
            let id = c.id.clone();
            client
                .create(&c)
                .and_then(|_| start_wait(&client, &startup, &id))
        }
        parse::Command::Signal(id, s) => client.signal(&id, s).map(|_| 0),
        parse::Command::Delete(id, f) => client.delete(&id, f).map(|_| 0),
        parse::Command::Inspect(id) => client.inspect(&id).map(|v| {
            show(startup.resources[1], &v);
            0
        }),
        parse::Command::List => client.list().map(|v| {
            show(startup.resources[1], &v);
            0
        }),
    };
    match result {
        Ok(code) => code,
        Err(status) => {
            write(err, &format!("container: {status:?}\n"));
            1
        }
    }
}
fn show(output: u64, values: &[client::OwnedInfo]) {
    for v in values {
        write(
            output,
            &format!(
                "{}\t{:?}\t{}\t{}\n",
                v.id,
                v.state,
                hex(&v.digest),
                v.exit_code
            ),
        );
    }
}
fn start_wait(
    client: &client::Client,
    startup: &Startup,
    id: &str,
) -> Result<i32, container_fidl::ContainerStatus> {
    let process = client.start(
        id,
        stdio(startup).map_err(|_| container_fidl::ContainerStatus::Unavailable)?,
    )?;
    loop {
        let (exited, code) = match client::process_status(process) {
            Ok(status) => status,
            Err(error) => {
                let _ = Memory::close(process);
                return Err(error);
            }
        };
        if exited {
            let _ = Memory::close(process);
            return Ok(code);
        }
        for _ in 0..128 {
            bexos_userspace::yield_now();
        }
    }
}

bexos_libc::entry!(entry);
fn entry(channel: u64) -> ! {
    let code = run(channel);
    bexos_userspace::syscall::exit_with_status(code)
}
