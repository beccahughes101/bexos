#[path = "../src/args.rs"]
mod args;
#[path = "../src/input.rs"]
mod input;
#[path = "../src/output.rs"]
mod output;
mod shell_dispatch;
use clap::{CommandFactory, Parser};
use serde_json::json;
#[test]
fn grammar_is_consistent() {
    args::Cli::command().debug_assert();
}
#[test]
fn offline_help_at_every_level() {
    fn visit(mut c: clap::Command, path: Vec<String>) {
        let mut argv = path.clone();
        argv.push("--help".into());
        let e = args::Cli::try_parse_from(argv).err().unwrap();
        assert_eq!(e.kind(), clap::error::ErrorKind::DisplayHelp);
        for s in c.get_subcommands_mut() {
            if s.get_name() == "help" {
                continue;
            }
            let mut p = path.clone();
            p.push(s.get_name().into());
            visit(s.clone(), p);
        }
    }
    visit(args::Cli::command(), vec!["bexctl".into()]);
}
#[test]
fn preserves_existing_invocations() {
    for argv in [
        vec!["shell", "--system"],
        vec!["shell", "--user", "alice"],
        vec!["shell", "--user", "alice", "--password-stdin"],
        vec!["shell", "--uid", "1000", "--password", "secret"],
        vec!["--socket", "/tmp/debug.sock", "ps"],
        vec![
            "--socket",
            "/tmp/debug.sock",
            "launch",
            "a",
            "b",
            "42",
            "--uid",
            "1000",
        ],
        vec![
            "--socket",
            "s",
            "trace",
            "record",
            "--format",
            "fxt",
            "-o",
            "trace.fxt",
        ],
        vec!["--socket", "s", "config", "get", "a", "--out", "a.bin"],
        vec!["exec", "debugd.version", "--", "--argument"],
        vec![
            "users",
            "update",
            "--uid",
            "1",
            "--name",
            "alice",
            "--current-password",
            "old",
            "--new-password",
            "new",
        ],
    ] {
        let mut all = vec!["bexctl"];
        all.extend(argv);
        assert!(args::Cli::try_parse_from(all.clone()).is_ok(), "{all:?}");
    }
}
#[test]
fn rejects_invalid_input_without_connection() {
    for argv in [
        vec!["shell"],
        vec!["shell", "--password", "secret"],
        vec!["shell", "--password-stdin"],
        vec!["shell", "--user", "alice", "--uid", "1000"],
        vec![
            "shell",
            "--user",
            "alice",
            "--password",
            "secret",
            "--password-stdin",
        ],
        vec!["shell", "--uid", "0"],
        vec!["shell", "--system", "--user", "alice"],
        vec!["shell", "--system", "--password", "secret"],
        vec!["users", "get", "nope"],
        vec!["config", "set", "p", "1", "x=bool:maybe"],
        vec!["tee", "open-session", "☃00000000000000000000000000000"],
        vec!["update", "check", "p", "--all"],
        vec!["trace", "start", "--buffer-size-kb", "0"],
        vec!["trace", "start", "--unknown"],
        vec!["ps", "surplus"],
    ] {
        let mut all = vec!["bexctl"];
        all.extend(argv);
        assert!(args::Cli::try_parse_from(all.clone()).is_err(), "{all:?}");
    }
}
#[test]
fn output_preserves_types_and_escapes_terminal_controls() {
    let rows = vec![json!({"pid":7,"name":"bad\x1b[31m\nname"})];
    let cols = [("pid", "PID"), ("name", "NAME")];
    let table = output::render(args::Format::Table, &cols, &rows);
    assert!(table.starts_with("PID"));
    assert!(!table.contains('\x1b'));
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&output::render(
            args::Format::Json,
            &cols,
            &rows
        ))
        .unwrap()[0]["pid"],
        7
    );
    assert_eq!(
        output::render(args::Format::Tsv, &cols, &[json!({"pid":7,"name":"name"})]),
        "7\tname\n"
    );
}
#[test]
fn reference_covers_command_tree() {
    let doc = std::fs::read_to_string(std::env::var("CLI_REFERENCE").unwrap()).unwrap();
    fn visit(c: &clap::Command, path: String, doc: &str) {
        assert!(
            doc.contains(&format!("`{path}")),
            "missing reference for {path}"
        );
        for s in c.get_subcommands() {
            if s.get_name() != "help" {
                visit(s, format!("{path} {}", s.get_name()), doc);
            }
        }
    }
    visit(&args::Cli::command(), "bexctl".into(), &doc);
}
#[test]
fn binary_dispatches_ps_and_preserves_exec_exit() {
    use bexos_debug_wire::*;
    use std::{
        io::{Read, Write},
        os::unix::net::UnixListener,
        process::Command,
        thread,
    };
    for (index, (command, format)) in [("ps", "json"), ("ps", "tsv"), ("exec", "json")]
        .iter()
        .enumerate()
    {
        let path = std::env::temp_dir().join(format!("bexctl-{}-{index}.sock", std::process::id()));
        let listener = UnixListener::bind(&path).unwrap();
        let expected = *command;
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut input = Vec::new();
            let frame = loop {
                let mut chunk = [0; 1024];
                let n = stream.read(&mut chunk).unwrap();
                assert_ne!(n, 0);
                input.extend_from_slice(&chunk[..n]);
                if let Ok((f, _)) = parse_frame(&input) {
                    break f;
                }
            };
            let mut payload = Vec::new();
            if expected == "ps" {
                assert_eq!(frame.method_id, METHOD_LIST_PROCESSES);
                encode_process_list(
                    &[ProcessInfo {
                        pid: 7,
                        resource_group_id: Some(1),
                        resource_group_name: "system".into(),
                        ..Default::default()
                    }],
                    &mut payload,
                );
            } else {
                assert_eq!(
                    decode_exec_request(&frame.payload).unwrap().component_id,
                    "debugd.version"
                );
                encode_exec_response(
                    &ExecResponse {
                        exit_code: 23,
                        stdout: "remote stdout".into(),
                        stderr: "remote stderr".into(),
                    },
                    &mut payload,
                );
            }
            let mut bytes = Vec::new();
            Frame { payload, ..frame }.encode(&mut bytes).unwrap();
            stream.write_all(&bytes).unwrap();
        });
        let mut process = Command::new(std::env::var("BEXCTL").unwrap());
        process.args([
            "--socket",
            path.to_str().unwrap(),
            "--format",
            format,
            command,
        ]);
        if *command == "exec" {
            process.arg("debugd.version");
        }
        let result = process.output().unwrap();
        server.join().unwrap();
        std::fs::remove_file(path).unwrap();
        if *command == "ps" {
            assert!(result.status.success());
            if *format == "json" {
                let rows: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
                assert_eq!(rows[0]["resource_group_name"], "system");
                assert!(rows[0]["main_thread_id"].is_null());
            } else {
                let output = String::from_utf8(result.stdout).unwrap();
                let fields: Vec<_> = output.trim_end().split('\t').collect();
                assert_eq!(fields.len(), 9);
                assert_eq!(&fields[4..], &["1", "system", "—", "—", "—"]);
            }
        } else {
            assert_eq!(result.status.code(), Some(23));
            assert_eq!(result.stdout, b"remote stdout");
            assert!(String::from_utf8_lossy(&result.stderr).contains("remote stderr"));
        }
    }
}
