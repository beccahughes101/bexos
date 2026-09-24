use super::*;
#[derive(Clone)]
pub struct LaunchRecord {
    pub package: String,
    pub process: String,
    pub process_handle: u64,
    pub space_handle: u64,
    pub thread_handle: u64,
    pub manager: u64,
    pub progress: u64,
    pub uid: u64,
    pub job_token: u64,
    /// Stable appd instance identity. Empty is the legacy singleton instance.
    pub instance_id: String,
}
impl LaunchRecord {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Encoder::new();
        w.text(&self.package);
        w.text(&self.process);
        for n in self.handles() {
            w.word(n);
        }
        w.word(self.uid);
        w.word(self.job_token);
        w.text(&self.instance_id);
        w.finish()
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut r = Decoder::new(bytes);
        let mut s = Self {
            package: r.text(128)?.into(),
            process: r.text(64)?.into(),
            process_handle: r.word()?,
            space_handle: r.word()?,
            thread_handle: r.word()?,
            manager: r.word()?,
            progress: r.word()?,
            uid: r.word()?,
            job_token: 0,
            instance_id: String::new(),
        };
        if let Ok(job_token) = r.word() {
            s.job_token = job_token;
        }
        if let Ok(instance_id) = r.text(128) {
            s.instance_id = instance_id.into();
        }
        r.finish()?;
        if s.process_handle == 0 || s.manager == 0 {
            return Err(Error::InvalidData);
        }
        Ok(s)
    }
    pub fn handles(&self) -> [u64; 5] {
        [
            self.process_handle,
            self.space_handle,
            self.thread_handle,
            self.manager,
            self.progress,
        ]
    }
}
