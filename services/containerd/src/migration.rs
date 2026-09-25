use crate::{runtime::*, spec::*};
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
use bexos_pkg_client::PendingResolution;
use bexos_userspace::{
    Channel,
    live_migration::{Resource, State},
};
use container_fidl::ContainerState;
use std::collections::BTreeSet;

fn encode_desired(w: &mut Encoder, value: &DesiredSpec) {
    for text in [
        &value.container_id,
        &value.image.registry_host,
        &value.image.repository,
        &value.image.tag,
    ] {
        w.text(text);
    }
    w.bytes(&value.image.expected_sha256);
    w.word(value.arguments.len() as u64);
    for item in &value.arguments {
        w.text(item);
    }
    w.word(value.environment.len() as u64);
    for item in &value.environment {
        w.text(item);
    }
    w.text(&value.working_directory);
    w.word(value.uid.into());
    w.word(value.gid.into());
    w.text(&value.hostname);
    w.word(value.resources.cpu_shares.into());
    w.word(value.resources.memory_limit_bytes);
    w.word(value.resources.process_limit.into());
    w.word(u64::from(value.readonly_rootfs));
}
fn decode_desired(r: &mut Decoder<'_>) -> Result<DesiredSpec, Error> {
    let container_id = r.text(64)?.into();
    let registry_host = r.text(128)?.into();
    let repository = r.text(128)?.into();
    let tag = r.text(64)?.into();
    let expected_sha256 = r.bytes(32)?.to_vec();
    let mut arguments = Vec::new();
    for _ in 0..r.count(256)? {
        arguments.push(r.text(4096)?.into());
    }
    let mut environment = Vec::new();
    for _ in 0..r.count(256)? {
        environment.push(r.text(4096)?.into());
    }
    let value = DesiredSpec {
        container_id,
        image: ImageReference {
            registry_host,
            repository,
            tag,
            expected_sha256,
        },
        arguments,
        environment,
        working_directory: r.text(4096)?.into(),
        uid: r.word()?.try_into().map_err(|_| Error::InvalidData)?,
        gid: r.word()?.try_into().map_err(|_| Error::InvalidData)?,
        hostname: r.text(64)?.into(),
        resources: Resources {
            cpu_shares: r.word()?.try_into().map_err(|_| Error::InvalidData)?,
            memory_limit_bytes: r.word()?,
            process_limit: r.word()?.try_into().map_err(|_| Error::InvalidData)?,
        },
        readonly_rootfs: r.flag()?,
    };
    value.validate().map_err(|_| Error::InvalidData)?;
    Ok(value)
}

