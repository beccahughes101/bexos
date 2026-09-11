//! Fixed type-B multitouch slots. Coordinates are scaled from device ABS ranges.
use crate::virtio::RawEvent;
use crate::{Phase, Pointer};
pub const SLOTS: usize = 32;
#[derive(Clone, Copy, Debug)]
pub struct Slot {
    pub tracking: i32,
    pub delivered: bool,
    pub x: i32,
    pub y: i32,
    pub dirty: bool,
    pub restarted: bool,
}
impl Default for Slot {
    fn default() -> Self {
        Self {
            tracking: -1,
            delivered: false,
            x: 0,
            y: 0,
            dirty: false,
            restarted: false,
        }
    }
}
#[derive(Clone, Debug)]
pub struct Touch {
    pub slots: [Slot; SLOTS],
    pub current: Option<usize>,
    pub enabled: bool,
    pub min_x: i32,
    pub max_x: i32,
    pub min_y: i32,
    pub max_y: i32,
}
impl Default for Touch {
    fn default() -> Self {
        Self {
            slots: [Slot::default(); SLOTS],
            current: Some(0),
            enabled: false,
            min_x: 0,
            max_x: 32767,
            min_y: 0,
            max_y: 32767,
        }
    }
}
pub fn axis(value: i32, min: i32, max: i32, extent: f64) -> f64 {
    if max <= min || !extent.is_finite() || extent < 1. {
        return 0.;
    }
    ((value as f64 - min as f64) / (max as f64 - min as f64)).clamp(0., 1.) * (extent - 1.)
}
impl Touch {
    pub fn feed(&mut self, event: RawEvent) -> bool {
        if event.kind != 3 {
            return false;
        }
        match event.code {
            0x2f => {
                self.enabled = true;
                self.current = usize::try_from(event.value).ok().filter(|v| *v < SLOTS);
            }
            0x35 | 0x36 | 0x39 => {
                self.enabled = true;
                if let Some(i) = self.current {
                    let s = &mut self.slots[i];
                    match event.code {
                        0x35 => s.x = event.value,
                        0x36 => s.y = event.value,
                        _ => {
                            if s.delivered && s.tracking != event.value && event.value >= 0 {
                                s.restarted = true;
                            }
                            s.tracking = event.value;
                        }
                    };
                    s.dirty = true;
                }
            }
            _ => return false,
        }
        true
    }
    pub fn flush(&mut self, device: u64, width: f64, height: f64, mut emit: impl FnMut(Pointer)) {
        for (id, s) in self.slots.iter_mut().enumerate() {
            if !s.dirty {
                continue;
            }
            s.dirty = false;
            if s.restarted {
                s.restarted = false;
                emit(Pointer {
                    device,
                    id: id as u32 + 1,
                    x: axis(s.x, self.min_x, self.max_x, width),
                    y: axis(s.y, self.min_y, self.max_y, height),
                    phase: Phase::Cancel,
                    buttons: 0,
                    scroll_x: 0.,
                    scroll_y: 0.,
                });
                s.delivered = false;
            }
            let phase = match (s.delivered, s.tracking >= 0) {
                (false, true) => Phase::Down,
                (true, true) => Phase::Move,
                (true, false) => Phase::Up,
                (false, false) => continue,
            };
            emit(Pointer {
                device,
                id: id as u32 + 1,
                x: axis(s.x, self.min_x, self.max_x, width),
                y: axis(s.y, self.min_y, self.max_y, height),
                phase,
                buttons: 0,
                scroll_x: 0.,
                scroll_y: 0.,
            });
            s.delivered = s.tracking >= 0;
        }
    }
    pub fn reset(&mut self) {
        self.slots.fill(Slot::default());
        self.current = Some(0);
    }
}
