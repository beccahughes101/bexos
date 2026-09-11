use alloc::vec::Vec;
use bexos_migration::{
    Error, blob,
    codec::{Decoder, Encoder},
};
use bexos_userspace::live_migration::{Resource, State};

use crate::service::{FeedUpdateManager, PlatformUpdateApplier, Runtime, UpdateService};

pub const UPDATES: u64 = 1 << 56;
pub const FEED: u64 = 2 << 56;
const LOCAL: u64 = (1 << 56) - 1;

impl State for Runtime {
    fn empty() -> Self {
        Self {
            control: bexos_userspace::Channel(0),
            migration: None,
            manager: UpdateService::new(),
            clients: Vec::new(),
            appd: None,
            appd_awaiting_response: false,
            app_manager: None,
            tee: None,
            platform: PlatformUpdateApplier::new(),
            feed: FeedUpdateManager::new(),
        }
    }

    fn keys(&self) -> Vec<u64> {
        let mut keys = alloc::vec![0];
        keys.extend(
            self.manager
                .checkpoint_keys()
                .into_iter()
                .map(|k| UPDATES | k),
        );
        let feed = self.feed.checkpoint();
        keys.extend(blob::keys(0, &feed).map(|k| FEED | k));
        keys
    }

    fn encode_record(&self, key: u64) -> Result<Option<Vec<u8>>, Error> {
        match key & !LOCAL {
            UPDATES => self.manager.checkpoint_record(key & LOCAL),
            FEED => {
                let feed = self.feed.checkpoint();
                Ok(blob::record(Some(&feed), key & LOCAL))
            }
            0 if key == 0 => {
                let mut w = Encoder::new();
                w.word(2);
                w.word(self.control.0);
                w.word(self.migration.map_or(0, |c| c.0));
                w.word(self.appd.map_or(0, |c| c.0));
                w.word(self.appd_awaiting_response as u64);
                w.word(self.app_manager.map_or(0, |c| c.0));
                w.word(self.tee.map_or(0, |c| c.0));
                w.word(self.platform.commit_pending as u64);
                w.word(self.platform.awaiting_completion as u64);
                match self.platform.completion {
                    None => w.word(0),
                    Some(Err(())) => w.word(1),
                    Some(Ok(generation)) => {
                        w.word(2);
                        w.word(generation);
                    }
                }
                Ok(Some(w.finish()))
            }
            _ => Err(Error::InvalidData),
        }
    }

    fn adopt_record(&mut self, key: u64, bytes: Option<&[u8]>) -> Result<(), Error> {
        match key & !LOCAL {
            UPDATES => self.manager.adopt_record(key & LOCAL, bytes),
            FEED => {
                let mut feed = self.feed.checkpoint();
                blob::adopt(Some(&mut feed), key & LOCAL, bytes)?;
                self.feed
                    .adopt_checkpoint(&feed)
                    .map_err(|_| Error::InvalidData)
            }
            0 if key == 0 => {
                let mut r = Decoder::new(bytes.ok_or(Error::InvalidData)?);
                let version = r.word()?;
                if !(1..=2).contains(&version) {
                    return Err(Error::UnsupportedVersion);
                }
                self.control = bexos_userspace::Channel(r.word()?);
                let migration = r.word()?;
                self.migration = (migration != 0).then_some(bexos_userspace::Channel(migration));
                let appd = r.word()?;
                self.appd = (appd != 0).then_some(bexos_userspace::Channel(appd));
                self.appd_awaiting_response = version >= 2 && r.flag()?;
                let app_manager = r.word()?;
                self.app_manager =
                    (app_manager != 0).then_some(bexos_userspace::Channel(app_manager));
                let tee = r.word()?;
                self.tee = (tee != 0).then_some(bexos_userspace::Channel(tee));
                self.platform.commit_pending = r.flag()?;
                self.platform.awaiting_completion = r.flag()?;
                self.platform.completion = match r.word()? {
                    0 => None,
                    1 => Some(Err(())),
                    2 => Some(Ok(r.word()?)),
                    _ => return Err(Error::InvalidData),
                };
                r.finish()
            }
            _ => Err(Error::InvalidData),
        }
    }

    fn validate(&self) -> Result<(), Error> {
        if self.control.0 == 0
            || self.migration.is_none()
            || (self.appd_awaiting_response && self.appd.is_none())
        {
            Err(Error::InvalidData)
        } else {
            Ok(())
        }
    }

    fn resources(&self) -> Vec<Resource> {
        let mut resources = alloc::vec![Resource::Handle(self.control.0)];
        if let Some(channel) = self.migration {
            resources.push(Resource::Handle(channel.0));
        }
        if let Some(channel) = self.appd {
            resources.push(Resource::Handle(channel.0));
        }
        if let Some(channel) = self.app_manager {
            resources.push(Resource::Handle(channel.0));
        }
        if let Some(channel) = self.tee {
            resources.push(Resource::Handle(channel.0));
        }
        resources
    }

