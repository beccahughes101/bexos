use super::*;
use alloc::vec::Vec;
use bexos_migration::{
    Error, blob,
    codec::{Decoder, Encoder},
};
use bexos_userspace::live_migration::{Resource, State};
pub const INSTALLER: u64 = 2 << 56;
pub const INPUT: u64 = 3 << 56;
pub const UPLOAD: u64 = 4 << 56;
const UPDATE_MANIFEST: u64 = 5 << 56;
const UPDATE_ARTIFACT: u64 = 6 << 56;
const LOCAL: u64 = (1 << 56) - 1;
pub struct Runtime {
    pub control: Channel,
    pub migration: Option<Channel>,
    pub uart_handle: u64,
    pub uart_va: u64,
    pub input: Vec<u8>,
    pub installer: BufferedTestAppInstaller,
    pub updates: LiveUpdateManager,
    pub platform: KernelPlatformUpdateApplier,
    pub apps: LiveAppManager,
    pub users: LiveUserManager,
    pub tee: LiveTeeManager,
    pub traces: LiveTraceManager,
    pub shells: shell_runtime::ShellRuntime,
}
impl State for Runtime {
    fn empty() -> Self {
        Self {
            control: Channel(0),
            migration: None,
            uart_handle: 0,
            uart_va: 0,
            input: Vec::new(),
            installer: BufferedTestAppInstaller::new(),
            updates: LiveUpdateManager::Unsupported(UnsupportedUpdateManager),
            platform: KernelPlatformUpdateApplier::new(KernelTransport(6)),
            apps: LiveAppManager::Unsupported(UnsupportedAppManager),
            users: LiveUserManager::Unsupported(UnsupportedUserManager),
            tee: LiveTeeManager::Unsupported(UnsupportedTeeManager),
            traces: LiveTraceManager::Unsupported(UnsupportedTraceManager),
            shells: shell_runtime::ShellRuntime::default(),
        }
    }
    fn keys(&self) -> Vec<u64> {
        let mut keys = alloc::vec![0];
        keys.extend(
            self.installer
                .checkpoint_keys()
                .into_iter()
                .map(|k| INSTALLER | k),
        );
        keys.extend(blob::keys(0, &self.input).map(|k| INPUT | k));
        if let LiveAppManager::Proxy(m) = &self.apps {
            if let Some(u) = &m.upload {
                keys.extend(blob::keys(0, &u.archive).map(|k| UPLOAD | k));
            }
        }
        if let LiveUpdateManager::Proxy(m) = &self.updates {
            if let Some(u) = &m.upload {
                keys.extend(blob::keys(0, &u.manifest).map(|k| UPDATE_MANIFEST | k));
                keys.extend(blob::keys(0, &u.artifact).map(|k| UPDATE_ARTIFACT | k));
            }
        }
        keys
    }
    fn encode_record(&self, key: u64) -> Result<Option<Vec<u8>>, Error> {
        match key & !LOCAL {
            UPDATE_MANIFEST | UPDATE_ARTIFACT => {
                return Ok(blob::record(
                    match &self.updates {
                        LiveUpdateManager::Proxy(m) => m.upload.as_ref().map(|u| {
                            if key & !LOCAL == UPDATE_MANIFEST {
                                u.manifest.as_slice()
                            } else {
                                u.artifact.as_slice()
                            }
                        }),
                        _ => None,
                    },
                    key,
                ));
            }
            INSTALLER => return self.installer.checkpoint_record(key & LOCAL),
            INPUT => {
                // Never export a partial credential-bearing request to a replacement.
                // Aborting this snapshot leaves the source running to finish the request.
                let method = self
                    .input
                    .get(12..16)
                    .map(|b| u32::from_le_bytes(b.try_into().unwrap()));
                if !self.input.is_empty()
                    && (method.is_none() || matches!(method, Some(66 | 67 | 69 | 112)))
                {
                    return Err(Error::InvalidData);
                }
                return Ok(blob::record(Some(&self.input), key));
            }
            UPLOAD => {
                return Ok(blob::record(
                    match &self.apps {
                        LiveAppManager::Proxy(m) => m.upload.as_ref().map(|u| u.archive.as_slice()),
                        _ => None,
                    },
                    key,
                ));
            }
            0 if key == 0 => {}
            _ => return Err(Error::InvalidData),
        }
        let mut w = Encoder::new();
        w.word(11);
        w.word(bexos_boot::ARCHITECTURE_ID);
        for n in [
            self.control.0,
            self.migration.map_or(0, |c| c.0),
            self.uart_handle,
            self.uart_va,
            self.input.len() as u64,
            self.platform.commit_pending as u64,
            self.platform.awaiting_completion as u64,
        ] {
            w.word(n);
        }
        match self.platform.completion {
            None => w.word(0),
            Some(Err(())) => w.word(1),
            Some(Ok(g)) => {
                w.word(2);
                w.word(g);
            }
        }
        if let LiveAppManager::Proxy(m) = &self.apps {
            w.word(1);
            w.word(m.channel.0);
            w.word(m.app_manager.map_or(0, |channel| channel.0));
            w.word(m.awaiting_response as u64);
            w.word(m.upload.is_some() as u64);
            if let Some(u) = &m.upload {
                w.word(u.upload_id);
                w.word(u.archive_len as u64);
                w.word(u.archive.len() as u64);
            }
        } else {
            w.word(0);
        }
        if let LiveUserManager::Proxy(m) = &self.users {
            w.word(1);
            w.word(m.channel.0);
            w.word(m.awaiting_response as u64);
        } else {
            w.word(0);
        }
        if let LiveTeeManager::Proxy(m) = &self.tee {
            w.word(1);
            w.word(m.channel.0);
        } else {
            w.word(0);
        }
        if let LiveTraceManager::Proxy(m) = &self.traces {
            w.word(1);
            w.word(m.channel().0);
        } else {
            w.word(0);
        }
        self.shells.encode(&mut w);
        if let LiveUpdateManager::Proxy(m) = &self.updates {
            w.word(1);
            w.word(m.channel.0);
            w.word(m.awaiting_response as u64);
            w.word(m.upload.is_some() as u64);
            if let Some(u) = &m.upload {
                for n in [
                    u.upload_id,
                    u.manifest_len as u64,
                    u.artifact_len as u64,
                    u.manifest.len() as u64,
                    u.artifact.len() as u64,
                ] {
                    w.word(n);
                }
            }
        } else {
            w.word(0);
        }
        Ok(Some(w.finish()))
    }
    fn adopt_record(&mut self, key: u64, bytes: Option<&[u8]>) -> Result<(), Error> {
        match key & !LOCAL {
            UPDATE_MANIFEST | UPDATE_ARTIFACT => {
                return blob::adopt(
                    match &mut self.updates {
                        LiveUpdateManager::Proxy(m) => m.upload.as_mut().map(|u| {
                            if key & !LOCAL == UPDATE_MANIFEST {
                                &mut u.manifest
                            } else {
                                &mut u.artifact
                            }
                        }),
                        _ => None,
                    },
                    key,
                    bytes,
                );
            }
            INSTALLER => return self.installer.adopt_record(key & LOCAL, bytes),
            INPUT => return blob::adopt(Some(&mut self.input), key, bytes),
            UPLOAD => {
                return blob::adopt(
                    match &mut self.apps {
                        LiveAppManager::Proxy(m) => m.upload.as_mut().map(|u| &mut u.archive),
                        _ => None,
                    },
                    key,
                    bytes,
                );
            }
            0 if key == 0 => {}
            _ => return Err(Error::InvalidData),
        }
        let mut r = Decoder::new(bytes.ok_or(Error::InvalidData)?);
        let version = r.word()?;
        if !(1..=11).contains(&version) || (version < 9 && bexos_boot::ARCHITECTURE_ID != 1) {
            return Err(Error::UnsupportedVersion);
        }
        if version >= 9 && r.word()? != bexos_boot::ARCHITECTURE_ID {
            return Err(Error::InvalidData);
        }
        self.control = Channel(r.word()?);
        self.migration = Some(Channel(r.word()?));
        self.uart_handle = r.word()?;
        self.uart_va = r.word()?;
        self.input.resize(r.count(1024 * 1024)?, 0);
        self.platform.commit_pending = r.flag()?;
        self.platform.awaiting_completion = r.flag()?;
        self.platform.completion = match r.word()? {
            0 => None,
            1 => Some(Err(())),
            2 => Some(Ok(r.word()?)),
            _ => return Err(Error::InvalidData),
        };
        if r.flag()? {
            let channel = Channel(r.word()?);
            let app_manager = if version >= 4 {
                match r.word()? {
                    0 => None,
                    raw => Some(Channel(raw)),
                }
            } else {
                None
            };
            let awaiting_response = version >= 8 && r.flag()?;
            let old = core::mem::replace(
                &mut self.apps,
                LiveAppManager::Unsupported(UnsupportedAppManager),
            );
            let mut manager = match old {
                LiveAppManager::Proxy(m) if m.channel.0 == channel.0 => m,
                _ => LifecycleAppManager::new(channel, app_manager),
            };
            manager.app_manager = app_manager;
            manager.awaiting_response = awaiting_response;
            let old = manager.upload.take();
            if r.flag()? {
                let upload_id = r.word()?;
                let archive_len = r.count(32 * 1024 * 1024)?;
                let len = r.count(archive_len)?;
                let mut archive = old
                    .filter(|u| u.upload_id == upload_id)
                    .map_or_else(Vec::new, |u| u.archive);
                archive.resize(len, 0);
                manager.upload = Some(LifecycleUpload {
                    upload_id,
                    archive_len,
                    archive,
                });
            }
            self.apps = LiveAppManager::Proxy(manager);
        } else {
            self.apps = LiveAppManager::Unsupported(UnsupportedAppManager);
        }
        if version >= 2 && r.flag()? {
            let mut manager = UserServiceManager::new(Channel(r.word()?));
            manager.awaiting_response = version >= 10 && r.flag()?;
            self.users = LiveUserManager::Proxy(manager);
        } else {
            self.users = LiveUserManager::Unsupported(UnsupportedUserManager);
        }
        if version >= 3 && r.flag()? {
            self.tee = LiveTeeManager::Proxy(TeeServiceManager::from_client(Channel(r.word()?)));
        } else {
            self.tee = LiveTeeManager::Unsupported(UnsupportedTeeManager);
        }
        if version >= 5 && r.flag()? {
            self.traces = LiveTraceManager::Proxy(TracedTraceManager::new(Channel(r.word()?)));
        } else if version >= 5 {
            self.traces = LiveTraceManager::Unsupported(UnsupportedTraceManager);
        }
        if version >= 6 {
            self.shells.adopt(&mut r)?;
        }
        if version >= 7 {
            if r.flag()? {
                let channel = Channel(r.word()?);
                let awaiting_response = version >= 11 && r.flag()?;
                let old = core::mem::replace(
                    &mut self.updates,
                    LiveUpdateManager::Unsupported(UnsupportedUpdateManager),
                );
                let mut manager = match old {
                    LiveUpdateManager::Proxy(m) if m.channel.0 == channel.0 => m,
                    _ => UpdateServiceManager {
                        channel,
                        awaiting_response,
                        upload: None,
                    },
                };
                manager.awaiting_response = awaiting_response;
                let old_upload = manager.upload.take();
                if r.flag()? {
                    let upload_id = r.word()?;
                    let manifest_len = r.count(65536)?;
                    let artifact_len = r.count(64 * 1024 * 1024)?;
                    let mlen = r.count(manifest_len)?;
                    let alen = r.count(artifact_len)?;
                    let mut upload = old_upload.filter(|u| u.upload_id == upload_id).unwrap_or(
                        ProxiedUpdateUpload {
                            upload_id,
                            manifest_len,
                            artifact_len,
                            manifest: Vec::new(),
                            artifact: Vec::new(),
                        },
                    );
                    upload.manifest_len = manifest_len;
                    upload.artifact_len = artifact_len;
                    upload.manifest.resize(mlen, 0);
                    upload.artifact.resize(alen, 0);
                    manager.upload = Some(upload);
                }
                self.updates = LiveUpdateManager::Proxy(manager);
            } else {
                self.updates = LiveUpdateManager::Unsupported(UnsupportedUpdateManager);
            }
        }
        r.finish()
    }
    fn validate(&self) -> Result<(), Error> {
        if self.control.0 == 0
            || self.migration.is_none()
            || self.uart_handle == 0
            || self.uart_va % 4096 != 0
            || self.platform.commit_pending
            || self.platform.awaiting_completion
        {
            return Err(Error::InvalidData);
        }
        Ok(())
    }
    fn resources(&self) -> Vec<Resource> {
        let mut out = alloc::vec![Resource::Handle(self.control.0)];
        if self.uart_va == 0 {
            out.push(Resource::Handle(self.uart_handle));
        } else {
            out.push(Resource::Mapping {
                handle: self.uart_handle,
                offset: 0,
                va: self.uart_va,
                size: 4096,
                rights: 6,
            });
        }
        if let Some(c) = self.migration {
            out.push(Resource::Handle(c.0));
        }
        if let LiveAppManager::Proxy(m) = &self.apps {
            out.push(Resource::Handle(m.channel.0));
            if let Some(channel) = m.app_manager {
                out.push(Resource::Handle(channel.0));
            }
        }
        if let LiveUserManager::Proxy(m) = &self.users {
            out.push(Resource::Handle(m.channel.0));
        }
        if let LiveTeeManager::Proxy(m) = &self.tee {
            out.push(Resource::Handle(m.channel.0));
        }
        if let LiveTraceManager::Proxy(m) = &self.traces {
            out.push(Resource::Handle(m.channel().0));
        }
        if let LiveUpdateManager::Proxy(m) = &self.updates {
            out.push(Resource::Handle(m.channel.0));
        }
        out.extend(self.shells.handles().into_iter().map(Resource::Handle));
        out
    }
    fn activated(&mut self, generation: u64) {
        self.shells.renew();
        log(&alloc::format!(
            "debugd: adopted generation={generation}; UART connection and partial input retained\n"
        ));
    }
}
