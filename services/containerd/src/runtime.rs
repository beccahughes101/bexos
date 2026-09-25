use crate::spec::{ContainerSpec, ImageReference, Resources};
use bexos_pkg_client::{ArtifactKind, ArtifactQuery, BlobDigest, HashType, PendingResolution};
use bexos_userspace::{Channel, Memory, Socket};
use container_fidl::{ContainerState, ContainerStatus};
use std::collections::BTreeMap;

pub const MAX_CONTAINERS: usize = 256;
pub const MAX_CONTROL_PROXIES: usize = 64;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Client {
    pub channel: u64,
    pub methods: Vec<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DesiredSpec {
    pub container_id: String,
    pub image: ImageReference,
    pub arguments: Vec<String>,
    pub environment: Vec<String>,
    pub working_directory: String,
    pub uid: u32,
    pub gid: u32,
    pub hostname: String,
    pub resources: Resources,
    pub readonly_rootfs: bool,
}

impl DesiredSpec {
    pub fn query(&self) -> ArtifactQuery {
        ArtifactQuery {
            registry_host: self.image.registry_host.clone(),
            repository: self.image.repository.clone(),
            tag: self.image.tag.clone(),
            expected_digest: self
                .image
                .expected_sha256
                .as_slice()
                .try_into()
                .ok()
                .map(|digest| BlobDigest {
                    hash_type: HashType::Sha256,
                    digest,
                }),
            kind: ArtifactKind::Container,
        }
    }
    pub fn validate(&self) -> Result<(), ContainerStatus> {
        if !crate::spec::valid_id(&self.container_id)
            || !matches!(self.image.expected_sha256.len(), 0 | 32)
            || self.arguments.len() > 64
            || self.environment.len() > 64
            || self
                .arguments
                .iter()
                .any(|v| v.len() > 4096 || v.contains('\0'))
            || self.environment.iter().any(|v| {
                v.len() > 4096
                    || v.contains('\0')
                    || v.split_once('=').is_none_or(|(name, _)| name.is_empty())
            })
            || (!self.working_directory.is_empty()
                && (!self.working_directory.starts_with('/')
                    || self.working_directory.len() > 4096))
            || self.hostname.len() > 64
            || self.query().validate().is_err()
        {
            return Err(ContainerStatus::InvalidArgs);
        }
        Ok(())
    }
}

pub struct PendingCreate {
    pub client: u64,
    pub desired: DesiredSpec,
    pub resolution: PendingResolution,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Container {
    pub spec: ContainerSpec,
    pub state: ContainerState,
    pub exit_code: i32,
    pub process: u64,
    /// Locally created stdio peers.  stdout/stderr are drained while running.
    pub io_peers: [u64; 3],
    pub generation: u64,
    pub proxies: Vec<ControlProxy>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ControlProxy {
    pub channel: u64,
    pub generation: u64,
    pub completion: Option<i32>,
    pub watch_pending: bool,
}

impl Container {
    pub fn cold(spec: ContainerSpec) -> Self {
        Self {
            spec,
            state: ContainerState::Stopped,
            exit_code: 0,
            process: 0,
            io_peers: [0; 3],
            generation: 0,
            proxies: Vec::new(),
        }
    }
    pub fn close_runtime_handles(&mut self) {
        if self.process != 0 {
            let _ = Memory::close(self.process);
            self.process = 0;
        }
        for handle in &mut self.io_peers {
            if *handle != 0 {
                let _ = Memory::close(*handle);
                *handle = 0;
            }
        }
    }
    pub fn drain_output(&self) {
        for handle in &self.io_peers[1..] {
            if *handle == 0 {
                continue;
            }
            for _ in 0..8 {
                match Socket(*handle).read(32 * 1024) {
                    Ok(bytes) if !bytes.is_empty() => {}
                    _ => break,
                }
            }
        }
    }
}

pub struct Runtime {
    pub control: Channel,
    pub migration: Option<Channel>,
    pub data: u64,
    pub resolver: u64,
    pub launcher: u64,
    pub clients: Vec<Client>,
    pub containers: BTreeMap<String, Container>,
    pub pending: Option<PendingCreate>,
    pub poll_tick: u32,
}

impl Default for Runtime {
    fn default() -> Self {
        Self {
            control: Channel(0),
            migration: None,
            data: 0,
            resolver: 0,
            launcher: 0,
            clients: Vec::new(),
            containers: BTreeMap::new(),
            pending: None,
            poll_tick: 0,
        }
    }
}

pub fn effective_spec(
    desired: DesiredSpec,
    process: &bexos_oci::ProcessConfig,
    manifest_digest: [u8; 32],
) -> Result<ContainerSpec, ContainerStatus> {
    let arguments = if desired.arguments.is_empty() {
        process.arguments.clone()
    } else {
        desired.arguments.clone()
    };
    let mut environment = process.environment.clone();
    for requested in &desired.environment {
        let name = requested
            .split_once('=')
            .ok_or(ContainerStatus::InvalidArgs)?
            .0;
        environment.retain(|current| current.split_once('=').is_none_or(|(old, _)| old != name));
        environment.push(requested.clone());
    }
    let working_directory = if desired.working_directory.is_empty() {
        process.working_directory.clone()
    } else {
        desired.working_directory.clone()
    };
    let (mut uid, mut gid) = (desired.uid, desired.gid);
    if uid == 0 && gid == 0 && !process.user.is_empty() {
        let (u, g) = process
            .user
            .split_once(':')
            .unwrap_or((&process.user, &process.user));
        uid = u.parse().map_err(|_| ContainerStatus::VerifyFailed)?;
        gid = g.parse().map_err(|_| ContainerStatus::VerifyFailed)?;
    }
    let spec = ContainerSpec {
        version: 1,
        container_id: desired.container_id.clone(),
        image: desired.image,
        arguments,
        environment,
        working_directory,
        uid,
        gid,
        hostname: if desired.hostname.is_empty() {
            desired.container_id
        } else {
            desired.hostname
        },
        resources: desired.resources,
        readonly_rootfs: desired.readonly_rootfs,
        manifest_digest,
    };
    spec.validate().map_err(|_| ContainerStatus::InvalidArgs)?;
    Ok(spec)
}

pub fn architecture() -> &'static str {
    #[cfg(target_arch = "aarch64")]
    {
        "arm64"
    }
    #[cfg(target_arch = "x86_64")]
    {
        "amd64"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn requested_environment_overrides_image_by_name() {
        let desired = DesiredSpec {
            container_id: "c".into(),
            image: ImageReference {
                registry_host: "r.test".into(),
                repository: "a/b".into(),
                tag: "v1".into(),
                expected_sha256: vec![],
            },
            arguments: vec![],
            environment: vec!["A=new".into(), "C=3".into()],
            working_directory: String::new(),
            uid: 0,
            gid: 0,
            hostname: String::new(),
            resources: Resources::default(),
            readonly_rootfs: true,
        };
        let process = bexos_oci::ProcessConfig {
            arguments: vec!["/bin/x".into()],
            environment: vec!["A=old".into(), "B=2".into()],
            working_directory: "/work".into(),
            user: "12:34".into(),
        };
        let result = effective_spec(desired, &process, [8; 32]).unwrap();
        assert_eq!(result.environment, ["B=2", "A=new", "C=3"]);
        assert_eq!((result.uid, result.gid), (12, 34));
        assert_eq!(result.working_directory, "/work");
    }
}
