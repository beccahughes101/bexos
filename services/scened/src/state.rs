use bexos_graphics::{
    Surface,
    progress::FrameClock,
    scene::{Graph, Node, Session},
};
use bexos_graphics_runtime::{Mapping, canvas::Canvas, migration::Component};
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
use bexos_userspace::{Channel, live_migration::Resource};
use std::collections::BTreeMap;
#[derive(Default)]
pub struct Scene {
    pub shell: crate::shell::Composition,
    pub period_us: u64,
    // Histograms are process-local caches. Keep their fixed windows off the
    // migration decoder's stack, which stages complete logical Scene values.
    pub metrics: Box<crate::metrics::Metrics>,
    pub scanout: crate::scanout::Scanout,
    pub gpu: crate::gpu::Backend,
    pub effects: Option<crate::effects::Cache>,
    pub legacy_effects: bool,
    pub layout_cache: BTreeMap<u64, bexos_flatland_layout::graph::Cache>,
    pub style_cache: BTreeMap<u64, crate::styling::Cache>,
    pub presentation_encoding: (u64, u64),
    pub legacy_styles: bool,
    pub pending_frame: Option<crate::presentation::PendingFrame>,
    pub takeover_deadline_us: u64,
    pub takeover_retry_us: u64,
    pub legacy_presentations: bool,
    pub controls: crate::controls::Controls,
    pub legacy_controls: bool,
    pub legacy_graphs: bool,
    pub world: bexos_graphics::world::World,
    pub world_generation: u64,
    pub damage_tracker: crate::damage::Tracker,
    pub full_damage: bool,
    pub desktop: Option<crate::desktop::Desktop>,
    pub canvas: Option<Canvas>,
    pub pending_display: Option<Channel>,
    pub splash: Option<Channel>,
    pub font_provider: Option<Channel>,
    pub font_metrics: Option<bexos_flatland_text::metrics::Metrics>,
    pub frozen: Option<Mapping>,
    pub ack: Option<Channel>,
    pub start_us: u64,
    pub clock: FrameClock,
    pub ready: bool,
    pub input: crate::input::Input,
    pub fences: bexos_graphics::synchronization::Synchronization,
    pub resumed: bool,
    pub sessions: BTreeMap<u64, Session>,
    pub buffers: BTreeMap<u64, Mapping>,
    pub queues: BTreeMap<u64, bexos_graphics::presentation::Queue>,
    pub snapshots: BTreeMap<u64, bexos_graphics::resolved::Snapshot>,
    pub snapshot_changes: std::collections::BTreeSet<u64>,
    pub dirty: bool,
    pub damage: Option<bexos_graphics::Damage>,
    pub legacy_records: bool,
    pub records_version: u64,
    pub legacy_fences: bool,
    pub records: BTreeMap<u64, super::records::Stream>,
}
impl Scene {
    pub fn frame_period_us(&self) -> u64 {
        if self.period_us == 0 {
            8_333
        } else {
            self.period_us
        }
    }
    pub fn ensure_font_metrics(
        &mut self,
    ) -> Result<bexos_flatland_text::metrics::Metrics, bexos_graphics::Error> {
        if let Some(metrics) = self.font_metrics {
            return Ok(metrics);
        }
        #[cfg(bexos_guest)]
        let metrics = {
            let endpoint = self.font_provider.ok_or(bexos_graphics::Error::Invalid)?;
            let mut client = bexos_font_client::Client::new(endpoint.0);
            let font = client
                .resolve(bexos_font_client::Request::sans("Inter"))
                .map_err(|_| bexos_graphics::Error::Invalid)?;
            bexos_flatland_text::metrics::Metrics::from_font(font.as_ref().as_ref())
                .map_err(|_| bexos_graphics::Error::Invalid)?
        };
        #[cfg(not(bexos_guest))]
        let metrics =
            bexos_flatland_text::metrics::Metrics::from_font(include_bytes!(env!("INTER")))
                .map_err(|_| bexos_graphics::Error::Invalid)?;
        self.font_metrics = Some(metrics);
        Ok(metrics)
    }
}
pub(crate) fn encode_graph(
    g: &Graph,
    w: &mut Encoder,
    embedded: bool,
    effects: bool,
    styles: bool,
) {
    encode_graph_version(g, w, embedded, effects, styles);
}
fn encode_graph_version(g: &Graph, w: &mut Encoder, embedded: bool, effects: bool, styles: bool) {
    w.word(g.root.unwrap_or(0));
    w.word(g.nodes.len() as u64);
    for (id, n) in &g.nodes {
        w.word(*id);
        if styles {
            w.text(&n.style.identifier);
            w.text(&n.style.classes);
            w.text(&n.style.inline);
        }
        if effects {
            w.word(n.effects.corner_radius.to_bits() as u64);
            w.word(n.effects.backdrop_radius as u64);
            w.word(n.layout.mode as u64);
            for v in [
                n.layout.width,
                n.layout.height,
                n.layout.grow,
                n.layout.gap,
                n.layout.padding,
                n.layout_offset.0,
                n.layout_offset.1,
            ] {
                w.word(v.to_bits() as u64);
            }
            w.word(n.layout.columns as u64);
            w.word(n.layout_size.is_some() as u64);
            if let Some((width, height)) = n.layout_size {
                w.word(width.to_bits() as u64);
                w.word(height.to_bits() as u64);
            }
        }
        if embedded {
            w.word(n.embedded.unwrap_or(0));
        }
        w.word(n.children.len() as u64);
        for c in &n.children {
            w.word(*c);
        }
        w.word(n.translation.0 as i64 as u64);
        w.word(n.translation.1 as i64 as u64);
        w.word(n.scale.0.to_bits() as u64);
        w.word(n.scale.1.to_bits() as u64);
        w.word(n.opacity.to_bits() as u64);
        w.word(n.clip.is_some() as u64);
        if let Some((a, b)) = n.clip {
            w.word(a as u64);
            w.word(b as u64);
        }
        w.word(n.content.is_some() as u64);
        if let Some((h, s)) = n.content {
            for v in [
                h,
                s.width as u64,
                s.height as u64,
                s.stride as u64,
                s.format as u64,
            ] {
                w.word(v);
            }
        }
    }
}
fn u32word(r: &mut Decoder<'_>) -> Result<u32, Error> {
    r.word()?.try_into().map_err(|_| Error::InvalidData)
}
pub(crate) fn decode_graph_version(
    r: &mut Decoder<'_>,
    embedded: bool,
    effects: bool,
    styles: bool,
) -> Result<Graph, Error> {
    let root = r.word()?;
    let mut g = Graph {
        root: (root != 0).then_some(root),
        ..Default::default()
    };
    for _ in 0..r.count(256)? {
        let id = r.word()?;
        let mut n = Node::default();
        if styles {
            n.style.identifier = r.text(64)?.into();
            n.style.classes = r.text(128)?.into();
            n.style.inline = r.text(512)?.into();
        }
        if effects {
            n.effects = bexos_graphics::effects::Effects {
                corner_radius: f32::from_bits(u32word(r)?),
                backdrop_radius: u32word(r)?,
            };
            n.layout.mode = u32word(r)?;
            n.layout.width = f32::from_bits(u32word(r)?);
            n.layout.height = f32::from_bits(u32word(r)?);
            n.layout.grow = f32::from_bits(u32word(r)?);
            n.layout.gap = f32::from_bits(u32word(r)?);
            n.layout.padding = f32::from_bits(u32word(r)?);
            n.layout_offset = (f32::from_bits(u32word(r)?), f32::from_bits(u32word(r)?));
            n.layout.columns = u32word(r)?;
            if r.flag()? {
                n.layout_size = Some((f32::from_bits(u32word(r)?), f32::from_bits(u32word(r)?)));
            }
        }
        if embedded {
            let child = r.word()?;
            n.embedded = (child != 0).then_some(child);
        }
        for _ in 0..r.count(256)? {
            n.children.push(r.word()?);
        }
        n.translation = (r.word()? as i64 as i32, r.word()? as i64 as i32);
        n.scale = (f32::from_bits(u32word(r)?), f32::from_bits(u32word(r)?));
        n.opacity = f32::from_bits(u32word(r)?);
        if r.flag()? {
            n.clip = Some((u32word(r)?, u32word(r)?));
        }
        if r.flag()? {
            let h = r.word()?;
            let s = Surface {
                width: u32word(r)?,
                height: u32word(r)?,
                stride: u32word(r)?,
                format: u32word(r)?.try_into().map_err(|_| Error::InvalidData)?,
            };
            n.content = Some((h, s));
        }
        if id == 0 || g.nodes.insert(id, n).is_some() {
            return Err(Error::InvalidData);
        }
    }
    g.validate().map_err(|_| Error::InvalidData)?;
    Ok(g)
}
impl Component for Scene {
    fn quiescence_ready(&self) -> bool {
        !self.scanout.client.busy() && self.gpu.quiescent()
    }
    fn record_keys(&self) -> Vec<u64> {
        super::records::keys(self)
    }
    fn encode_component_record(&self, key: u64) -> Result<Option<Vec<u8>>, Error> {
        super::records::encode(self, key)
    }
    fn adopt_component_record(&mut self, key: u64, bytes: Option<&[u8]>) -> Result<(), Error> {
        super::records::adopt(self, key, bytes)
    }
    fn finish_records(&mut self) -> Result<(), Error> {
        super::records::finish(self)
    }
    fn deadline_profile(&self) -> bool {
        true
    }
    fn frame_period_ns(&self) -> u64 {
        self.frame_period_us() * 1000
    }
    fn encode(&self, w: &mut Encoder) -> Result<(), Error> {
        w.word(self.canvas.is_some() as u64);
        if let Some(c) = &self.canvas {
            c.encode(w);
        }
        w.word(self.splash.map_or(0, |c| c.0));
        w.word(self.frozen.is_some() as u64);
        if let Some(m) = &self.frozen {
            m.encode(w);
        }
        w.word(self.ack.map_or(0, |c| c.0));
        w.word(self.start_us);
        w.word(self.clock.next_us);
        w.word(self.ready as u64);
        w.word(self.sessions.len() as u64);
        for (id, s) in &self.sessions {
            w.word(*id);
            w.word(s.presentation);
            encode_graph_version(&s.pending, w, false, false, false);
            encode_graph_version(&s.committed, w, false, false, false);
        }
        w.word(self.buffers.len() as u64);
        for (id, m) in &self.buffers {
            w.word(*id);
            m.encode(w);
        }
        Ok(())
    }
    fn decode(&mut self, r: &mut Decoder<'_>) -> Result<(), Error> {
        let canvas = if r.flag()? {
            Some(Canvas::decode(r)?)
        } else {
            None
        };
        let splash = r.word()?;
        let frozen = if r.flag()? {
            Some(Mapping::decode(r)?)
        } else {
            None
        };
        let ack = r.word()?;
        let start_us = r.word()?;
        let clock = FrameClock { next_us: r.word()? };
        let ready = r.flag()?;
        let mut sessions = BTreeMap::new();
        for _ in 0..r.count(16)? {
            let id = r.word()?;
            let presentation = r.word()?;
            let pending = decode_graph_version(r, false, false, false)?;
            let committed = decode_graph_version(r, false, false, false)?;
            if sessions
                .insert(
                    id,
                    Session {
                        pending,
                        committed,
                        presentation,
                    },
                )
                .is_some()
            {
                return Err(Error::InvalidData);
            }
        }
        let mut buffers = BTreeMap::new();
        for _ in 0..r.count(256)? {
            let id = r.word()?;
            let m = Mapping::decode(r)?;
            if id != m.handle || buffers.insert(id, m).is_some() {
                return Err(Error::InvalidData);
            }
        }
        let font_provider = if self.records_version >= 11 {
            r.word()?
        } else {
            0
        };
        // The initial global record arrives during bulk transfer. Build text
        // then, and retain it across subsequent global deltas, so the final
        // quiescent boundary does not shape fonts or initialize a rasterizer.
        let desktop = match &canvas {
            Some(c) => match self.desktop.take() {
                Some(cache) if cache.surface == c.surface => Some(cache),
                _ => Some(
                    crate::desktop::Desktop::new(
                        c.surface,
                        (font_provider != 0).then_some(Channel(font_provider)),
                    )
                    .map_err(|_| Error::InvalidData)?,
                ),
            },
            None => None,
        };
        *self = Self {
            desktop,
            canvas,
            splash: (splash != 0).then_some(Channel(splash)),
            font_provider: (font_provider != 0).then_some(Channel(font_provider)),
            frozen,
            ack: (ack != 0).then_some(Channel(ack)),
            start_us,
            clock,
            ready,
            resumed: false,
            sessions,
            buffers,
            ..Default::default()
        };
        self.validate()
    }
    fn resources(&self) -> Vec<Resource> {
        let mut out = self.canvas.as_ref().map_or(Vec::new(), |c| c.resources());
        out.extend(self.input.resources());
        out.extend(self.scanout.resources());
        out.extend(self.gpu.resources());
        out.extend(self.controls.resources());
        if self.shell.hide_reply != 0 {
            out.push(Resource::Handle(self.shell.hide_reply));
        }
        out.extend(self.fences.handles().into_iter().map(Resource::Handle));
        for c in [self.splash, self.ack].into_iter().flatten() {
            out.push(Resource::Handle(c.0));
        }
        if let Some(provider) = self.font_provider {
            out.push(Resource::Handle(provider.0));
        }
        if let Some(m) = &self.frozen {
            out.extend(m.resources());
        }
        for m in self.buffers.values() {
            out.extend(m.resources());
        }
        out
    }
    fn activate(&mut self) {
        self.resumed = true;
        self.dirty = true;
        self.records.clear();
        self.legacy_records = false;
        self.records_version = 0;
        self.legacy_fences = false;
        self.legacy_controls = false;
        self.legacy_graphs = false;
        self.legacy_presentations = false;
        self.presentation_encoding = (0, 0);
        self.scanout.legacy = false;
        self.legacy_effects = false;
        self.legacy_styles = false;
        self.gpu.legacy = false;
        self.gpu.pending_encoding = 0;
        if let Some(c) = &self.canvas {
            if self
                .sessions
                .values()
                .flat_map(|s| [&s.pending, &s.committed])
                .flat_map(|g| g.nodes.values())
                .chain(
                    self.queues
                        .values()
                        .flat_map(|q| &q.frames)
                        .flat_map(|f| f.graph.nodes.values()),
                )
                .any(|n| n.effects.backdrop_radius != 0)
            {
                self.effects = crate::effects::Cache::new(c.surface).ok();
            }
        }
        if let Some(c) = &mut self.canvas {
            c.output.owned = true;
        }
        if let Some(m) = &mut self.frozen {
            m.owned = true;
        }
        for m in self.buffers.values_mut() {
            m.owned = true;
        }
    }
    fn validate(&self) -> Result<(), Error> {
        if ![0, 8_333, 16_667].contains(&self.period_us) {
            return Err(Error::InvalidData);
        }
        if let Some(frame) = &self.pending_frame {
            if self.canvas.as_ref().is_none_or(|c| c.display.0 == 0)
                || frame.count > 16
                || self.legacy_presentations
                || frame.sequences[..frame.count].iter().any(|(view, seq)| {
                    self.queues
                        .get(view)
                        .is_some_and(|q| *seq > q.latched_sequence || *seq < q.presented_sequence)
                })
            {
                return Err(Error::InvalidData);
            }
        }
        if self.input.gesture_owner != 0
            && !self
                .controls
                .shell
                .iter()
                .any(|c| c.channel.0 == self.input.gesture_owner)
        {
            return Err(Error::InvalidData);
        }
        self.controls.validate()?;
        bexos_graphics::world::World::validate(&self.sessions).map_err(|_| Error::InvalidData)?;
        if self
            .controls
            .views
            .keys()
            .chain(self.controls.committed.keys())
            .any(|id| !self.sessions.contains_key(id))
        {
            return Err(Error::InvalidData);
        }
        self.fences.validate().map_err(|_| Error::InvalidData)?;
        if self.fences.frames.len() + self.scanout.deferred.len() > 64
            || self
                .scanout
                .client
                .entries
                .iter()
                .any(|e| !self.buffers.contains_key(&e.key))
        {
            return Err(Error::InvalidData);
        }
        for (id, q) in &self.queues {
            if !self.sessions.contains_key(id) {
                return Err(Error::InvalidData);
            }
            q.validate().map_err(|_| Error::InvalidData)?;
            for p in &q.frames {
                for n in p.graph.nodes.values() {
                    if let Some((h, s)) = n.content {
                        s.validate(self.buffers.get(&h).ok_or(Error::InvalidData)?.size)
                            .map_err(|_| Error::InvalidData)?;
                    }
                }
            }
        }
        if self
            .sessions
            .keys()
            .chain(self.buffers.keys())
            .any(|id| *id == 0 || *id > u32::MAX as u64 / 2)
        {
            return Err(Error::Capacity);
        }
        if self.sessions.len() > 16
            || self.buffers.len() > 512
            || self
                .buffers
                .values()
                .fold(0u64, |n, m| n.saturating_add(m.size))
                > 256 * 1024 * 1024
            || self
                .sessions
                .values()
                .map(|s| s.pending.nodes.len())
                .sum::<usize>()
                > 4096
            || self
                .sessions
                .values()
                .map(|s| s.committed.nodes.len())
                .sum::<usize>()
                > 4096
        {
            return Err(Error::InvalidData);
        }
        for s in self.sessions.values() {
            for g in [&s.pending, &s.committed] {
                g.validate().map_err(|_| Error::InvalidData)?;
                for n in g.nodes.values() {
                    if let Some((h, s)) = n.content {
                        let m = self.buffers.get(&h).ok_or(Error::InvalidData)?;
                        s.validate(m.size).map_err(|_| Error::InvalidData)?;
                    }
                }
            }
        }
        if let Some(m) = &self.frozen {
            let c = self.canvas.as_ref().ok_or(Error::InvalidData)?;
            if m.size != c.output.size {
                return Err(Error::InvalidData);
            }
        }
        Ok(())
    }
}
