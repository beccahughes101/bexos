use crate::spec::ContainerSpec;
use app_opener_fidl::*;
use bexos_userspace::{Channel, Memory};

fn encode<T: FidlEncode>(
    ordinal: u64,
    value: &T,
    handles: &mut [HandleRef],
) -> Result<Vec<u8>, OpenerStatus> {
    let mut bytes = vec![0; 128 * 1024];
    let encoded = value
        .encode(&mut bytes[8..], handles)
        .map_err(|_| OpenerStatus::InvalidArgs)?;
    bytes[..8].copy_from_slice(&ordinal.to_le_bytes());
    bytes.truncate(encoded.bytes + 8);
    Ok(bytes)
}

pub fn bind(opener: Channel) -> Result<Channel, OpenerStatus> {
    let (client, server) = Channel::pair().map_err(|_| OpenerStatus::LaunchFailed)?;
    let mut refs = [HandleRef { raw: 0 }; 1];
    let bytes = encode(
        6,
        &OpenerBindCommandLauncherRequest {
            launcher: HandleRef { raw: server.0 },
        },
        &mut refs,
    )?;
    if opener.send(&bytes, &[server.0]).is_err() {
        let _ = Memory::close(client.0);
        let _ = Memory::close(server.0);
        return Err(OpenerStatus::LaunchFailed);
    }
    let message = opener
        .recv_blocking()
        .map_err(|_| OpenerStatus::LaunchFailed)?;
    let response = OpenerBindCommandLauncherResponse::decode(&message.bytes, &[])
        .map_err(|_| OpenerStatus::LaunchFailed)?;
    if response.status != OpenerStatus::Ok {
        let _ = Memory::close(client.0);
        return Err(response.status);
    }
    Ok(client)
}

pub fn launch(
    launcher: u64,
    spec: &ContainerSpec,
    rootfs: u64,
    stdio: [u64; 3],
) -> Result<u64, OpenerStatus> {
    let handles = [rootfs, stdio[0], stdio[1], stdio[2]];
    let args: Vec<_> = spec.arguments.iter().map(String::as_str).collect();
    let env: Vec<_> = spec.environment.iter().map(String::as_str).collect();
    let executable = args.first().copied().ok_or(OpenerStatus::InvalidArgs)?;
    let request = CommandLauncherLaunchContainerRequest {
        container_id: &spec.container_id,
        rootfs: HandleRef { raw: rootfs },
        executable,
        arguments: WireStringVector::from_slice(&args),
        environment: WireStringVector::from_slice(&env),
        working_directory: &spec.working_directory,
        uid: spec.uid,
        gid: spec.gid,
        hostname: &spec.hostname,
        readonly_rootfs: spec.readonly_rootfs,
        resources: ContainerResourceLimits {
            cpu_shares: spec.resources.cpu_shares,
            memory_limit_bytes: spec.resources.memory_limit_bytes,
            process_limit: spec.resources.process_limit,
        },
        stdin_stream: HandleRef { raw: stdio[0] },
        stdout_stream: HandleRef { raw: stdio[1] },
        stderr_stream: HandleRef { raw: stdio[2] },
    };
    let mut refs = [HandleRef { raw: 0 }; 4];
    let bytes = match encode(5, &request, &mut refs) {
        Ok(bytes) => bytes,
        Err(error) => {
            for handle in handles {
                let _ = Memory::close(handle);
            }
            return Err(error);
        }
    };
    if Channel(launcher).send(&bytes, &handles).is_err() {
        for h in handles {
            let _ = Memory::close(h);
        }
        return Err(OpenerStatus::LaunchFailed);
    }
    let message = Channel(launcher)
        .recv_blocking()
        .map_err(|_| OpenerStatus::LaunchFailed)?;
    let response = CommandLauncherLaunchContainerResponse::decode(
        &message.bytes,
        &message
            .handles
            .iter()
            .map(|raw| HandleRef { raw: *raw })
            .collect::<Vec<_>>(),
    )
    .map_err(|_| OpenerStatus::LaunchFailed)?;
    if response.status != OpenerStatus::Ok
        || response.process_control.raw == 0
        || message.handles != [response.process_control.raw]
    {
        for h in message.handles {
            let _ = Memory::close(h);
        }
        return Err(if response.status == OpenerStatus::Ok {
            OpenerStatus::LaunchFailed
        } else {
            response.status
        });
    }
    Ok(response.process_control.raw)
}

pub fn status(process: u64) -> Result<(bool, i32), OpenerStatus> {
    let mut refs = [];
    let bytes = encode(1, &ProcessControlGetStatusRequest {}, &mut refs)?;
    Channel(process)
        .send(&bytes, &[])
        .map_err(|_| OpenerStatus::LaunchFailed)?;
    let message = Channel(process)
        .recv_blocking()
        .map_err(|_| OpenerStatus::LaunchFailed)?;
    let response = ProcessControlGetStatusResponse::decode(&message.bytes, &[])
        .map_err(|_| OpenerStatus::LaunchFailed)?;
    if response.status != OpenerStatus::Ok {
        return Err(response.status);
    }
    Ok((response.exited, response.exit_code))
}

pub fn signal(process: u64, signal: u32) -> Result<(), OpenerStatus> {
    let mut refs = [];
    let bytes = encode(3, &ProcessControlSendSignalRequest { signal }, &mut refs)?;
    Channel(process)
        .send(&bytes, &[])
        .map_err(|_| OpenerStatus::LaunchFailed)?;
    let message = Channel(process)
        .recv_blocking()
        .map_err(|_| OpenerStatus::LaunchFailed)?;
    let response = ProcessControlSendSignalResponse::decode(&message.bytes, &[])
        .map_err(|_| OpenerStatus::LaunchFailed)?;
    if response.status == OpenerStatus::Ok {
        Ok(())
    } else {
        Err(response.status)
    }
}
