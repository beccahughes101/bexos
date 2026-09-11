//! Versioned logical input state. DMA and channel capabilities remain with owners.
use crate::{
    Event, Key, Phase, Pointer,
    queue::EventQueue,
    router::{Capture, Router},
    touch::{SLOTS, Slot},
    virtio::Device,
};
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
fn u32word(r: &mut Decoder<'_>) -> Result<u32, Error> {
    r.word()?.try_into().map_err(|_| Error::InvalidData)
}
pub fn pointer_write(p: Pointer, w: &mut Encoder) {
    for v in [
        p.device,
        p.id as u64,
        p.x.to_bits(),
        p.y.to_bits(),
        p.phase as u64,
        p.buttons as u64,
        p.scroll_x.to_bits() as u64,
        p.scroll_y.to_bits() as u64,
    ] {
        w.word(v)
    }
}
pub fn pointer_read(r: &mut Decoder<'_>) -> Result<Pointer, Error> {
    let p = Pointer {
        device: r.word()?,
        id: u32word(r)?,
        x: f64::from_bits(r.word()?),
        y: f64::from_bits(r.word()?),
        phase: match r.word()? {
            1 => Phase::Down,
            2 => Phase::Move,
            3 => Phase::Up,
            4 => Phase::Hover,
            5 => Phase::Cancel,
            _ => return Err(Error::InvalidData),
        },
        buttons: u32word(r)?,
        scroll_x: f32::from_bits(u32word(r)?),
        scroll_y: f32::from_bits(u32word(r)?),
    };
    if !p.x.is_finite() || !p.y.is_finite() || !p.scroll_x.is_finite() || !p.scroll_y.is_finite() {
        return Err(Error::InvalidData);
    }
    Ok(p)
}
pub fn event_write(e: Event, w: &mut Encoder) {
    match e {
        Event::Reset => w.word(0),
        Event::Pointer(p) => {
            w.word(1);
            pointer_write(p, w)
        }
        Event::Key(k) => {
            w.word(2);
            for v in [
                k.device,
                k.code as u64,
                k.state as u64,
                k.modifiers as u64,
                k.unicode as u64,
            ] {
                w.word(v)
            }
        }
    }
}
pub fn event_read(r: &mut Decoder<'_>) -> Result<Event, Error> {
    Ok(match r.word()? {
        0 => Event::Reset,
        1 => Event::Pointer(pointer_read(r)?),
        2 => {
            let k = Key {
                device: r.word()?,
                code: u32word(r)?,
                state: r.word()?.try_into().map_err(|_| Error::InvalidData)?,
                modifiers: u32word(r)?,
                unicode: u32word(r)?,
            };
            if k.state > 2 || k.code >= 128 {
                return Err(Error::InvalidData);
            }
            Event::Key(k)
        }
        _ => return Err(Error::InvalidData),
    })
}
impl Device {
    pub fn encode(&self, w: &mut Encoder) {
        self.encode_keyboard(w, true);
    }
    /// Secure input checkpoints retain device topology, never typed keys.
    pub fn encode_keyboard(&self, w: &mut Encoder, include_keys: bool) {
        w.word(3);
        w.word(self.id);
        pointer_write(self.pointer, w);
        for p in self.pressed {
            w.word((include_keys && p) as u64)
        }
        w.word(self.caps as u64);
        for key in self.keymap.normal.iter().chain(&self.keymap.shifted) {
            w.word(*key as u64)
        }
        w.word(self.changed as u64);
        w.word(self.old_buttons as u64);
        for a in self.axes {
            w.word(a as i64 as u64)
        }
        w.word(if include_keys {
            self.repeat_code.map_or(128, |c| c as u64)
        } else {
            128
        });
        w.word(if include_keys { self.repeat_at } else { 0 });
        w.word(self.acceleration.to_bits());
        w.word(self.dropping as u64);
        let t = &self.touch;
        w.word(t.current.map_or(SLOTS, |i| i) as u64);
        w.word(t.enabled as u64);
        for v in [t.min_x, t.max_x, t.min_y, t.max_y] {
            w.word(v as i64 as u64)
        }
        for s in t.slots {
            w.word(s.tracking as i64 as u64);
            w.word(s.delivered as u64);
            w.word(s.x as i64 as u64);
            w.word(s.y as i64 as u64);
            w.word(s.dirty as u64);
            w.word(s.restarted as u64);
        }
        w.word(self.repeat_delay);
        w.word(self.repeat_interval);
    }
    pub fn decode(r: &mut Decoder<'_>) -> Result<Self, Error> {
        let version = r.word()?;
        if !(1..=3).contains(&version) {
            return Err(Error::UnsupportedVersion);
        }
        let mut d = Device::new(r.word()?);
        d.pointer = pointer_read(r)?;
        if d.pointer.device != d.id {
            return Err(Error::InvalidData);
        }
        for p in &mut d.pressed {
            *p = r.flag()?
        }
        d.caps = r.flag()?;
        for k in d.keymap.normal.iter_mut().chain(&mut d.keymap.shifted) {
            *k = u32word(r)?;
            if *k != 0 && char::from_u32(*k).is_none() {
                return Err(Error::InvalidData);
            }
        }
        d.changed = r.flag()?;
        d.old_buttons = u32word(r)?;
        for a in &mut d.axes {
            *a = i32::try_from(r.word()? as i64).map_err(|_| Error::InvalidData)?;
        }
        let code = r.word()?;
        d.repeat_code = if code == 128 {
            None
        } else {
            if code >= 128 {
                return Err(Error::InvalidData);
            }
            Some(code as u16)
        };
        d.repeat_at = r.word()?;
        d.acceleration = f64::from_bits(r.word()?);
        if !d.acceleration.is_finite() || !(0. ..=4.).contains(&d.acceleration) {
            return Err(Error::InvalidData);
        }
        d.dropping = r.flag()?;
        let slot = r.count(SLOTS)?;
        d.touch.current = (slot < SLOTS).then_some(slot);
        d.touch.enabled = r.flag()?;
        let mut axes = [0; 4];
        for a in &mut axes {
            *a = i32::try_from(r.word()? as i64).map_err(|_| Error::InvalidData)?;
        }
        [d.touch.min_x, d.touch.max_x, d.touch.min_y, d.touch.max_y] = axes;
        for s in &mut d.touch.slots {
            *s = Slot {
                tracking: i32::try_from(r.word()? as i64).map_err(|_| Error::InvalidData)?,
                delivered: r.flag()?,
                x: i32::try_from(r.word()? as i64).map_err(|_| Error::InvalidData)?,
                y: i32::try_from(r.word()? as i64).map_err(|_| Error::InvalidData)?,
                dirty: r.flag()?,
                restarted: if version >= 2 { r.flag()? } else { false },
            };
        }
        if version >= 3 {
            d.repeat_delay = r.word()?;
            d.repeat_interval = r.word()?;
            if !(100_000..=2_000_000).contains(&d.repeat_delay)
                || !(5_000..=1_000_000).contains(&d.repeat_interval)
            {
                return Err(Error::InvalidData);
            }
        }
        Ok(d)
    }
}
impl Router {
    pub fn encode(&self, w: &mut Encoder) {
        w.word(1);
        w.word(self.focused.unwrap_or(0));
        w.word(self.captures.len() as u64);
        for ((device, id), c) in &self.captures {
            w.word(*device);
            w.word(*id as u64);
            w.word(c.view);
            w.word(c.node);
            pointer_write(c.last, w);
        }
        w.word(self.keys.len() as u64);
        for key in self.keys.values() {
            event_write(Event::Key(*key), w)
        }
        w.word(self.blocked.len() as u64);
        for (d, c) in &self.blocked {
            w.word(*d);
            w.word(*c as u64)
        }
    }
    pub fn decode(r: &mut Decoder<'_>) -> Result<Self, Error> {
        if r.word()? != 1 {
            return Err(Error::UnsupportedVersion);
        }
        let mut out = Self::default();
        out.focused = match r.word()? {
            0 => None,
            v => Some(v),
        };
        for _ in 0..r.count(64)? {
            let device = r.word()?;
            let id = u32word(r)?;
            let view = r.word()?;
            let node = r.word()?;
            let last = pointer_read(r)?;
            if view == 0
                || node == 0
                || device != last.device
                || id != last.id
                || out
                    .captures
                    .insert((device, id), Capture { view, node, last })
                    .is_some()
            {
                return Err(Error::InvalidData);
            }
        }
        for _ in 0..r.count(256)? {
            let Event::Key(k) = event_read(r)? else {
                return Err(Error::InvalidData);
            };
            if out.keys.insert((k.device, k.code), k).is_some() {
                return Err(Error::InvalidData);
            }
        }
        for _ in 0..r.count(256)? {
            let d = r.word()?;
            let c = u32word(r)?;
            if !out.blocked.insert((d, c)) {
                return Err(Error::InvalidData);
            }
        }
        Ok(out)
    }
}
impl<const N: usize> EventQueue<N> {
    pub fn encode(&self, w: &mut Encoder) {
        w.word(self.overflows);
        w.word(self.len() as u64);
        for event in self.iter() {
            event_write(event, w);
        }
    }
    pub fn decode(r: &mut Decoder<'_>) -> Result<Self, Error> {
        let mut out = Self::default();
        out.overflows = r.word()?;
        for _ in 0..r.count(N)? {
            out.push(event_read(r)?);
        }
        Ok(out)
    }
}
