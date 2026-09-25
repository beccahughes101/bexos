use crate::runtime::{Blob, Client, Job, Runtime, SecretHandle, Waiter};
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
use bexos_pkg_client::{ArtifactKind, ArtifactQuery, BlobDigest, HashType};
use bexos_userspace::{
    Channel,
    live_migration::{Resource, State},
};
use sha2::{Digest, Sha256};
#[cfg(test)]
mod tests;
pub struct Inventory {
    blobs: Vec<[u8; 32]>,
    secrets: Vec<String>,
    jobs: usize,
}
const JOBS: u64 = 1000;
const BLOBS: u64 = 10000;
const SECRETS: u64 = 20000;
impl Runtime {
    pub fn state_keys(&self) -> Vec<u64> {
        let mut keys = vec![0];
        keys.extend((0..self.jobs.len()).map(|i| JOBS + i as u64));
        keys.extend((0..self.blobs.len()).map(|i| BLOBS + i as u64));
        keys.extend((0..self.secrets.len()).map(|i| SECRETS + i as u64));
        keys
    }
}
impl State for Runtime {
    fn empty() -> Self {
        Self::empty()
    }
    fn keys(&self) -> Vec<u64> {
        self.state_keys()
    }
    fn encode_record(&self, key: u64) -> Result<Option<Vec<u8>>, Error> {
        let mut w = Encoder::new();
        w.word(1);
        if key == 0 {
            w.bytes(&Sha256::digest(include_bytes!(env!("PKGD_CONFIG"))));
            for handle in [
                self.control.0,
                self.migration.map_or(0, |c| c.0),
                self.vfs.0,
                self.network.0,
                self.tls.0,
                self.tee.0,
                self.time.0,
                self.cache_file.0,
                self.state_file.0,
                self.directory.0,
            ] {
                w.word(handle);
            }
            w.word(self.blobs.len() as u64);
            for digest in self.blobs.keys() {
                w.bytes(digest);
            }
            w.word(self.secrets.len() as u64);
            for host in self.secrets.keys() {
                w.text(host);
            }
            w.word(self.jobs.len() as u64);
            w.word(self.clients.len() as u64);
            for client in &self.clients {
                w.word(client.channel);
                w.text(&client.package);
                w.text(&client.protocol);
                w.word(client.methods.len() as u64);
                for ordinal in &client.methods {
                    w.word(*ordinal);
                }
            }
        } else if (JOBS..BLOBS).contains(&key) {
            let Some(job) = self.jobs.get((key - JOBS) as usize) else {
                return Ok(None);
            };
            w.text(&job.query.registry_host);
            w.text(&job.query.repository);
            w.text(&job.query.tag);
            w.word(job.query.kind as u64);
            w.word(job.attempt as u64);
            if let Some(digest) = job.source_digest {
                w.word(digest.hash_type as u64);
                w.bytes(&digest.digest);
            } else {
                w.word(0);
            }
            w.word(job.waiters.len() as u64);
            for waiter in &job.waiters {
                w.word(waiter.channel);
                w.text(&waiter.caller);
                if let Some(digest) = waiter.expected {
                    w.word(digest.hash_type as u64);
                    w.bytes(&digest.digest);
                } else {
                    w.word(0);
                }
            }
        } else if (BLOBS..SECRETS).contains(&key) {
            let Some((digest, blob)) = self.blobs.iter().nth((key - BLOBS) as usize) else {
                return Ok(None);
            };
            w.bytes(digest);
            w.word(blob.handle);
            w.word(blob.length);
        } else if key >= SECRETS {
            let Some((host, secret)) = self.secrets.iter().nth((key - SECRETS) as usize) else {
                return Ok(None);
            };
            w.text(host);
            w.word(secret.handle);
            w.word(secret.length);
            w.word(secret.sealed as u64);
        } else {
            return Ok(None);
        }
        let bytes = w.finish();
        if bytes.len() > bexos_userspace::live_migration::MAX_RECORD_DATA {
            return Err(Error::InvalidData);
        }
        Ok(Some(bytes))
    }
    fn adopt_record(&mut self, key: u64, bytes: Option<&[u8]>) -> Result<(), Error> {
        let Some(bytes) = bytes else {
            return Ok(());
        };
        let mut r = Decoder::new(bytes);
        if r.word()? != 1 {
            return Err(Error::UnsupportedVersion);
        }
        if key == 0 {
            if r.bytes(32)? != Sha256::digest(include_bytes!(env!("PKGD_CONFIG"))).as_slice() {
                return Err(Error::InvalidData);
            }
            self.control = Channel(r.word()?);
            let migration = r.word()?;
            self.migration = (migration != 0).then_some(Channel(migration));
            self.vfs = Channel(r.word()?);
            self.network = Channel(r.word()?);
            self.tls = Channel(r.word()?);
            self.tee = Channel(r.word()?);
            self.time = Channel(r.word()?);
            self.cache_file = Channel(r.word()?);
            self.state_file = Channel(r.word()?);
            self.directory = Channel(r.word()?);
            let mut blob_ids = Vec::new();
            for _ in 0..r.count(128)? {
                blob_ids.push(<[u8; 32]>::try_from(r.bytes(32)?).map_err(|_| Error::InvalidData)?);
            }
            self.blobs.retain(|digest, _| blob_ids.contains(digest));
            let mut secret_hosts = Vec::new();
            for _ in 0..r.count(64)? {
                secret_hosts.push(r.text(128)?.to_string());
            }
            self.secrets.retain(|host, _| secret_hosts.contains(host));
            let jobs = r.count(self.config.max_inflight)?;
            if blob_ids.windows(2).any(|w| w[0] >= w[1])
                || secret_hosts.windows(2).any(|w| w[0] >= w[1])
            {
                return Err(Error::InvalidData);
            }
            self.adoption_inventory = Some(Inventory {
                blobs: blob_ids,
                secrets: secret_hosts,
                jobs,
            });
            self.jobs.truncate(jobs);
            self.clients.clear();
            for _ in 0..r.count(self.config.max_waiters)? {
                let channel = r.word()?;
                let package = r.text(128)?.into();
                let protocol = r.text(32)?.into();
                let mut methods = Vec::new();
                for _ in 0..r.count(3)? {
                    methods.push(r.word()?);
                }
                self.clients.push(Client {
                    channel,
                    package,
                    protocol,
                    methods,
                });
            }
        } else if (JOBS..BLOBS).contains(&key) {
            let query = ArtifactQuery {
                registry_host: r.text(128)?.into(),
                repository: r.text(128)?.into(),
                tag: r.text(64)?.into(),
                kind: kind(r.word()?)?,
                expected_digest: None,
            };
            query.validate().map_err(|_| Error::InvalidData)?;
            let attempt = u8::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
            let source_digest = match r.word()? {
                0 => None,
                hash @ (1 | 2) => Some(BlobDigest {
                    hash_type: if hash == 1 {
                        HashType::Sha256
                    } else {
                        HashType::Blake3
                    },
                    digest: r.bytes(32)?.try_into().map_err(|_| Error::InvalidData)?,
                }),
                _ => return Err(Error::InvalidData),
            };
            let mut waiters = Vec::new();
            for _ in 0..r.count(self.config.max_waiters)? {
                let channel = r.word()?;
                let caller = r.text(128)?.into();
                let hash = r.word()?;
                let expected = if hash == 0 {
                    None
                } else {
                    Some(BlobDigest {
                        hash_type: match hash {
                            1 => HashType::Sha256,
                            2 => HashType::Blake3,
                            _ => return Err(Error::InvalidData),
                        },
                        digest: r.bytes(32)?.try_into().map_err(|_| Error::InvalidData)?,
                    })
                };
                waiters.push(Waiter {
                    channel,
                    caller,
                    expected,
                });
            }
            let index = (key - JOBS) as usize;
            if index > self.jobs.len() || index >= self.config.max_inflight {
                return Err(Error::InvalidData);
            }
            let job = Job {
                query,
                source_digest,
                attempt,
                waiters,
                future: None,
            };
            if index == self.jobs.len() {
                self.jobs.push(job);
            } else {
                self.jobs[index] = job;
            }
        } else if (BLOBS..SECRETS).contains(&key) {
            let digest = r.bytes(32)?.try_into().map_err(|_| Error::InvalidData)?;
            self.blobs.insert(
                digest,
                Blob {
                    handle: r.word()?,
                    length: r.word()?,
                },
            );
        } else if key >= SECRETS {
            let host = r.text(128)?.into();
            self.secrets.insert(
                host,
                SecretHandle {
                    handle: r.word()?,
                    length: r.word()?,
                    sealed: r.flag()?,
                },
            );
        } else {
            return Err(Error::InvalidData);
        }
        r.finish()
    }
    fn quiescence_ready(&self) -> bool {
        self.jobs.iter().all(|job| job.future.is_none())
            && self.cas.borrow().is_none()
            && self.secure.borrow().is_none()
    }
    fn validate(&self) -> Result<(), Error> {
        if self.control.0 == 0
            || self.jobs.len() > self.config.max_inflight
            || self.clients.len() > self.config.max_waiters
            || self.blobs.len() > 128
            || self.secrets.len() > 64
        {
            return Err(Error::InvalidData);
        }
        if let Some(expected) = &self.adoption_inventory {
            if self.jobs.len() != expected.jobs
                || self.blobs.keys().copied().collect::<Vec<_>>() != expected.blobs
                || self.secrets.keys().cloned().collect::<Vec<_>>() != expected.secrets
            {
                return Err(Error::InvalidData);
            }
        }
        let mut channels = std::collections::BTreeSet::new();
        for client in &self.clients {
            if client.channel == 0
                || !channels.insert(client.channel)
                || !matches!(
                    client.protocol.as_str(),
                    "PackageResolver" | "CredentialManager"
                )
                || client.methods.iter().any(|ordinal| {
                    !(1..=if client.protocol == "PackageResolver" {
                        2
                    } else {
                        3
                    })
                        .contains(ordinal)
                })
            {
                return Err(Error::InvalidData);
            }
        }
        if self.jobs.iter().map(|job| job.waiters.len()).sum::<usize>() > self.config.max_waiters {
            return Err(Error::InvalidData);
        }
        for job in &self.jobs {
            job.query.validate().map_err(|_| Error::InvalidData)?;
            if job.attempt > 3 || job.waiters.is_empty() {
                return Err(Error::InvalidData);
            }
            let repo = self
                .config
                .repository(&job.query)
                .ok_or(Error::InvalidData)?;
            for waiter in &job.waiters {
                if !self
                    .config
                    .permits(&waiter.caller, &repo.id(), job.query.kind)
                    || !self.clients.iter().any(|client| {
                        client.channel == waiter.channel
                            && client.package == waiter.caller
                            && client.protocol == "PackageResolver"
                            && client.methods.contains(&if job.source_digest.is_some() {
                                2
                            } else {
                                1
                            })
                    })
                {
                    return Err(Error::InvalidData);
                }
            }
        }
        Ok(())
    }
    fn finish_adoption(&mut self) -> Result<(), Error> {
        self.validate()?;
        for (digest, blob) in &self.blobs {
            let (kind, rights) = bexos_userspace::Memory::object_info(blob.handle)
                .map_err(|_| Error::InvalidData)?;
            let allowed = kernel_fidl::Rights::READ.0
                | kernel_fidl::Rights::MAP.0
                | kernel_fidl::Rights::TRANSFER.0
                | kernel_fidl::Rights::DUPLICATE.0;
            if kind != kernel_fidl::ObjectType::Vmo
                || rights != allowed
                || blob.length == 0
                || blob.length > self.config.max_payload_bytes
            {
                return Err(Error::InvalidData);
            }
            let address =
                bexos_userspace::Memory::map(blob.handle, blob.length, kernel_fidl::Rights::READ.0)
                    .map_err(|_| Error::InvalidData)?;
            let bytes =
                unsafe { core::slice::from_raw_parts(address as *const u8, blob.length as usize) };
            let valid = <[u8; 32]>::from(Sha256::digest(bytes)) == *digest;
            bexos_userspace::Memory::unmap(address, blob.length).map_err(|_| Error::InvalidData)?;
            if !valid {
                return Err(Error::InvalidData);
            }
        }
        self.open_storage().map_err(|_| Error::InvalidData)?;
        crate::wire::restore_secrets(self).map_err(|_| Error::InvalidData)
    }
    fn resources(&self) -> Vec<Resource> {
        let mut handles = vec![
            self.control.0,
            self.migration.map_or(0, |c| c.0),
            self.vfs.0,
            self.network.0,
            self.tls.0,
            self.tee.0,
            self.time.0,
            self.cache_file.0,
            self.state_file.0,
            self.directory.0,
        ];
        handles.extend(self.clients.iter().map(|c| c.channel));
        handles.extend(self.blobs.values().map(|b| b.handle));
        handles.extend(self.secrets.values().map(|s| s.handle));
        handles
            .into_iter()
            .filter(|h| *h != 0)
            .map(Resource::Handle)
            .collect()
    }
    fn activated(&mut self, _: u64) {
        self.adoption_inventory = None;
    }
}
fn kind(value: u64) -> Result<ArtifactKind, Error> {
    match value {
        1 => Ok(ArtifactKind::Application),
        2 => Ok(ArtifactKind::Driver),
        3 => Ok(ArtifactKind::Font),
        4 => Ok(ArtifactKind::Firmware),
        5 => Ok(ArtifactKind::NetworkExtension),
        _ => Err(Error::InvalidData),
    }
}
