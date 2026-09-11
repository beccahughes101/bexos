//! One bounded rendering worker. Driver IPC, shader compilation and Vulkan
//! completion waits never run on the compositor's input/presentation thread.
//! All renderer objects are destroyed before the stopped state is published.
use bexos_flatland::{Damage, Surface};
use bexos_flatland_render::{
    GpuRenderer,
    composition::{Composition, Layer},
};
use bexos_graphics_runtime::{self as rt, Mapping};
use bexos_userspace::Channel;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    time::Duration,
};
const INITIALIZING: u8 = 0;
const READY: u8 = 1;
const STOPPED: u8 = 2;
#[path = "worker_warmup.rs"]
mod warmup;
pub struct Job {
    pub layers: Vec<Layer>,
    pub output: Mapping,
    pub surface: Surface,
    pub damage: Damage,
    pub repair: Damage,
}
impl Job {
    fn validate(&self) -> Result<(), String> {
        self.surface
            .validate(self.output.size)
            .map_err(|_| "invalid output mapping")?;
        self.damage
            .validate(self.surface)
            .map_err(|_| "invalid presentation damage")?;
        self.repair
            .validate(self.surface)
            .map_err(|_| "invalid readback repair")?;
        if self.layers.is_empty() {
            return Err("missing Vello scene".into());
        }
        Ok(())
    }
}
pub struct Completed {
    pub job: Job,
    pub result: Result<(), String>,
    pub elapsed_us: u64,
    pub cpu_ns: Option<u64>,
    pub submission_us: u64,
    pub gpu_interval_ns: Option<u64>,
}
enum Message {
    Complete(Completed),
    Failed(String),
}
pub struct Worker {
    recovery_timeout_us: Arc<AtomicU64>,
    sender: SyncSender<Job>,
    receiver: Receiver<Message>,
    state: Arc<AtomicU8>,
    stop: Arc<AtomicBool>,
    thread: usize,
    wakeup: Channel,
    failed: Arc<AtomicBool>,
}
struct Start {
    software_timeout_ms: u32,
    recovery_timeout_us: Arc<AtomicU64>,
    profile: bool,
    endpoint: Channel,
    jobs: Receiver<Job>,
    replies: SyncSender<Message>,
    state: Arc<AtomicU8>,
    stop: Arc<AtomicBool>,
    wakeup: Channel,
    failed: Arc<AtomicBool>,
}
unsafe extern "C" {
    fn pthread_create(
        thread: *mut usize,
        attributes: *const core::ffi::c_void,
        entry: extern "C" fn(*mut core::ffi::c_void) -> *mut core::ffi::c_void,
        argument: *mut core::ffi::c_void,
    ) -> i32;
    fn pthread_join(thread: usize, value: *mut *mut core::ffi::c_void) -> i32;
    fn pthread_detach(thread: usize) -> i32;
}
impl Worker {
    pub fn start(endpoint: Channel) -> Result<Self, String> {
        Self::start_profiled(endpoint, false)
    }
    pub fn start_profiled(endpoint: Channel, profile: bool) -> Result<Self, String> {
        Self::start_configured(endpoint, profile, 2000)
    }
    pub fn start_configured(
        endpoint: Channel,
        profile: bool,
        software_timeout_ms: u32,
    ) -> Result<Self, String> {
        bexos_flatland_render::recovery::timeout_us(true, software_timeout_ms)
            .ok_or("invalid software Vulkan recovery timeout")?;
        let recovery_timeout_us = Arc::new(AtomicU64::new(
            bexos_flatland_render::recovery::HARDWARE_TIMEOUT_US,
        ));
        let (wake_tx, wake_rx) =
            Channel::pair().map_err(|e| format!("GPU completion channel: {e:?}"))?;
        let (sender, jobs) = mpsc::sync_channel(1);
        let (replies, receiver) = mpsc::sync_channel(2);
        let state = Arc::new(AtomicU8::new(INITIALIZING));
        let stop = Arc::new(AtomicBool::new(false));
        let failed = Arc::new(AtomicBool::new(false));
        let start = Box::new(Start {
            software_timeout_ms,
            recovery_timeout_us: recovery_timeout_us.clone(),
            profile,
            endpoint,
            jobs,
            replies,
            state: state.clone(),
            stop: stop.clone(),
            wakeup: wake_tx,
            failed: failed.clone(),
        });
        let argument = Box::into_raw(start);
        let mut thread = 0;
        // The BexOS pthread default has a real allocated 2 MiB stack and no
        // fictitious Unix guard-page mapping. Its TLS and join are kernel-backed.
        let status =
            unsafe { pthread_create(&mut thread, core::ptr::null(), entry, argument.cast()) };
        if status != 0 {
            rt::close(&[wake_tx.0, wake_rx.0]);
            unsafe {
                drop(Box::from_raw(argument));
            }
            return Err(format!("GPU worker creation: {status}"));
        }
        Ok(Self {
            recovery_timeout_us,
            sender,
            receiver,
            state,
            stop,
            thread,
            wakeup: wake_rx,
            failed,
        })
    }
    pub fn ready(&self) -> bool {
        self.state.load(Ordering::Acquire) == READY && !self.stop.load(Ordering::Acquire)
    }
    pub fn recovery_timeout_us(&self) -> u64 {
        self.recovery_timeout_us.load(Ordering::Acquire)
    }
    pub fn wakeup(&self) -> Channel {
        self.wakeup
    }
    pub fn failed(&self) -> bool {
        self.failed.load(Ordering::Acquire)
    }
    pub fn stopped(&self) -> bool {
        self.state.load(Ordering::Acquire) == STOPPED
    }
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Release);
    }
    pub fn submit(&self, job: Job) -> Result<(), Job> {
        self.sender.try_send(job).map_err(|e| match e {
            mpsc::TrySendError::Full(job) | mpsc::TrySendError::Disconnected(job) => job,
        })
    }
    pub fn poll(&self) -> Result<Option<Completed>, String> {
        for _ in 0..4 {
            match self.wakeup.try_recv() {
                Ok(message) => rt::close(&message.handles),
                Err(_) => break,
            }
        }
        match self.receiver.try_recv() {
            Ok(Message::Complete(job)) => Ok(Some(job)),
            Ok(Message::Failed(error)) => Err(error),
            Err(mpsc::TryRecvError::Empty | mpsc::TryRecvError::Disconnected) => Ok(None),
        }
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.stop();
        // Normal migration reaches STOPPED first; failed initialization can also
        // be reaped. Dropping a running worker never blocks the service loop.
        unsafe {
            if self.stopped() {
                pthread_join(self.thread, core::ptr::null_mut());
            } else {
                pthread_detach(self.thread);
            }
        }
        rt::close(&[self.wakeup.0]);
    }
}
extern "C" fn entry(argument: *mut core::ffi::c_void) -> *mut core::ffi::c_void {
    let mut start = unsafe { Box::from_raw(argument.cast::<Start>()) };
    let result = run(&mut start);
    if let Err(error) = result {
        start.failed.store(true, Ordering::Release);
        let _ = start.replies.try_send(Message::Failed(error));
    }
    start.state.store(STOPPED, Ordering::Release);
    let _ = start.wakeup.send(&[], &[]);
    rt::close(&[start.wakeup.0]);
    core::ptr::null_mut()
}
fn run(start: &mut Start) -> Result<(), String> {
    let caps =
        rt::discovery::read(&mut start.endpoint).map_err(|e| format!("GPU discovery: {e:?}"))?;
    if !bexos_virtio_gpu_protocol::transport::supported(&caps.gpu) {
        return Err("Venus is unavailable".into());
    }
    let capset = rt::discovery::capset(&mut start.endpoint, 4, 0)
        .map_err(|e| format!("Venus capset: {e:?}"))?;
    bexos_venus_transport::install(start.endpoint, capset)
        .map_err(|_| "Venus transport already installed")?;
    let result = render_loop(start);
    // A failed release is retained by the transport/driver; never pretend that
    // live GPU leases were destroyed or reset the visible scanout to recover.
    let uninstalled =
        bexos_venus_transport::uninstall().map_err(|e| format!("Venus cache retirement: {e}"));
    if uninstalled.is_err() {
        let _ = bexos_venus_transport::abandon_after_device_destroyed();
    }
    result.and(uninstalled.map(|_| ()))
}
struct Target {
    surface: Surface,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    readback: wgpu::Buffer,
    stride: u32,
    completion: Arc<AtomicU8>,
    composition: Option<Composition>,
    timestamps: Option<bexos_flatland_render::timing::Timestamps>,
    submission_us: u64,
    gpu_interval_ns: Option<u64>,
}
impl Target {
    fn new(gpu: &super::Device, surface: Surface) -> Result<Self, String> {
        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("scened Vello target"),
            size: wgpu::Extent3d {
                width: surface.width,
                height: surface.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        gpu.errors.check()?;
        let view = texture.create_view(&Default::default());
        let stride = (surface.width * 4).div_ceil(256) * 256;
        let timestamps = bexos_flatland_render::timing::Timestamps::new(&gpu.device, &gpu.queue);
        let readback = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("scened retained readback"),
            size: stride as u64 * surface.height as u64 + if timestamps.is_some() { 16 } else { 0 },
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        gpu.errors.check()?;
        Ok(Self {
            surface,
            texture,
            view,
            readback,
            stride,
            completion: Arc::new(AtomicU8::new(0)),
            composition: None,
            timestamps,
            submission_us: 0,
            gpu_interval_ns: None,
        })
    }
    fn render(
        &mut self,
        gpu: &super::Device,
        renderer: &mut GpuRenderer,
        job: &mut Job,
        warming: bool,
        timeout: Duration,
    ) -> Result<(), String> {
        let begin = rt::now_us();
        self.submission_us = 0;
        self.gpu_interval_ns = None;
        if let Some(timestamps) = &self.timestamps {
            timestamps.begin(&gpu.device, &gpu.queue);
        }
        if job.layers.len() > 1 {
            if self.composition.is_none() {
                self.composition = Some(Composition::new(&gpu.device, renderer, &self.texture));
            }
            self.composition.as_mut().unwrap().render(
                &gpu.device,
                &gpu.queue,
                renderer,
                &job.layers,
                &self.texture,
                &self.view,
            )?;
        } else {
            let layer = job.layers.first().ok_or("missing Vello scene")?;
            renderer
                .render(
                    &gpu.device,
                    &gpu.queue,
                    &layer.scene,
                    &self.view,
                    self.surface.width,
                    self.surface.height,
                )
                .map_err(|e| format!("Vello frame: {e:?}"))?;
        }
        gpu.errors.check()?;
        if warming {
            bexos_userspace::log("venus-wgpu: warmup render encoded\n");
        }
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        let damage = job.repair;
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: damage.x,
                    y: damage.y,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &self.readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.stride),
                    rows_per_image: Some(damage.height),
                },
            },
            wgpu::Extent3d {
                width: damage.width,
                height: damage.height,
                depth_or_array_layers: 1,
            },
        );
        let timestamp_offset = u64::from(self.stride) * u64::from(self.surface.height);
        if let Some(timestamps) = &self.timestamps {
            timestamps.end(&mut encoder, &self.readback, timestamp_offset);
        }
        let submission = gpu.queue.submit([encoder.finish()]);
        gpu.errors.check()?;
        if warming {
            bexos_userspace::log("venus-wgpu: warmup readback submitted\n");
        }
        self.submission_us = rt::now_us().saturating_sub(begin);
        self.completion.store(0, Ordering::Release);
        let callback = self.completion.clone();
        self.readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |r| {
                callback.store(if r.is_ok() { 1 } else { 2 }, Ordering::Release);
            });
        gpu.device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(submission),
                timeout: Some(timeout),
            })
            .map_err(|e| format!("Vello completion: {e:?}"))?;
        gpu.errors.check()?;
        if self.completion.load(Ordering::Acquire) != 1 {
            return Err("Vello readback failed".into());
        }
        {
            let mapped = self.readback.slice(..).get_mapped_range();
            if let Some(timestamps) = &self.timestamps {
                self.gpu_interval_ns = timestamps.elapsed_ns(&mapped[timestamp_offset as usize..]);
            }
            let output = job.output.bytes_mut();
            for y in 0..damage.height {
                let source = &mapped[(y * self.stride) as usize..][..damage.width as usize * 4];
                let offset = ((damage.y + y) * job.surface.stride + damage.x * 4) as usize;
                let destination = &mut output[offset..offset + damage.width as usize * 4];
                for (src, dst) in source.chunks_exact(4).zip(destination.chunks_exact_mut(4)) {
                    dst.copy_from_slice(
                        &bexos_flatland::Format::Rgba
                            .convert(src.try_into().unwrap(), job.surface.format),
                    );
                }
            }
        }
        self.readback.unmap();
        Ok(())
    }
    fn release(mut self, renderer: &mut GpuRenderer) {
        if let Some(composition) = self.composition.take() {
            composition.release(renderer);
        }
    }
}
fn render_loop(start: &Start) -> Result<(), String> {
    let gpu = bexos_userspace::block_on(super::Device::with_timestamps(start.profile))?;
    let software = gpu.adapter.get_info().device_type == wgpu::DeviceType::Cpu;
    let timeout_us =
        bexos_flatland_render::recovery::timeout_us(software, start.software_timeout_ms)
            .ok_or("invalid software Vulkan recovery timeout")?;
    start
        .recovery_timeout_us
        .store(timeout_us, Ordering::Release);
    let timeout = Duration::from_micros(timeout_us);
    if software {
        bexos_userspace::log(&format!(
            "venus-wgpu: software adapter recovery_timeout_us={timeout_us} hardware_performance_verified=false\n"
        ));
    }
    let mut renderer =
        GpuRenderer::new(&gpu.device).map_err(|e| format!("Vello initialization: {e:?}"))?;
    gpu.errors.check()?;
    let mut target = Some(warmup::prepare(&gpu, &mut renderer, timeout)?);
    start.state.store(READY, Ordering::Release);
    let _ = start.wakeup.send(&[], &[]);
    // Nonoverlapping scheduled CPU intervals include receive polling and the
    // preceding completion notification. Blocked waits add no CPU time.
    let mut cpu_baseline = bexos_userspace::syscall::runtime_stats().ok().map(|s| s.0);
    while !start.stop.load(Ordering::Acquire) {
        let mut job = match start.jobs.recv_timeout(Duration::from_millis(1)) {
            Ok(job) => job,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        let begin = rt::now_us();
        // Validate before allocating a texture: invalid public jobs must return
        // their mapping, never trigger wgpu validation panics in this thread.
        let result = job.validate().and_then(|()| {
            if target.as_ref().is_none_or(|t| t.surface != job.surface) {
                if let Some(old) = target.take() {
                    old.release(&mut renderer);
                }
                target = Some(Target::new(&gpu, job.surface)?);
            }
            target
                .as_mut()
                .unwrap()
                .render(&gpu, &mut renderer, &mut job, false, timeout)
        });
        let failed = result.is_err();
        let (submission_us, gpu_interval_ns) = if result.is_ok() {
            let target = target.as_ref().unwrap();
            (target.submission_us, target.gpu_interval_ns)
        } else {
            (0, None)
        };
        let cpu_end = bexos_userspace::syscall::runtime_stats().ok().map(|s| s.0);
        let cpu_ns = cpu_baseline.zip(cpu_end).map(|(a, b)| b.saturating_sub(a));
        cpu_baseline = cpu_end;
        let sent = start
            .replies
            .send(Message::Complete(Completed {
                job,
                result,
                elapsed_us: rt::now_us().saturating_sub(begin),
                cpu_ns,
                submission_us,
                gpu_interval_ns,
            }))
            .is_ok();
        let _ = start.wakeup.send(&[], &[]);
        if !sent {
            break;
        }
        if failed {
            break;
        }
    }
    // Stop races with a queued job are resolved by returning its mapping. No
    // submitted frame or buffer can disappear during a failed preparation.
    while let Ok(job) = start.jobs.try_recv() {
        let _ = start.replies.try_send(Message::Complete(Completed {
            job,
            result: Err("renderer preparation canceled".into()),
            elapsed_us: 0,
            cpu_ns: None,
            submission_us: 0,
            gpu_interval_ns: None,
        }));
    }
    if let Some(target) = target {
        target.release(&mut renderer);
    }
    let retirement = rt::now_us();
    bexos_userspace::log("venus-wgpu: retiring renderer workspace\n");
    drop(renderer);
    drop(gpu);
    bexos_userspace::log(&format!(
        "venus-wgpu: renderer workspace retired elapsed_us={}\n",
        rt::now_us().saturating_sub(retirement)
    ));
    Ok(())
}
