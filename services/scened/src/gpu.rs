//! Backend selection, bounded worker coordination, and migration drain policy.
//! Logical scene graphs remain authoritative; renderer caches are rebuildable.
use crate::{presentation::PendingFrame, state::Scene};
use bexos_graphics_runtime::Mapping;
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
#[cfg(bexos_guest)]
use bexos_userspace::Memory;
use bexos_userspace::{Channel, live_migration::Resource};
#[derive(Default)]
pub struct Backend {
    pub pending_encoding: u64,
    pub endpoint: u64,
    pub disabled: bool,
    pub legacy: bool,
    pub pending: Option<PendingFrame>,
    pub spare: Option<Mapping>,
    /// Process-local output freshness. None means the spare needs a full repair.
    pub spare_damage: Option<bexos_graphics::Damage>,
    pub output_valid: bool,
    pub frames: u64,
    #[cfg(bexos_guest)]
    announced: bool,
    #[cfg(bexos_guest)]
    announced_frame: bool,
    pub copied_bytes: u64,
    pub last_work_us: u64,
    #[cfg(bexos_guest)]
    worker: Option<bexos_venus_wgpu::worker::Worker>,
    #[cfg(bexos_guest)]
    images: std::collections::BTreeMap<u64, bexos_flatland_render::vello::peniko::ImageData>,
}
impl Backend {
    pub fn blocks_frames(&self) -> bool {
        self.pending.is_some() && !self.disabled
    }
    /// Stop depending on an overdue job without releasing its output or any
    /// process-local GPU objects. Quiescence still waits for worker retirement.
    pub fn expire(&mut self, now: u64) -> bool {
        if !self.disabled
            && self
                .pending
                .as_ref()
                .is_some_and(|f| now >= f.submission.deadline_us)
        {
            self.disabled = true;
            self.invalidate_outputs();
            // The GPU result no longer owns presentation feedback. Keep the
            // outstanding-work marker for drain, but do not serialize obsolete
            // sequences after CPU presentation has advanced the same views.
            if let Some(frame) = &mut self.pending {
                frame.count = 0;
                frame.sequences.fill((0, 0));
            }
            true
        } else {
            false
        }
    }
    pub fn invalidate_outputs(&mut self) {
        self.output_valid = false;
        self.spare_damage = None;
    }
    pub fn forget_buffer(&mut self, buffer: u64) {
        #[cfg(bexos_guest)]
        {
            self.images.remove(&buffer);
        }
        #[cfg(not(bexos_guest))]
        let _ = buffer;
    }
    pub fn invalidate_transaction(
        &mut self,
        old: &bexos_graphics::scene::Graph,
        new: &bexos_graphics::scene::Graph,
        fenced: bool,
    ) {
        #[cfg(bexos_guest)]
        {
            for node in old.nodes.values() {
                if let Some((buffer, _)) = node.content {
                    if !new
                        .nodes
                        .values()
                        .any(|n| n.content.is_some_and(|(id, _)| id == buffer))
                    {
                        self.images.remove(&buffer);
                    }
                }
            }
            // Legacy Present has no immutable-buffer lease. Its accepted
            // presentation is the content-change boundary for cached uploads.
            if !fenced {
                for node in new.nodes.values() {
                    if let Some((buffer, _)) = node.content {
                        self.images.remove(&buffer);
                    }
                }
            }
        }
        #[cfg(not(bexos_guest))]
        let _ = (old, new, fenced);
    }
    pub fn wakeup(&self) -> Option<Channel> {
        #[cfg(bexos_guest)]
        {
            self.worker.as_ref().map(|w| w.wakeup())
        }
        #[cfg(not(bexos_guest))]
        {
            None
        }
    }
    pub fn new(endpoint: u64) -> Self {
        Self {
            endpoint,
            ..Self::default()
        }
    }
    pub fn quiescent(&self) -> bool {
        #[cfg(bexos_guest)]
        {
            self.worker.is_none() && self.pending.is_none()
        }
        #[cfg(not(bexos_guest))]
        {
            self.pending.is_none()
        }
    }
    pub fn resources(&self) -> Vec<Resource> {
        if self.endpoint == 0 {
            Vec::new()
        } else {
            vec![Resource::Handle(self.endpoint)]
        }
    }
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Encoder::new();
        w.word(1);
        for value in [
            self.endpoint,
            self.disabled as u64,
            self.frames,
            self.copied_bytes,
            self.last_work_us,
        ] {
            w.word(value);
        }
        // A pre-copy may see pending work. Final quiescence drains it into the
        // normal display PendingFrame record, never into a Rust/Vulkan pointer.
        let mut bytes = w.finish();
        bytes.extend(PendingFrame::encode_version(
            self.pending.as_ref(),
            if self.pending_encoding == 0 {
                3
            } else {
                self.pending_encoding
            },
        ));
        bytes
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut r = Decoder::new(bytes);
        if r.word()? != 1 {
            return Err(Error::UnsupportedVersion);
        }
        let state = Self {
            endpoint: r.word()?,
            disabled: r.flag()?,
            frames: r.word()?,
            copied_bytes: r.word()?,
            last_work_us: r.word()?,
            pending: PendingFrame::decode(bytes.get(48..).ok_or(Error::InvalidData)?)?,
            pending_encoding: u64::from_le_bytes(
                bytes
                    .get(48..56)
                    .ok_or(Error::InvalidData)?
                    .try_into()
                    .unwrap(),
            ),
            ..Self::default()
        };
        Ok(state)
    }
}
#[cfg(not(bexos_guest))]
pub fn poll(_s: &mut Scene, _draining: bool) {}
#[cfg(not(bexos_guest))]
pub fn render(_s: &mut Scene, _now: u64) -> bool {
    false
}