    fn activated(&mut self, generation: u64) {
        bexos_userspace::log(&alloc::format!(
            "updated: adopted generation={generation}\n"
        ));
    }
}

fn resize(mut bytes: Vec<u8>, len: usize) -> Vec<u8> {
    bytes.resize(len, 0);
    bytes
}

impl UpdateService {
    fn stream(&self, i: u64) -> Option<&Vec<u8>> {
        match i {
            1 => self.upload.as_ref().map(|u| &u.manifest),
            2 => self.upload.as_ref().map(|u| &u.artifact),
            3 => self.staged.as_ref().map(|u| &u.manifest),
            4 => self.staged.as_ref().map(|u| &u.artifact),
            _ => None,
        }
    }

    fn stream_mut(&mut self, i: u64) -> Option<&mut Vec<u8>> {
        match i {
            1 => self.upload.as_mut().map(|u| &mut u.manifest),
            2 => self.upload.as_mut().map(|u| &mut u.artifact),
            3 => self.staged.as_mut().map(|u| &mut u.manifest),
            4 => self.staged.as_mut().map(|u| &mut u.artifact),
            _ => None,
        }
    }

    pub fn checkpoint_keys(&self) -> Vec<u64> {
        let mut keys = alloc::vec![0];
        for i in 1..=4 {
            if let Some(bytes) = self.stream(i) {
                keys.extend(blob::keys(i, bytes));
            }
        }
        keys
    }

    pub fn checkpoint_record(&self, key: u64) -> Result<Option<Vec<u8>>, Error> {
        if key != 0 {
            return Ok(blob::record(self.stream(key >> 32).map(Vec::as_slice), key));
        }
        let mut w = Encoder::new();
        w.word(2);
        w.word(self.minimum_generation);
        w.word(self.pending_platform_generation.unwrap_or(0));
        if let Some(pending) = &self.pending_service_generation {
            w.word(1);
            w.text(&pending.target);
            w.word(pending.generation);
        } else {
            w.word(0);
        }
        w.word(self.last_status as i64 as u64);
        w.text(&self.last_message);
        w.word(self.upload.is_some() as u64);
        if let Some(upload) = &self.upload {
            for n in [
                upload.upload_id,
                upload.manifest_len as u64,
                upload.artifact_len as u64,
                upload.manifest.len() as u64,
                upload.artifact.len() as u64,
            ] {
                w.word(n);
            }
        }
        w.word(self.staged.is_some() as u64);
        if let Some(staged) = &self.staged {
            w.word(staged.manifest.len() as u64);
            w.word(staged.artifact.len() as u64);
        }
        Ok(Some(w.finish()))
    }

    pub fn adopt_record(&mut self, key: u64, bytes: Option<&[u8]>) -> Result<(), Error> {
        if key != 0 {
            return blob::adopt(self.stream_mut(key >> 32), key, bytes);
        }
        let mut r = Decoder::new(bytes.ok_or(Error::InvalidData)?);
        let version = r.word()?;
        if !(1..=2).contains(&version) {
            return Err(Error::UnsupportedVersion);
        }
        self.minimum_generation = r.word()?;
        let pending = r.word()?;
        self.pending_platform_generation = (pending != 0).then_some(pending);
        if version >= 2 && r.flag()? {
            self.pending_service_generation = Some(crate::service::PendingServiceGeneration {
                target: r.text(128)?.into(),
                generation: r.word()?,
            });
        } else {
            self.pending_service_generation = None;
        }
        self.last_status = r.word()? as i32;
        self.last_message = r.text(4096)?.into();
        let old = self.upload.take();
        if r.flag()? {
            let upload_id = r.word()?;
            let manifest_len = r.count(65536)?;
            let artifact_len = r.count(64 * 1024 * 1024)?;
            let manifest_actual = r.count(manifest_len)?;
            let artifact_actual = r.count(artifact_len)?;
            let (manifest, artifact) = old
                .filter(|u| u.upload_id == upload_id)
                .map_or_else(|| (Vec::new(), Vec::new()), |u| (u.manifest, u.artifact));
            self.upload = Some(crate::service::Upload {
                upload_id,
                manifest_len,
                artifact_len,
                manifest: resize(manifest, manifest_actual),
                artifact: resize(artifact, artifact_actual),
            });
        }
        let old = self.staged.take();
        if r.flag()? {
            let manifest_actual = r.count(65536)?;
            let artifact_actual = r.count(64 * 1024 * 1024)?;
            let (manifest, artifact) =
                old.map_or_else(|| (Vec::new(), Vec::new()), |u| (u.manifest, u.artifact));
            self.staged = Some(crate::service::Staged {
                manifest: resize(manifest, manifest_actual),
                artifact: resize(artifact, artifact_actual),
            });
        }
        r.finish()
    }
}
