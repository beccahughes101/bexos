use bexos_graphics_runtime::Mapping;
use bexos_userspace::Memory;
use bexos_wasm_runtime::resources::Handle;
use std::collections::BTreeMap;
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

    pub fn render(
        &mut self,
        batch: &bexos_dioxus_scene::SceneBatch,
        fonts: &BTreeMap<u32, bexos_dioxus_render::FontResource>,
    ) -> Result<RenderedFrame> {
        #[cfg(bexos_guest)]
        if let Backend::Gpu(worker) = &self.backend {
            match render_gpu(worker, batch, fonts) {
                Ok(frame) => return Ok(frame),
                Err(error) => {
                    self.failure = Some(error);
                    self.backend = Backend::Cpu;
                }
            }
        }
        render_cpu(batch, fonts)
    }
}

fn render_cpu(
    batch: &bexos_dioxus_scene::SceneBatch,
    fonts: &BTreeMap<u32, bexos_dioxus_render::FontResource>,
) -> Result<RenderedFrame> {
    let mut paths_only = batch.clone();
    paths_only
        .commands
        .retain(|command| !matches!(command, bexos_dioxus_scene::Command::Glyphs(_)));
    let (surface, mut pixels) = bexos_dioxus_scene::rasterize(&paths_only)
        .map_err(|error| wasmtime::format_err!("cpu scene replay: {error:?}"))?;
    rasterize_glyphs(batch, fonts, &mut pixels)
        .map_err(|error| wasmtime::format_err!("CPU glyph rendering: {error}"))?;
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

fn rasterize_glyphs(
    batch: &bexos_dioxus_scene::SceneBatch,
    fonts: &BTreeMap<u32, bexos_dioxus_render::FontResource>,
    pixels: &mut [u8],
) -> std::result::Result<(), &'static str> {
    use skrifa::{
        FontRef, GlyphId, MetadataProvider,
        instance::{LocationRef, Size},
        outline::DrawSettings,
    };
    let stride = batch.width as usize * 4;
    for command in &batch.commands {
        let bexos_dioxus_scene::Command::Glyphs(run) = command else {
            continue;
        };
        let data = fonts.get(&run.font_asset).ok_or("missing mapped font")?;
        let font = FontRef::from_index(data.data.as_ref().as_ref(), data.collection_index)
            .map_err(|_| "invalid mapped font")?;
        let outlines = font.outline_glyphs();
        let mut cursor = run.x;
        for glyph in &run.glyphs {
            let Some(outline) = outlines.get(GlyphId::new(glyph.id)) else {
                cursor += glyph.advance;
                continue;
            };
            let mut pen = FlatteningPen::default();
            outline
                .draw(
                    DrawSettings::unhinted(Size::new(run.size), LocationRef::default()),
                    &mut pen,
                )
                .map_err(|_| "invalid glyph outline")?;
            pen.finish();
            let origin_x = cursor + glyph.x;
            let baseline = run.y + glyph.y;
            fill_outline(
                pixels,
                batch.width,
                batch.height,
                stride,
                &pen.contours,
                origin_x,
                baseline,
                run.color,
            );
            cursor += glyph.advance;
        }
    }
    Ok(())
}

#[derive(Default)]
struct FlatteningPen {
    contours: Vec<Vec<(f32, f32)>>,
    current: Vec<(f32, f32)>,
}

impl FlatteningPen {
    fn finish(&mut self) {
        if self.current.len() > 2 {
            self.contours.push(core::mem::take(&mut self.current));
        } else {
            self.current.clear();
        }
    }
    fn last(&self) -> (f32, f32) {
        self.current.last().copied().unwrap_or((0.0, 0.0))
    }
}

impl skrifa::outline::OutlinePen for FlatteningPen {
    fn move_to(&mut self, x: f32, y: f32) {
        self.finish();
        self.current.push((x, y));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.current.push((x, y));
    }
    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        let (x0, y0) = self.last();
        for step in 1..=8 {
            let t = step as f32 / 8.0;
            let u = 1.0 - t;
            self.current.push((
                u * u * x0 + 2.0 * u * t * cx + t * t * x,
                u * u * y0 + 2.0 * u * t * cy + t * t * y,
            ));
        }
    }
    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        let (x0, y0) = self.last();
        for step in 1..=12 {
            let t = step as f32 / 12.0;
            let u = 1.0 - t;
            self.current.push((
                u * u * u * x0 + 3.0 * u * u * t * cx0 + 3.0 * u * t * t * cx1 + t * t * t * x,
                u * u * u * y0 + 3.0 * u * u * t * cy0 + 3.0 * u * t * t * cy1 + t * t * t * y,
            ));
        }
    }
    fn close(&mut self) {
        self.finish();
    }
}

fn fill_outline(
    pixels: &mut [u8],
    width: u32,
    height: u32,
    stride: usize,
    contours: &[Vec<(f32, f32)>],
    origin_x: f32,
    baseline: f32,
    color: [u8; 4],
) {
    let points = contours
        .iter()
        .flatten()
        .map(|(x, y)| (origin_x + x, baseline - y))
        .collect::<Vec<_>>();
    let Some(min_x) = points.iter().map(|point| point.0).reduce(f32::min) else {
        return;
    };
    let min_y = points.iter().map(|point| point.1).reduce(f32::min).unwrap();
    let max_x = points.iter().map(|point| point.0).reduce(f32::max).unwrap();
    let max_y = points.iter().map(|point| point.1).reduce(f32::max).unwrap();
    for py in (min_y.floor().max(0.0) as u32)..(max_y.ceil().min(height as f32) as u32) {
        for px in (min_x.floor().max(0.0) as u32)..(max_x.ceil().min(width as f32) as u32) {
            let sample = (px as f32 + 0.5, py as f32 + 0.5);
            let mut inside = false;
            for contour in contours {
                for index in 0..contour.len() {
                    let raw_a = contour[index];
                    let raw_b = contour[(index + 1) % contour.len()];
                    let a = (origin_x + raw_a.0, baseline - raw_a.1);
                    let b = (origin_x + raw_b.0, baseline - raw_b.1);
                    if (a.1 > sample.1) != (b.1 > sample.1)
                        && sample.0 < (b.0 - a.0) * (sample.1 - a.1) / (b.1 - a.1) + a.0
                    {
                        inside = !inside;
                    }
                }
            }
            if inside {
                let offset = py as usize * stride + px as usize * 4;
                let alpha = color[3] as u16;
                for channel in 0..3 {
                    pixels[offset + channel] = ((color[channel] as u16 * alpha
                        + pixels[offset + channel] as u16 * (255 - alpha))
                        / 255) as u8;
                }
                pixels[offset + 3] = 255;
            }
        }
    }
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
    fonts: &BTreeMap<u32, bexos_dioxus_render::FontResource>,
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
    let layers =
        bexos_dioxus_render::vello_layers_with_fonts(batch, |asset| fonts.get(&asset).cloned())
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