#[cfg(bexos_guest)]
pub fn poll(s: &mut Scene, draining: bool) {
    use bexos_venus_wgpu::worker::Worker;
    if s.gpu.expire(bexos_graphics_runtime::now_us()) {
        s.dirty = true;
        s.damage = None;
        s.metrics.failed = s.metrics.failed.saturating_add(1);
        bexos_userspace::log(
            "scened: GPU frame deadline expired; retaining visible frame and using CPU\n",
        );
    }
    if let Some(worker) = s.gpu.worker.as_ref() {
        if draining || s.gpu.disabled {
            worker.stop();
        }
        if worker.ready() && !s.gpu.announced {
            s.gpu.announced = true;
            s.dirty = true;
            bexos_userspace::log(
                "scened: Vello/Venus worker ready; retained readback presentation\n",
            );
        }
        match worker.poll() {
            Ok(Some(completed)) => {
                let already_failed = s.gpu.disabled;
                let mut frame = s.gpu.pending.take();
                s.gpu.last_work_us = completed.elapsed_us;
                if completed.result.is_ok() && !already_failed {
                    if let Some(c) = s.canvas.as_mut() {
                        let old = std::mem::replace(&mut c.output, completed.job.output);
                        s.gpu.spare = Some(old);
                        s.gpu.output_valid = true;
                        s.gpu.spare_damage = Some(completed.job.damage);
                        s.gpu.frames += 1;
                        s.gpu.copied_bytes += (completed.job.repair.width as u64
                            * completed.job.repair.height as u64)
                            * 4;
                        if let Ok(submission) =
                            bexos_graphics_runtime::presentation::Submission::begin(
                                c,
                                completed.job.damage,
                            )
                        {
                            s.full_damage = false;
                            s.metrics.pending_worker = Some(crate::metrics::WorkerSample {
                                wall_us: completed.elapsed_us,
                                cpu_ns: completed.cpu_ns,
                                submission_us: completed.submission_us,
                                interval_ns: completed.gpu_interval_ns,
                                readback_bytes: completed.job.repair.width as u64
                                    * completed.job.repair.height as u64
                                    * 4,
                            });
                            if !s.gpu.announced_frame {
                                s.gpu.announced_frame = true;
                                bexos_userspace::log(
                                    "scened: Vello/Venus composed frame submitted\n",
                                );
                            }
                            if let Some(mut f) = frame.take() {
                                f.submission = submission;
                                s.pending_frame = Some(f);
                            }
                        }
                    }
                } else {
                    s.gpu.spare = Some(completed.job.output);
                    s.gpu.output_valid = false;
                    s.gpu.spare_damage = None;
                    if !draining && !already_failed {
                        bexos_userspace::log(&format!(
                            "scened: GPU frame failed {:?}; retaining visible frame and using CPU\n",
                            completed.result
                        ));
                        s.gpu.disabled = true;
                        worker.stop();
                    }
                }
                if frame.is_some() {
                    if !already_failed {
                        s.metrics.failed = s.metrics.failed.saturating_add(1);
                    }
                    s.dirty = true;
                    s.damage = None;
                }
            }
            Err(error) => {
                bexos_userspace::log(&format!(
                    "scened: GPU backend unavailable: {error}; using CPU\n"
                ));
                s.gpu.disabled = true;
                worker.stop();
            }
            Ok(None) => {}
        }
        if worker.stopped() {
            if worker.failed() {
                s.gpu.disabled = true;
            }
            // Drain notification precedes STOPPED. The queued completion must be
            // consumed before reaping a worker or cutting over process state.
            if s.gpu.pending.is_none() {
                s.gpu.worker = None;
                s.gpu.announced = false;
                s.gpu.announced_frame = false;
                s.gpu.images.clear();
                s.gpu.spare = None;
                s.gpu.invalidate_outputs();
                if s.gpu.disabled && s.gpu.endpoint != 0 {
                    let _ = Memory::close(s.gpu.endpoint);
                    s.gpu.endpoint = 0;
                }
            }
        }
    } else if !draining && !s.gpu.disabled && s.gpu.endpoint != 0 && s.start_us != 0 {
        let profile = s
            .desktop
            .as_ref()
            .is_some_and(|desktop| desktop.profile_gpu);
        let software_timeout_ms = s
            .desktop
            .as_ref()
            .map_or(2000, |d| d.software_vulkan_timeout_ms);
        match Worker::start_configured(Channel(s.gpu.endpoint), profile, software_timeout_ms) {
            Ok(worker) => {
                s.gpu.worker = Some(worker);
            }
            Err(error) => {
                bexos_userspace::log(&format!("scened: {error}; using CPU\n"));
                s.gpu.disabled = true;
                let _ = Memory::close(s.gpu.endpoint);
                s.gpu.endpoint = 0;
            }
        }
    }
}
#[cfg(bexos_guest)]
pub fn render(s: &mut Scene, now: u64) -> bool {
    use bexos_flatland_render::vello::{
        self,
        kurbo::{Affine, Rect, RoundedRect},
        peniko::{BlendMode, Blob, Fill, ImageAlphaType, ImageData, ImageFormat},
    };
    if s.gpu.blocks_frames() {
        return true;
    }
    if s.gpu.disabled
        || !s.gpu.worker.as_ref().is_some_and(|w| w.ready())
        || s.frozen.is_some()
        || s.controls.ring.is_some()
        || s.controls.display != Default::default()
        || s.world
            .items(&s.snapshots)
            .filter(|(_, i)| i.effects.backdrop_radius != 0)
            .count()
            > bexos_flatland_render::composition::MAX_BACKDROPS
    {
        return false;
    }
    let Some(c) = s.canvas.as_ref() else {
        return false;
    };
    // Logical display damage and stale alternate-buffer pixels are independent.
    // Vello still rasterizes the target, but readback repairs only their union.
    // Backdrop dependencies currently conservatively invalidate the full frame.
    let damage = if !s.gpu.output_valid
        || s.resumed
        || s.world
            .items(&s.snapshots)
            .any(|(_, i)| i.effects.backdrop_radius != 0)
    {
        c.surface.full()
    } else {
        s.damage.unwrap_or(c.surface.full())
    };
    let repair = if s.gpu.spare.is_some() {
        s.gpu
            .spare_damage
            .map_or(c.surface.full(), |stale| stale.union(damage))
    } else {
        c.surface.full()
    };
    let output = match s
        .gpu
        .spare
        .take()
        .or_else(|| Mapping::new(c.output.size).ok())
    {
        Some(m) => m,
        None => return false,
    };
    let mut scene = vello::Scene::new();
    let mut layers = Vec::new();
    let mut backdrop = None;
    if let Some(desktop) = &s.desktop {
        desktop.append_gpu(&mut scene);
    }
    s.gpu.images.retain(|key, _| s.buffers.contains_key(key));
    for (_, item) in s.world.items(&s.snapshots) {
        if item.effects.backdrop_radius != 0 {
            layers.push(bexos_flatland_render::composition::Layer { scene, backdrop });
            scene = vello::Scene::new();
            backdrop = Some(*item);
        }
        if !s.gpu.images.contains_key(&item.buffer) {
            let Some(mapping) = s.buffers.get(&item.buffer) else {
                s.gpu.spare = Some(output);
                return false;
            };
            let mut pixels =
                Vec::with_capacity(item.surface.width as usize * item.surface.height as usize * 4);
            for row in mapping
                .bytes()
                .chunks_exact(item.surface.stride as usize)
                .take(item.surface.height as usize)
            {
                for pixel in row[..item.surface.width as usize * 4].chunks_exact(4) {
                    pixels.extend_from_slice(
                        &item
                            .surface
                            .format
                            .convert(pixel.try_into().unwrap(), bexos_graphics::Format::Rgba),
                    );
                }
            }
            s.gpu.copied_bytes += pixels.len() as u64;
            s.gpu.images.insert(
                item.buffer,
                ImageData {
                    data: Blob::new(std::sync::Arc::new(pixels)),
                    format: ImageFormat::Rgba8,
                    alpha_type: ImageAlphaType::AlphaPremultiplied,
                    width: item.surface.width,
                    height: item.surface.height,
                },
            );
        }
        let t = item.transform;
        let transform = Affine::new([t.sx, 0., 0., t.sy, t.x, t.y]);
        let r = item.visible;
        scene.push_layer(
            Fill::NonZero,
            BlendMode::default(),
            item.opacity,
            Affine::IDENTITY,
            &Rect::new(r.x, r.y, r.x + r.width, r.y + r.height),
        );
        if item.effects.corner_radius != 0. {
            scene.push_layer(
                Fill::NonZero,
                BlendMode::default(),
                1.,
                transform,
                &RoundedRect::new(
                    0.,
                    0.,
                    item.surface.width as f64,
                    item.surface.height as f64,
                    item.effects.corner_radius as f64,
                ),
            );
        }
        scene.draw_image(&s.gpu.images[&item.buffer], transform);
        if item.effects.corner_radius != 0. {
            scene.pop_layer();
        }
        scene.pop_layer();
    }
    let frame = PendingFrame::new(
        s,
        bexos_graphics_runtime::presentation::Submission {
            deadline_us: now.saturating_add(s.gpu.worker.as_ref().unwrap().recovery_timeout_us()),
        },
        now,
        0,
    );
    layers.push(bexos_flatland_render::composition::Layer { scene, backdrop });
    let job = bexos_venus_wgpu::worker::Job {
        layers,
        output,
        surface: c.surface,
        damage,
        repair,
    };
    match s.gpu.worker.as_ref().unwrap().submit(job) {
        Ok(()) => {
            s.gpu.pending = Some(frame);
            s.dirty = false;
            s.damage = None;
            true
        }
        Err(job) => {
            s.gpu.spare = Some(job.output);
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overdue_gpu_work_unblocks_cpu_frames_without_releasing_worker_ownership() {
        let scene = Scene::default();
        let mut frame = PendingFrame::new(
            &scene,
            bexos_graphics_runtime::presentation::Submission { deadline_us: 200 },
            100,
            0,
        );
        frame.count = 1;
        frame.sequences[0] = (5, 7);
        let mut backend = Backend {
            pending: Some(frame),
            output_valid: true,
            spare_damage: Some(bexos_graphics::Damage {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
            }),
            ..Default::default()
        };
        assert!(backend.blocks_frames());
        assert!(!backend.expire(199));
        assert!(backend.expire(200));
        assert!(!backend.blocks_frames());
        assert!(!backend.output_valid);
        assert!(backend.spare_damage.is_none());
        assert!(backend.pending.is_some());
        assert_eq!(backend.pending.as_ref().unwrap().count, 0);
        assert!(!backend.quiescent());
        assert!(!backend.expire(201));
        let restored = Backend::decode(&backend.encode()).unwrap();
        assert!(restored.disabled);
        assert_eq!(restored.pending.unwrap().submission.deadline_us, 200);
    }
}
