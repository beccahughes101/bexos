//! Explicit appd delegation and bounded stdio pumping for external commands.
use app_opener_fidl as opener_fidl;
use bexos_wasm_guest::bexos::wasm::kernel;
use brush_core::{
    ExecutionResult,
    bexos::{Command, CommandFuture},
};
use opener_fidl::{FidlDecode, HandleRef, WireStringVector};
use std::io::{Read, Write};

use bexos_wasm_guest::command::{Resource, bind, cwd, receive, send};
pub fn launch(mut request: Command) -> CommandFuture {
    Box::pin(async move {
        let result = execute(&mut request).await;
        let status = match result {
            Ok(status) => status,
            Err((status, error)) => {
                if let Some(stderr) = &mut request.files[2] {
                    let _ = writeln!(stderr, "brush: {}: {error}", request.name);
                }
                status
            }
        };
        Ok(ExecutionResult::new(status))
    })
}
async fn execute(request: &mut Command) -> Result<u8, (u8, String)> {
    let fail = |s: String| (126, s);
    let launcher = bind().await.map_err(fail)?;
    let mut selected = None;
    for candidate in crate::path::candidates(&request.name, &request.path, &request.cwd) {
        let name = match candidate {
            crate::path::Candidate::Installed(name) => name,
            crate::path::Candidate::File(path) => match std::fs::metadata(&path) {
                Ok(_) => {
                    return Err((
                        126,
                        format!(
                            "{} is not a declared app command; source shell scripts with .",
                            path.display()
                        ),
                    ));
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err((126, format!("{}: {e}", path.display()))),
            },
        };
        send(
            launcher.0,
            1,
            &opener_fidl::CommandLauncherResolveCommandRequest { name: &name },
        )
        .map_err(fail)?;
        let reply = receive(launcher.0).await.map_err(fail)?;
        let resolved = opener_fidl::CommandLauncherResolveCommandResponse::decode(&reply.data, &[])
            .map_err(|_| (126, "invalid command resolution reply".into()))?;
        match resolved.status {
            opener_fidl::OpenerStatus::Ok => {
                selected = Some((
                    name,
                    resolved.package_name.to_owned(),
                    resolved.process_name.to_owned(),
                ));
                break;
            }
            opener_fidl::OpenerStatus::NotFound => continue,
            _ => {
                return Err((
                    126,
                    "command is ambiguous or invalid; use package:command".into(),
                ));
            }
        }
    }
    let (name, package_name, process_name) =
        selected.ok_or((127, "command not found in PATH".into()))?;
    let cwd = cwd(&request.cwd).map_err(fail)?;
    let mut parents = Vec::new();
    let mut children = Vec::new();
    for _ in 0..3 {
        let (a, b) = kernel::socket_pair().map_err(|_| (126, "stdio resource limit".into()))?;
        parents.push(Resource(a));
        children.push(Resource(b));
    }
    let args: Vec<_> = request.arguments.iter().map(String::as_str).collect();
    let env: Vec<_> = request
        .environment
        .iter()
        .map(|(n, v)| format!("{n}={v}"))
        .collect();
    let envrefs: Vec<_> = env.iter().map(String::as_str).collect();
    let command_name = name
        .rsplit_once(':')
        .map(|(_, c)| c)
        .unwrap_or(&name)
        .strip_prefix("/system/bin/")
        .unwrap_or(name.rsplit_once(':').map(|(_, c)| c).unwrap_or(&name));
    send(
        launcher.0,
        2,
        &opener_fidl::CommandLauncherLaunchCommandRequest {
            command_name,
            package_name: &package_name,
            process_name: &process_name,
            arguments: WireStringVector::from_slice(&args),
            environment: WireStringVector::from_slice(&envrefs),
            cwd: HandleRef { raw: cwd.0.into() },
            stdin_stream: HandleRef {
                raw: children[0].0.into(),
            },
            stdout_stream: HandleRef {
                raw: children[1].0.into(),
            },
            stderr_stream: HandleRef {
                raw: children[2].0.into(),
            },
        },
    )
    .map_err(fail)?;
    let reply = receive(launcher.0).await.map_err(fail)?;
    let handles: Vec<_> = reply
        .resources
        .iter()
        .map(|h| HandleRef { raw: (*h).into() })
        .collect();
    let response = opener_fidl::CommandLauncherLaunchCommandResponse::decode(&reply.data, &handles)
        .map_err(|_| (126, "invalid command launch reply".into()))?;
    let owned: Vec<_> = reply.resources.iter().map(|h| Resource(*h)).collect();
    if response.status != opener_fidl::OpenerStatus::Ok
        || owned.len() != 1
        || response.process_control.raw != owned[0].0 as u64
    {
        return Err((126, "appd rejected command launch".into()));
    }
    let control = &owned[0];
    drop(children);
    let mut interrupted = false;
    let mut pending: [Vec<u8>; 3] = Default::default();
    let mut eof = [false; 3];
    let mut status = None;
    loop {
        if pending[0].is_empty() && !eof[0] {
            let mut bytes = [0; 4096];
            match request.files[0]
                .as_mut()
                .map_or(Ok(0), |f| f.read(&mut bytes))
            {
                Ok(0) => {
                    eof[0] = true;
                    let _ = kernel::socket_shutdown(parents[0].0, false, true);
                }
                Ok(n) => pending[0].extend_from_slice(&bytes[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => (),
                Err(e) => return Err((126, format!("stdin: {e}"))),
            }
        }
        if !pending[0].is_empty() {
            match kernel::socket_write(parents[0].0, &pending[0]) {
                Ok(n) => {
                    pending[0].drain(..n as usize);
                }
                Err(kernel::StreamError::WouldBlock) => (),
                Err(_) => {
                    pending[0].clear();
                    eof[0] = true;
                }
            }
        }
        for i in 1..3 {
            if pending[i].is_empty() && !eof[i] {
                match kernel::socket_read(parents[i].0, 4096) {
                    Ok(bytes) => {
                        eof[i] = bytes.is_empty();
                        pending[i] = bytes;
                    }
                    Err(kernel::StreamError::WouldBlock) => (),
                    Err(_) => return Err((126, "command output disconnected".into())),
                }
            }
            if !pending[i].is_empty() {
                match request.files[i]
                    .as_mut()
                    .map_or(Ok(pending[i].len()), |f| f.write(&pending[i]))
                {
                    Ok(n) => {
                        pending[i].drain(..n);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => (),
                    Err(e) => return Err((126, format!("output: {e}"))),
                }
            }
        }
        if status.is_none() {
            if brush_core::bexos::interrupted() && !interrupted {
                send(
                    control.0,
                    3,
                    &opener_fidl::ProcessControlSendSignalRequest { signal: 2 },
                )
                .map_err(fail)?;
                let reply = receive(control.0).await.map_err(fail)?;
                let response =
                    opener_fidl::ProcessControlSendSignalResponse::decode(&reply.data, &[])
                        .map_err(|_| (126, "invalid signal reply".into()))?;
                if response.status != opener_fidl::OpenerStatus::Ok {
                    return Err((126, "appd could not interrupt command".into()));
                }
                interrupted = true;
            }
            send(
                control.0,
                1,
                &opener_fidl::ProcessControlGetStatusRequest {},
            )
            .map_err(fail)?;
            let reply = receive(control.0).await.map_err(fail)?;
            let response = opener_fidl::ProcessControlGetStatusResponse::decode(&reply.data, &[])
                .map_err(|_| (126, "invalid command status reply".into()))?;
            if response.status != opener_fidl::OpenerStatus::Ok {
                return Err((126, "command status unavailable".into()));
            }
            if response.exited {
                status = Some(response.exit_code as u8);
            }
        }

        if let Some(status) = status {
            if eof[1] && eof[2] && pending[1].is_empty() && pending[2].is_empty() {
                return Ok(status);
            }
        }
        tokio::task::yield_now().await;
    }
}
