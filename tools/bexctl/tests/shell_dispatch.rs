use bexos_debug_wire::*;
use std::{
    io::{Read, Write},
    os::unix::net::UnixListener,
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

#[test]
fn password_stdin_streams_before_eof_and_preserves_remote_exit() {
    let path = std::env::temp_dir().join(format!("bexctl-shell-{}.sock", std::process::id()));
    let listener = UnixListener::bind(&path).unwrap();
    listener.set_nonblocking(true).unwrap();
    let server = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        let (mut stream, _) = loop {
            match listener.accept() {
                Ok(connection) => break connection,
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline =>
                {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(e) => panic!("CLI did not connect: {e}"),
            }
        };
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let mut buffered = Vec::new();
        loop {
            let (frame, used) = loop {
                if let Ok(frame) = parse_frame(&buffered) {
                    break frame;
                }
                let mut chunk = [0; 4096];
                let n = stream.read(&mut chunk).unwrap_or(0);
                if n == 0 {
                    return;
                }
                buffered.extend_from_slice(&chunk[..n]);
            };
            buffered.drain(..used);
            let request = decode_shell_request(&frame.payload).unwrap();
            let mut reply = ShellResponse {
                session_id: 7,
                uid: 42,
                ..Default::default()
            };
            match frame.method_id {
                METHOD_SHELL_OPEN => {
                    assert_eq!(request.user, "alice");
                    assert_eq!(request.password, "secret");
                    assert!(!request.system);
                }
                METHOD_SHELL_MODE => assert_eq!(request.mode, 0),
                METHOD_SHELL_EXCHANGE => {
                    assert!(request.password.is_empty());
                    reply.consumed = request.input.len() as u32;
                    reply.stdout = request.input;
                    if request.eof {
                        reply.stdout.extend_from_slice(b"finished");
                        reply.exited = true;
                        reply.exit_code = 17;
                    }
                }
                METHOD_SHELL_CLOSE => {
                    reply.status = -1;
                    reply.message = "no active shell session".into();
                }
                other => panic!("unexpected shell method {other}"),
            }
            let mut payload = Vec::new();
            encode_shell_response(&reply, &mut payload);
            let mut bytes = Vec::new();
            Frame {
                payload,
                ..frame.clone()
            }
            .encode(&mut bytes)
            .unwrap();
            if stream.write_all(&bytes).is_err() || frame.method_id == METHOD_SHELL_CLOSE {
                break;
            }
        }
    });
    let mut child = Command::new(std::env::var("BEXCTL").unwrap())
        .args([
            "--socket",
            path.to_str().unwrap(),
            "shell",
            "--user",
            "alice",
            "--password-stdin",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = child.stdout.take().unwrap();
    stdin.write_all(b"secret\npayload\0bytes").unwrap();
    let (send, receive) = mpsc::channel();
    let reader = thread::spawn(move || {
        let mut output = [0; 13];
        let result = stdout.read_exact(&mut output);
        let _ = send.send((result, output, stdout));
    });
    let first = receive.recv_timeout(Duration::from_secs(5));
    if first.is_err() {
        let _ = child.kill();
    }
    drop(stdin);
    let status = child.wait().unwrap();
    reader.join().unwrap();
    let server_result = server.join();
    std::fs::remove_file(path).unwrap();
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    assert_eq!(status.code(), Some(17), "{stderr}");
    server_result.unwrap();
    let (read, output, mut stdout) = first.expect("terminal input was buffered until stdin EOF");
    read.unwrap();
    assert_eq!(&output, b"payload\0bytes");
    let mut tail = Vec::new();
    stdout.read_to_end(&mut tail).unwrap();
    assert_eq!(tail, b"finished");
    assert_eq!(status.code(), Some(17));
}
