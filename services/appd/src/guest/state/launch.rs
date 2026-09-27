use super::*;
#[derive(Clone)]
pub struct LaunchRecord {
    pub package: String,
    pub process: String,
    pub process_handle: u64,
    pub component_job_handle: u64,
    pub controller_handle: u64,
    pub events_handle: u64,
    pub native_host_job_handle: u64,
    pub native_host_process_handle: u64,
    pub native_host_space_handle: u64,
    pub native_host_thread_handle: u64,
    pub runner_ready: bool,
    pub runner_stopped: bool,
    pub stop_deadline_ns: u64,
    pub stop_exit_code: i32,
    pub runner_provider: String,
    pub runner_provider_version: String,
    pub runner_provider_path: String,
    pub runner_provider_signer: String,
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
        for n in [
            self.process_handle,
            self.space_handle,
            self.thread_handle,
            self.manager,
            self.progress,
        ] {
            w.word(n);
        }
        w.word(self.uid);
        w.word(self.job_token);
        w.text(&self.instance_id);
        w.word(self.component_job_handle);
        w.word(self.controller_handle);
        w.word(self.events_handle);
        w.word(self.runner_ready as u64);
        w.word(self.runner_stopped as u64);
        w.word(self.stop_deadline_ns);
        w.word(self.stop_exit_code as i64 as u64);
        w.text(&self.runner_provider);
        w.text(&self.runner_provider_path);
        w.text(&self.runner_provider_signer);
        w.word(self.native_host_job_handle);
        w.word(self.native_host_process_handle);
        w.word(self.native_host_space_handle);
        w.word(self.native_host_thread_handle);
        // Appended for checkpoint compatibility with pre-RFC-72 launch
        // records; older decoders ignore this tail and older records default
        // the provider version to empty.
        w.text(&self.runner_provider_version);
        w.finish()
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut r = Decoder::new(bytes);
        let mut s = Self {
            package: r.text(128)?.into(),
            process: r.text(64)?.into(),
            process_handle: r.word()?,
            component_job_handle: 0,
            controller_handle: 0,
            events_handle: 0,
            native_host_job_handle: 0,
            native_host_process_handle: 0,
            native_host_space_handle: 0,
            native_host_thread_handle: 0,
            runner_ready: false,
            runner_stopped: false,
            stop_deadline_ns: 0,
            stop_exit_code: -1,
            runner_provider: String::new(),
            runner_provider_version: String::new(),
            runner_provider_path: String::new(),
            runner_provider_signer: String::new(),
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
        if let Ok(component_job_handle) = r.word() {
            s.component_job_handle = component_job_handle;
        }
        if let Ok(controller_handle) = r.word() {
            s.controller_handle = controller_handle;
        }
        if let Ok(events_handle) = r.word() {
            s.events_handle = events_handle;
        }
        if let Ok(runner_ready) = r.word() {
            s.runner_ready = runner_ready != 0;
        }
        if let Ok(runner_stopped) = r.word() {
            s.runner_stopped = runner_stopped != 0;
        }
        if let Ok(stop_deadline_ns) = r.word() {
            s.stop_deadline_ns = stop_deadline_ns;
        }
        if let Ok(stop_exit_code) = r.word() {
            s.stop_exit_code = stop_exit_code as i64 as i32;
        }
        if let Ok(value) = r.text(128) {
            s.runner_provider = value.into();
        }
        if let Ok(value) = r.text(256) {
            s.runner_provider_path = value.into();
        }
        if let Ok(value) = r.text(128) {
            s.runner_provider_signer = value.into();
        }
        if let Ok(value) = r.word() {
            s.native_host_job_handle = value;
        }
        if let Ok(value) = r.word() {
            s.native_host_process_handle = value;
        }
        if let Ok(value) = r.word() {
            s.native_host_space_handle = value;
        }
        if let Ok(value) = r.word() {
            s.native_host_thread_handle = value;
        }
        if let Ok(value) = r.text(64) {
            s.runner_provider_version = value.into();
        }
        r.finish()?;
        if s.process_handle == 0 || s.manager == 0 {
            return Err(Error::InvalidData);
        }
        Ok(s)
    }
    pub fn handles(&self) -> [u64; 12] {
        [
            self.process_handle,
            self.component_job_handle,
            self.controller_handle,
            self.events_handle,
            self.native_host_job_handle,
            self.native_host_process_handle,
            self.native_host_space_handle,
            self.native_host_thread_handle,
            self.space_handle,
            self.thread_handle,
            self.manager,
            self.progress,
        ]
    }
}
