use super::*;
use bexos_migration::{
    Error, blob,
    codec::{Decoder, Encoder},
};
fn resize(mut bytes: Vec<u8>, len: usize) -> Vec<u8> {
    bytes.resize(len, 0);
    bytes
}
impl BufferedUpdateManager {
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
            if let Some(b) = self.stream(i) {
                keys.extend(blob::keys(i, b));
            }
        }
        keys
    }
    pub fn checkpoint_record(&self, key: u64) -> Result<Option<Vec<u8>>, Error> {
        if key != 0 {
            return Ok(blob::record(self.stream(key >> 32).map(Vec::as_slice), key));
        }
        let mut w = Encoder::new();
        w.word(1);
        w.word(self.minimum_generation);
        w.word(self.pending_platform_generation.unwrap_or(0));
        w.word(self.last_status.status as i64 as u64);
        w.text(&self.last_status.message);
        w.word(self.upload.is_some() as u64);
        if let Some(u) = &self.upload {
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
        w.word(self.staged.is_some() as u64);
        if let Some(u) = &self.staged {
            w.word(u.manifest.len() as u64);
            w.word(u.artifact.len() as u64);
        }
        Ok(Some(w.finish()))
    }
    pub fn adopt_record(&mut self, key: u64, bytes: Option<&[u8]>) -> Result<(), Error> {
        if key != 0 {
            return blob::adopt(self.stream_mut(key >> 32), key, bytes);
        }
        let mut r = Decoder::new(bytes.ok_or(Error::InvalidData)?);
        if r.word()? != 1 {
            return Err(Error::UnsupportedVersion);
        }
        self.minimum_generation = r.word()?;
        let pending = r.word()?;
        self.pending_platform_generation = (pending != 0).then_some(pending);
        self.last_status = DebugStatusResponse {
            status: r.word()? as i32,
            message: r.text(4096)?.into(),
        };
        let old = self.upload.take();
        if r.flag()? {
            let upload_id = r.word()?;
            let manifest_len = r.count(65536)?;
            let artifact_len = r.count(32 * 1024 * 1024)?;
            let ml = r.count(manifest_len)?;
            let al = r.count(artifact_len)?;
            let (m, a) = old
                .filter(|u| u.upload_id == upload_id)
                .map_or_else(|| (Vec::new(), Vec::new()), |u| (u.manifest, u.artifact));
            self.upload = Some(BufferedUpdateUpload {
                upload_id,
                manifest_len,
                artifact_len,
                manifest: resize(m, ml),
                artifact: resize(a, al),
            });
        }
        let old = self.staged.take();
        if r.flag()? {
            let ml = r.count(65536)?;
            let al = r.count(32 * 1024 * 1024)?;
            let (m, a) = old.map_or_else(|| (Vec::new(), Vec::new()), |u| (u.manifest, u.artifact));
            self.staged = Some(StagedUpdate {
                manifest: resize(m, ml),
                artifact: resize(a, al),
            });
        }
        r.finish()
    }
}
impl BufferedTestAppInstaller {
    fn stream(&self, i: u64) -> Option<&Vec<u8>> {
        match i {
            1 => self.upload.as_ref().map(|u| &u.manifest),
            2 => self.upload.as_ref().map(|u| &u.elf),
            3 => self.installed.as_ref().map(|u| &u.manifest),
            4 => self.installed.as_ref().map(|u| &u.elf),
            _ => None,
        }
    }
    fn stream_mut(&mut self, i: u64) -> Option<&mut Vec<u8>> {
        match i {
            1 => self.upload.as_mut().map(|u| &mut u.manifest),
            2 => self.upload.as_mut().map(|u| &mut u.elf),
            3 => self.installed.as_mut().map(|u| &mut u.manifest),
            4 => self.installed.as_mut().map(|u| &mut u.elf),
            _ => None,
        }
    }
    pub fn checkpoint_keys(&self) -> Vec<u64> {
        let mut keys = alloc::vec![0];
        for i in 1..=4 {
            if let Some(b) = self.stream(i) {
                keys.extend(blob::keys(i, b));
            }
        }
        keys
    }
    pub fn checkpoint_record(&self, key: u64) -> Result<Option<Vec<u8>>, Error> {
        if key != 0 {
            return Ok(blob::record(self.stream(key >> 32).map(Vec::as_slice), key));
        }
        let mut w = Encoder::new();
        w.word(1);
        w.word(self.upload.is_some() as u64);
        if let Some(u) = &self.upload {
            w.word(u.upload_id);
            w.text(&u.package_id);
            for n in [u.manifest_len, u.elf_len, u.manifest.len(), u.elf.len()] {
                w.word(n as u64);
            }
        }
        w.word(self.installed.is_some() as u64);
        if let Some(u) = &self.installed {
            w.text(&u.package_id);
            w.word(u.manifest.len() as u64);
            w.word(u.elf.len() as u64);
        }
        Ok(Some(w.finish()))
    }
    pub fn adopt_record(&mut self, key: u64, bytes: Option<&[u8]>) -> Result<(), Error> {
        if key != 0 {
            return blob::adopt(self.stream_mut(key >> 32), key, bytes);
        }
        let mut r = Decoder::new(bytes.ok_or(Error::InvalidData)?);
        if r.word()? != 1 {
            return Err(Error::UnsupportedVersion);
        }
        let old = self.upload.take();
        if r.flag()? {
            let upload_id = r.word()?;
            let package_id = r.text(128)?.into();
            let manifest_len = r.count(65536)?;
            let elf_len = r.count(32 * 1024 * 1024)?;
            let ml = r.count(manifest_len)?;
            let el = r.count(elf_len)?;
            let (m, e) = old
                .filter(|u| u.upload_id == upload_id)
                .map_or_else(|| (Vec::new(), Vec::new()), |u| (u.manifest, u.elf));
            self.upload = Some(BufferedUpload {
                upload_id,
                package_id,
                manifest_len,
                elf_len,
                manifest: resize(m, ml),
                elf: resize(e, el),
            });
        }
        let old = self.installed.take();
        if r.flag()? {
            let package_id = r.text(128)?.into();
            let ml = r.count(65536)?;
            let el = r.count(32 * 1024 * 1024)?;
            let (m, e) = old.map_or_else(|| (Vec::new(), Vec::new()), |u| (u.manifest, u.elf));
            self.installed = Some(InstalledTestApp {
                package_id,
                manifest: resize(m, ml),
                elf: resize(e, el),
            });
        }
        r.finish()
    }
}
