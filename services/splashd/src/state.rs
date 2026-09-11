use bexos_graphics::{
    progress::{FrameClock, Progress},
    render::Renderer,
};
use bexos_graphics_runtime::{canvas::Canvas, migration::Component};
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
use bexos_userspace::{Channel, live_migration::Resource};
#[derive(Default)]
pub struct Splash {
    pub canvas: Option<Canvas>,
    pub firmware: Option<(bexos_graphics_runtime::Mapping, bexos_graphics::Surface)>,
    pub renderer: Option<Renderer>,
    pub progress: Progress,
    pub clock: FrameClock,
    pub ack: Option<Channel>,
    pub deadline: u64,
    pub handoff_generation: u64,
    pub frozen: bool,
    pub frames: u64,
    pub render_us: u64,
    pub resumed: bool,
}
impl Component for Splash {
    fn deadline_profile(&self) -> bool {
        true
    }
    fn encode(&self, w: &mut Encoder) -> Result<(), Error> {
        w.word(self.canvas.is_some() as u64);
        if let Some(c) = &self.canvas {
            c.encode(w);
        }
        w.word(self.firmware.is_some() as u64);
        if let Some((m, s)) = &self.firmware {
            m.encode(w);
            for v in [s.width, s.height, s.stride, s.format as u32] {
                w.word(v as u64);
            }
        }
        w.word(self.progress.stage as u64);
        w.word(self.progress.percent as u64);
        w.bytes(self.progress.message.as_bytes());
        for v in [
            self.clock.next_us,
            self.ack.map_or(0, |c| c.0),
            self.deadline,
            self.handoff_generation,
            self.frozen as u64,
            self.frames,
            self.render_us,
        ] {
            w.word(v);
        }
        Ok(())
    }
    fn decode(&mut self, r: &mut Decoder<'_>) -> Result<(), Error> {
        let canvas = if r.flag()? {
            Some(Canvas::decode(r)?)
        } else {
            None
        };
        let firmware = if r.flag()? {
            let m = bexos_graphics_runtime::Mapping::decode(r)?;
            let s = bexos_graphics::Surface {
                width: r.word()? as u32,
                height: r.word()? as u32,
                stride: r.word()? as u32,
                format: (r.word()? as u32)
                    .try_into()
                    .map_err(|_| Error::InvalidData)?,
            };
            s.validate(m.size).map_err(|_| Error::InvalidData)?;
            Some((m, s))
        } else {
            None
        };
        let stage = r.word()?.try_into().map_err(|_| Error::InvalidData)?;
        let percent = r.word()?.try_into().map_err(|_| Error::InvalidData)?;
        let msg = core::str::from_utf8(r.bytes(64)?).map_err(|_| Error::InvalidData)?;
        let mut progress = Progress::default();
        progress
            .report(stage, percent, msg)
            .map_err(|_| Error::InvalidData)?;
        let clock = FrameClock { next_us: r.word()? };
        let ack = r.word()?;
        let deadline = r.word()?;
        let handoff_generation = r.word()?;
        let frozen = r.flag()?;
        let frames = r.word()?;
        let render_us = r.word()?;
        *self = Self {
            canvas,
            firmware,
            renderer: None,
            progress,
            clock,
            ack: (ack != 0).then_some(Channel(ack)),
            deadline,
            handoff_generation,
            frozen,
            frames,
            render_us,
            resumed: false,
        };
        self.validate()
    }
    fn resources(&self) -> Vec<Resource> {
        let mut out = self.canvas.as_ref().map_or(Vec::new(), |c| c.resources());
        if let Some((m, _)) = &self.firmware {
            out.extend(m.resources());
        }
        if let Some(c) = self.ack {
            out.push(Resource::Handle(c.0));
        }
        out
    }
    fn activate(&mut self) {
        self.resumed = true;
        if let Some(c) = &mut self.canvas {
            c.output.owned = true;
        }
        if let Some((m, _)) = &mut self.firmware {
            m.owned = true;
        }
        self.renderer = None;
    }
    fn validate(&self) -> Result<(), Error> {
        if self.frozen != (self.ack.is_some()) || self.frozen && self.canvas.is_none() {
            Err(Error::InvalidData)
        } else {
            Ok(())
        }
    }
}
