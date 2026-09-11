use bexos_graphics_runtime::Mapping;
use bexos_userspace::Memory;
use bexos_wasm_runtime::resources::Handle;
use wasmtime::Result;

pub struct RenderedFrame {
    pub handle: u64,
    pub surface: bexos_graphics::Surface,
    #[allow(dead_code)]
    mapping: Option<Mapping>,
}

impl Drop for RenderedFrame {
    fn drop(&mut self) {
        if self.mapping.is_none() {
            let _ = Memory::close(self.handle);
        }
    }
}

pub struct UiRenderer {
    backend: Backend,
    failure: Option<String>,
}

enum Backend {
    Cpu,
    #[cfg(bexos_guest)]
    Gpu(bexos_venus_wgpu::worker::Worker),
}

impl UiRenderer {
    pub fn new(_display: Option<&dyn Handle>, gpu_allowed: bool) -> Self {
        if gpu_allowed {
            #[cfg(bexos_guest)]
            {
                match start_worker(_display.expect("gpu display grant checked")) {
                    Ok(worker) => {
                        return Self {
                            backend: Backend::Gpu(worker),
                            failure: None,
                        };
                    }
                    Err(error) => {
                        return Self {
                            backend: Backend::Cpu,
                            failure: Some(error),
                        };
                    }
                }
            }
            #[cfg(not(bexos_guest))]
            {
                return Self {
                    backend: Backend::Cpu,
                    failure: Some("GPU rendering unavailable on this host build".into()),
                };
            }
        }
        Self {
            backend: Backend::Cpu,
            failure: Some("GPU transport grant unavailable".into()),
        }
    }

    pub fn backend(&self) -> &'static str {
        match &self.backend {
            Backend::Cpu => "cpu",
            #[cfg(bexos_guest)]
            Backend::Gpu(worker) if worker.ready() && !worker.failed() => "gpu",
            #[cfg(bexos_guest)]
            Backend::Gpu(_) => "gpu-initializing",
        }
    }

    pub fn failure(&self) -> Option<String> {
        self.failure.clone()
    }

    pub fn drain_for_migration(&mut self) -> bool {
        #[cfg(bexos_guest)]
        if let Backend::Gpu(worker) = &self.backend {
            if worker.poll().is_err() {
                return true;
            }
            worker.stopped() || worker.ready()
        } else {
            true
        }
        #[cfg(not(bexos_guest))]
        {
            true
        }
    }

    pub fn render(&mut self, batch: &bexos_dioxus_scene::SceneBatch) -> Result<RenderedFrame> {
        #[cfg(bexos_guest)]
        if let Backend::Gpu(worker) = &self.backend {
            match render_gpu(worker, batch) {
                Ok(frame) => return Ok(frame),
                Err(error) => {
                    self.failure = Some(error);
                    self.backend = Backend::Cpu;
                }
            }
        }
        render_cpu(batch)
    }
}

fn render_cpu(batch: &bexos_dioxus_scene::SceneBatch) -> Result<RenderedFrame> {
    let (surface, pixels) = bexos_dioxus_scene::rasterize(batch)
        .map_err(|error| wasmtime::format_err!("cpu scene replay: {error:?}"))?;
    let handle = Memory::from_bytes(&pixels)
        .map_err(|error| wasmtime::format_err!("scene buffer: {error:?}"))?;
    Ok(RenderedFrame {
        handle,
        surface: bexos_graphics::Surface {
            width: surface.width,
            height: surface.height,
            stride: surface.stride,
            format: bexos_graphics::Format::Bgra,
        },
        mapping: None,
    })
}

#[cfg(bexos_guest)]
fn start_worker(
    display: &dyn Handle,
) -> std::result::Result<bexos_venus_wgpu::worker::Worker, String> {
    let duplicate = Memory::duplicate(display.native(), 1 | 2 | 4 | 32)
        .map_err(|status| format!("GPU transport duplicate: {status:?}"))?;
    match bexos_venus_wgpu::worker::Worker::start(bexos_userspace::Channel(duplicate)) {
        Ok(worker) => Ok(worker),
        Err(error) => {
            let _ = Memory::close(duplicate);
            Err(error)
        }
    }
}

#[cfg(bexos_guest)]
fn render_gpu(
    worker: &bexos_venus_wgpu::worker::Worker,
    batch: &bexos_dioxus_scene::SceneBatch,
) -> std::result::Result<RenderedFrame, String> {
    wait_ready(worker)?;
    let surface = bexos_dioxus_render::surface_for(batch)
        .map_err(|error| format!("Vello scene surface: {error:?}"))?;
    let len = surface
        .validate(u64::MAX)
        .map_err(|error| format!("Vello output surface: {error:?}"))?;
    let mut output =
        Mapping::new(len as u64).map_err(|status| format!("Vello output VMO: {status:?}"))?;
    output.bytes_mut().fill(0);
    let layers = bexos_dioxus_render::vello_layers(batch)
        .map_err(|error| format!("Vello scene conversion: {error:?}"))?;
    let damage = surface.full();
    worker
        .submit(bexos_venus_wgpu::worker::Job {
            layers,
            output,
            surface,
            damage,
            repair: damage,
        })
        .map_err(|_| "GPU rendering queue full".to_string())?;
    let completed = receive(worker)?;
    completed.result?;
    Ok(RenderedFrame {
        handle: completed.job.output.handle,
        surface: bexos_graphics::Surface {
            width: completed.job.surface.width,
            height: completed.job.surface.height,
            stride: completed.job.surface.stride,
            format: completed.job.surface.format,
        },
        mapping: Some(completed.job.output),
    })
}

#[cfg(bexos_guest)]
fn wait_ready(worker: &bexos_venus_wgpu::worker::Worker) -> std::result::Result<(), String> {
    let deadline = bexos_graphics_runtime::now_us().saturating_add(2_000_000);
    while !worker.ready() {
        worker.poll()?;
        if worker.stopped() || bexos_graphics_runtime::now_us() >= deadline {
            return Err("GPU renderer initialization timed out".into());
        }
        bexos_graphics_runtime::wait(
            &[worker.wakeup()],
            bexos_graphics_runtime::now_us().saturating_add(1_000),
        );
    }
    Ok(())
}

#[cfg(bexos_guest)]
fn receive(
    worker: &bexos_venus_wgpu::worker::Worker,
) -> std::result::Result<bexos_venus_wgpu::worker::Completed, String> {
    let deadline = bexos_graphics_runtime::now_us().saturating_add(2_000_000);
    loop {
        if let Some(completed) = worker.poll()? {
            return Ok(completed);
        }
        if bexos_graphics_runtime::now_us() >= deadline {
            return Err("GPU renderer completion timed out".into());
        }
        bexos_graphics_runtime::wait(
            &[worker.wakeup()],
            bexos_graphics_runtime::now_us().saturating_add(1_000),
        );
    }
}
