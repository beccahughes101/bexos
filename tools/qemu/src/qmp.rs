//! Bounded QMP control for transferring the single RPMB connection.
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

fn receive(stream: &mut BufReader<UnixStream>) -> Result<Value, String> {
    let mut line = Vec::new();
    stream
        .take(65537)
        .read_until(b'\n', &mut line)
        .map_err(|e| format!("read QMP: {e}"))?;
    if line.is_empty() || line.len() > 65536 || line.last() != Some(&b'\n') {
        return Err("invalid or oversized QMP response".into());
    }
    serde_json::from_slice(&line).map_err(|e| format!("decode QMP: {e}"))
}

fn execute(
    stream: &mut BufReader<UnixStream>,
    command: &str,
    arguments: Value,
) -> Result<(), String> {
    let request = json!({"execute":command,"arguments":arguments,"id":command});
    serde_json::to_writer(stream.get_mut(), &request).map_err(|e| format!("write QMP: {e}"))?;
    stream
        .get_mut()
        .write_all(b"\n")
        .map_err(|e| format!("flush QMP: {e}"))?;
    for _ in 0..32 {
        let response = receive(stream)?;
        if response.get("id").and_then(Value::as_str) == Some(command) {
            return if response.get("return").is_some() {
                Ok(())
            } else {
                Err(format!("QMP {command} rejected: {response}"))
            };
        }
        if response.get("event").is_none() {
            return Err("unexpected QMP response identity".into());
        }
    }
    Err("QMP event limit exceeded".into())
}

pub fn change_serial(control: &Path, device: &str, endpoint: Option<&Path>) -> Result<(), String> {
    let socket = super::connect_unix(control).map_err(|e| format!("connect QMP: {e}"))?;
    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|e| e.to_string())?;
    socket
        .set_write_timeout(Some(Duration::from_secs(5)))
        .map_err(|e| e.to_string())?;
    let mut stream = BufReader::new(socket);
    if receive(&mut stream)?.get("QMP").is_none() {
        return Err("missing QMP greeting".into());
    }
    execute(&mut stream, "qmp_capabilities", json!({}))?;
    let backend = match endpoint {
        Some(path) => {
            let path = super::socket_argument(control.parent().unwrap_or(Path::new("")), path);
            json!({"type":"socket","data":{"addr":{"type":"unix","data":{"path":path}},"server":false}})
        }
        None => json!({"type":"null","data":{}}),
    };
    execute(
        &mut stream,
        "chardev-change",
        json!({"id":device,"backend":backend}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn responses_are_bounded_and_events_cannot_impersonate_acknowledgements() {
        for response in [
            b"{\"event\":\"RESET\"}\n{\"return\":{},\"id\":\"test\"}\n".as_slice(),
            b"{\"return\":{},\"id\":\"other\"}\n",
            b"{\"error\":{},\"id\":\"test\"}\n",
        ] {
            let (client, mut server) = UnixStream::pair().unwrap();
            server.write_all(response).unwrap();
            let result = execute(&mut BufReader::new(client), "test", json!({}));
            assert_eq!(result.is_ok(), response.starts_with(b"{\"event\""));
        }
        let (client, mut server) = UnixStream::pair().unwrap();
        let writer = std::thread::spawn(move || {
            let _ = server.write_all(&vec![b'x'; 65537]);
        });
        assert!(receive(&mut BufReader::new(client)).is_err());
        writer.join().unwrap();
    }
}