impl State for Runtime {
    fn empty() -> Self {
        Self::default()
    }
    fn keys(&self) -> Vec<u64> {
        (0..=self.containers.len() as u64).collect()
    }
    fn encode_record(&self, key: u64) -> Result<Option<Vec<u8>>, Error> {
        let mut w = Encoder::new();
        w.word(2);
        if key == 0 {
            for value in [
                self.control.0,
                self.migration.map_or(0, |c| c.0),
                self.data,
                self.resolver,
                self.launcher,
            ] {
                w.word(value);
            }
            w.word(self.clients.len() as u64);
            for c in &self.clients {
                w.word(c.channel);
                w.word(c.methods.len() as u64);
                for method in &c.methods {
                    w.word(*method);
                }
            }
            w.word(self.poll_tick.into());
            if let Some(p) = &self.pending {
                w.word(1);
                w.word(p.client);
                w.word(p.resolution.channel().map_or(0, |c| c.0));
                encode_desired(&mut w, &p.desired);
            } else {
                w.word(0);
            }
        } else {
            let Some(container) = self.containers.values().nth(key as usize - 1) else {
                return Ok(None);
            };
            w.bytes(
                container
                    .spec
                    .encode_prototxt()
                    .map_err(|_| Error::InvalidData)?
                    .as_bytes(),
            );
            w.word(match container.state {
                ContainerState::Created => 1,
                ContainerState::Running => 2,
                ContainerState::Stopped => 3,
            });
            w.word(container.exit_code as u32 as u64);
            w.word(container.process);
            for h in container.io_peers {
                w.word(h);
            }
            w.word(container.generation);
            w.word(container.proxies.len() as u64);
            for proxy in &container.proxies {
                w.word(proxy.channel);
                w.word(proxy.generation);
                w.word(proxy.completion.map_or(u64::MAX, |code| code as u32 as u64));
                w.word(u64::from(proxy.watch_pending));
            }
        }
        Ok(Some(w.finish()))
    }
    fn adopt_record(&mut self, key: u64, bytes: Option<&[u8]>) -> Result<(), Error> {
        let Some(bytes) = bytes else {
            return if key == 0 {
                Err(Error::InvalidData)
            } else {
                Ok(())
            };
        };
        let mut r = Decoder::new(bytes);
        let version = r.word()?;
        if !matches!(version, 1 | 2) {
            return Err(Error::UnsupportedVersion);
        }
        if key == 0 {
            self.control = Channel(r.word()?);
            let migration = r.word()?;
            self.migration = (migration != 0).then_some(Channel(migration));
            self.data = r.word()?;
            self.resolver = r.word()?;
            self.launcher = r.word()?;
            self.clients.clear();
            for _ in 0..r.count(256)? {
                let channel = r.word()?;
                let mut methods = Vec::new();
                for _ in 0..r.count(6)? {
                    methods.push(r.word()?);
                }
                self.clients.push(Client { channel, methods });
            }
            self.poll_tick = r.word()?.try_into().map_err(|_| Error::InvalidData)?;
            self.pending = if r.flag()? {
                let client = r.word()?;
                let channel = r.word()?;
                let desired = decode_desired(&mut r)?;
                Some(PendingCreate {
                    client,
                    resolution: PendingResolution::adopt(
                        Channel(channel),
                        desired.query().expected_digest,
                    ),
                    desired,
                })
            } else {
                None
            };
        } else {
            let spec = ContainerSpec::decode_prototxt(r.bytes(64 * 1024)?)
                .map_err(|_| Error::InvalidData)?;
            let state = match r.word()? {
                1 => ContainerState::Created,
                2 => ContainerState::Running,
                3 => ContainerState::Stopped,
                _ => return Err(Error::InvalidData),
            };
            let exit_code = r.word()? as u32 as i32;
            let process = r.word()?;
            let io_peers = [r.word()?, r.word()?, r.word()?];
            let generation = if version >= 2 { r.word()? } else { 0 };
            let mut proxies = Vec::new();
            if version >= 2 {
                for _ in 0..r.count(64)? {
                    let channel = r.word()?;
                    let proxy_generation = r.word()?;
                    let code = r.word()?;
                    proxies.push(ControlProxy {
                        channel,
                        generation: proxy_generation,
                        completion: (code != u64::MAX).then_some(code as u32 as i32),
                        watch_pending: r.flag()?,
                    });
                }
            }
            self.containers.insert(
                spec.container_id.clone(),
                Container {
                    spec,
                    state,
                    exit_code,
                    process,
                    io_peers,
                    generation,
                    proxies,
                },
            );
        }
        r.finish()
    }
    fn validate(&self) -> Result<(), Error> {
        if self.control.0 == 0
            || self.migration.is_none()
            || self.data == 0
            || self.launcher == 0
            || self.containers.len() > MAX_CONTAINERS
            || (self.resolver == 0) == self.pending.is_none()
        {
            return Err(Error::InvalidData);
        }
        if self
            .clients
            .iter()
            .any(|c| c.channel == 0 || c.methods.iter().any(|m| !matches!(m, 1..=6)))
        {
            return Err(Error::InvalidData);
        }
        for (id, c) in &self.containers {
            if id != &c.spec.container_id
                || c.spec.validate().is_err()
                || (c.state == ContainerState::Running) != (c.process != 0)
                || (c.state == ContainerState::Running && c.generation == 0)
                || c.proxies.len() > MAX_CONTROL_PROXIES
                || c.proxies.iter().any(|proxy| {
                    proxy.channel == 0
                        || proxy.generation == 0
                        || proxy.generation > c.generation
                        || (proxy.generation < c.generation && proxy.completion.is_none())
                })
            {
                return Err(Error::InvalidData);
            }
        }
        let mut unique = BTreeSet::new();
        if self.resources().iter().any(|r| match r {
            Resource::Handle(h) => !unique.insert(*h),
            _ => false,
        }) {
            return Err(Error::InvalidData);
        }
        Ok(())
    }
    fn resources(&self) -> Vec<Resource> {
        let mut handles = BTreeSet::new();
        handles.extend([
            self.control.0,
            self.migration.map_or(0, |c| c.0),
            self.data,
            self.resolver,
            self.launcher,
        ]);
        handles.extend(self.clients.iter().map(|c| c.channel));
        if let Some(p) = &self.pending {
            handles.insert(p.client);
            handles.insert(p.resolution.channel().map_or(0, |c| c.0));
        }
        for c in self.containers.values() {
            handles.insert(c.process);
            handles.extend(c.io_peers);
            handles.extend(c.proxies.iter().map(|p| p.channel));
        }
        handles.remove(&0);
        handles.into_iter().map(Resource::Handle).collect()
    }
    fn activated(&mut self, _generation: u64) {
        bexos_userspace::log("containerd: transplant activated\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn empty_runtime_validation_requires_authority_handles() {
        assert!(Runtime::default().validate().is_err());
    }
}
