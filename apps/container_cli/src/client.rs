use bexos_userspace::{Channel, Memory};
use container_fidl::*;

pub struct Client(pub Channel);

impl Client {
    fn call<Q: FidlEncode>(
        &self,
        ordinal: u64,
        request: &Q,
        handles: &[u64],
    ) -> Result<bexos_userspace::Message, ContainerStatus> {
        let mut bytes = vec![0; 128 * 1024];
        let mut refs = [HandleRef { raw: 0 }; 4];
        let encoded = match request.encode(&mut bytes[8..], &mut refs) {
            Ok(encoded) => encoded,
            Err(_) => {
                close(handles);
                return Err(ContainerStatus::InvalidArgs);
            }
        };
        bytes[..8].copy_from_slice(&ordinal.to_le_bytes());
        if self.0.send(&bytes[..encoded.bytes + 8], handles).is_err() {
            close(handles);
            return Err(ContainerStatus::Unavailable);
        }
        let message = self
            .0
            .recv_blocking()
            .map_err(|_| ContainerStatus::Unavailable)?;
        if ordinal != 2 && !message.handles.is_empty() {
            close(&message.handles);
            return Err(ContainerStatus::Unavailable);
        }
        Ok(message)
    }
    pub fn create(&self, options: &crate::parse::Create) -> Result<(), ContainerStatus> {
        let args = options
            .arguments
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        let env = options
            .environment
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        let message = self.call(
            1,
            &ContainerManagerCreateRequest {
                container_id: &options.id,
                image: ImageReference {
                    registry_host: &options.host,
                    repository: &options.repository,
                    tag: &options.tag,
                    expected_sha256: &options.digest,
                },
                arguments: WireStringVector::from_slice(&args),
                environment: WireStringVector::from_slice(&env),
                working_directory: &options.cwd,
                uid: options.uid,
                gid: options.gid,
                hostname: &options.hostname,
                resources: ResourceLimits {
                    cpu_shares: options.cpu_shares,
                    memory_limit_bytes: options.memory,
                    process_limit: options.pids,
                },
                readonly_rootfs: options.readonly,
            },
            &[],
        )?;
        check(
            ContainerManagerCreateResponse::decode(&message.bytes, &[])
                .map_err(|_| ContainerStatus::Unavailable)?
                .status,
        )
    }
    pub fn start(&self, id: &str, stdio: [u64; 3]) -> Result<u64, ContainerStatus> {
        let message = self.call(
            2,
            &ContainerManagerStartRequest {
                container_id: id,
                stdin_stream: HandleRef { raw: stdio[0] },
                stdout_stream: HandleRef { raw: stdio[1] },
                stderr_stream: HandleRef { raw: stdio[2] },
            },
            &stdio,
        )?;
        let refs = message
            .handles
            .iter()
            .map(|raw| HandleRef { raw: *raw })
            .collect::<Vec<_>>();
        let response = match ContainerManagerStartResponse::decode(&message.bytes, &refs) {
            Ok(response) => response,
            Err(_) => {
                close(&message.handles);
                return Err(ContainerStatus::Unavailable);
            }
        };
        if let Err(error) = check(response.status) {
            close(&message.handles);
            return Err(error);
        }
        if response.process_control.raw == 0 || message.handles != [response.process_control.raw] {
            close(&message.handles);
            return Err(ContainerStatus::Unavailable);
        }
        Ok(response.process_control.raw)
    }
    pub fn signal(&self, id: &str, signal: u32) -> Result<(), ContainerStatus> {
        let m = self.call(
            3,
            &ContainerManagerSignalRequest {
                container_id: id,
                signal,
            },
            &[],
        )?;
        check(
            ContainerManagerSignalResponse::decode(&m.bytes, &[])
                .map_err(|_| ContainerStatus::Unavailable)?
                .status,
        )
    }
    pub fn delete(&self, id: &str, force: bool) -> Result<(), ContainerStatus> {
        let m = self.call(
            4,
            &ContainerManagerDeleteRequest {
                container_id: id,
                force,
            },
            &[],
        )?;
        check(
            ContainerManagerDeleteResponse::decode(&m.bytes, &[])
                .map_err(|_| ContainerStatus::Unavailable)?
                .status,
        )
    }
    pub fn inspect(&self, id: &str) -> Result<Vec<OwnedInfo>, ContainerStatus> {
        let m = self.call(5, &ContainerManagerInspectRequest { container_id: id }, &[])?;
        let r = ContainerManagerInspectResponse::decode(&m.bytes, &[])
            .map_err(|_| ContainerStatus::Unavailable)?;
        check(r.status)?;
        owned(&r.info)
    }
    pub fn list(&self) -> Result<Vec<OwnedInfo>, ContainerStatus> {
        let m = self.call(6, &ContainerManagerListRequest {}, &[])?;
        let r = ContainerManagerListResponse::decode(&m.bytes, &[])
            .map_err(|_| ContainerStatus::Unavailable)?;
        check(r.status)?;
        owned(&r.containers)
    }
}
fn close(handles: &[u64]) {
    for handle in handles {
        if *handle != 0 {
            let _ = Memory::close(*handle);
        }
    }
}
fn check(status: ContainerStatus) -> Result<(), ContainerStatus> {
    if status == ContainerStatus::Ok {
        Ok(())
    } else {
        Err(status)
    }
}

#[derive(Debug)]
pub struct OwnedInfo {
    pub id: String,
    pub state: ContainerState,
    pub digest: [u8; 32],
    pub exit_code: i32,
}
fn owned(values: &WireVector<'_, ContainerInfo<'_>>) -> Result<Vec<OwnedInfo>, ContainerStatus> {
    (0..values.len())
        .map(|i| {
            values
                .get(i)
                .map(|v| OwnedInfo {
                    id: v.container_id.into(),
                    state: v.state,
                    digest: v.manifest_digest,
                    exit_code: v.exit_code,
                })
                .map_err(|_| ContainerStatus::Unavailable)
        })
        .collect()
}

pub fn process_status(process: u64) -> Result<(bool, i32), ContainerStatus> {
    use app_opener_fidl::*;
    let mut bytes = vec![0; 64];
    bytes[..8].copy_from_slice(&1u64.to_le_bytes());
    let encoded = ProcessControlGetStatusRequest {}
        .encode(&mut bytes[8..], &mut [])
        .map_err(|_| ContainerStatus::Unavailable)?;
    Channel(process)
        .send(&bytes[..8 + encoded.bytes], &[])
        .map_err(|_| ContainerStatus::Unavailable)?;
    let m = Channel(process)
        .recv_blocking()
        .map_err(|_| ContainerStatus::Unavailable)?;
    let r = ProcessControlGetStatusResponse::decode(&m.bytes, &[])
        .map_err(|_| ContainerStatus::Unavailable)?;
    if r.status != OpenerStatus::Ok {
        return Err(ContainerStatus::LaunchFailed);
    }
    Ok((r.exited, r.exit_code))
}
