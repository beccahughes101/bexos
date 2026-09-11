//! Linux input-event records used by VirtIO. Report storage is fixed-size.
use crate::{Event, Key, Phase, Pointer, keymap::Keymap};
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct RawEvent {
    pub kind: u16,
    pub code: u16,
    pub value: i32,
}
impl RawEvent {
    pub fn decode(bytes: [u8; 8]) -> Self {
        Self {
            kind: u16::from_le_bytes([bytes[0], bytes[1]]),
            code: u16::from_le_bytes([bytes[2], bytes[3]]),
            value: i32::from_le_bytes(bytes[4..].try_into().unwrap()),
        }
    }
}
pub struct Device {
    pub id: u64,
    pub pointer: Pointer,
    pub pressed: [bool; 128],
    pub caps: bool,
    pub keymap: Keymap,
    pub(crate) changed: bool,
    pub(crate) old_buttons: u32,
    pub touch: crate::touch::Touch,
    pub axes: [i32; 4],
    pub repeat_code: Option<u16>,
    pub repeat_at: u64,
    pub repeat_delay: u64,
    pub repeat_interval: u64,
    pub acceleration: f64,
    pub dropping: bool,
}
impl Device {
    pub fn new(id: u64) -> Self {
        Self {
            id,
            pointer: Pointer {
                device: id,
                ..Default::default()
            },
            pressed: [false; 128],
            caps: false,
            keymap: Keymap::default(),
            touch: Default::default(),
            axes: [0, 32767, 0, 32767],
            repeat_code: None,
            repeat_at: 0,
            repeat_delay: 500_000,
            repeat_interval: 33_333,
            acceleration: 0.25,
            dropping: false,
            changed: false,
            old_buttons: 0,
        }
    }
    pub fn feed(&mut self, raw: RawEvent, width: f64, height: f64) -> Option<Event> {
        self.feed_at(raw, width, height, 0)
    }
    pub fn feed_at(&mut self, raw: RawEvent, width: f64, height: f64, now: u64) -> Option<Event> {
        if !width.is_finite() || !height.is_finite() || width <= 0. || height <= 0. {
            return None;
        }
        if self.dropping {
            if raw.kind == 0 && raw.code == 0 {
                self.dropping = false;
            }
            return None;
        }
        match (raw.kind, raw.code) {
            (0, 3) => {
                self.touch.reset();
                self.repeat_code = None;
                self.dropping = true;
                self.pressed.fill(false);
                self.pointer.buttons = 0;
                self.old_buttons = 0;
                self.changed = false;
                return Some(Event::Reset);
            }
            (1, code) if code < 128 => {
                if !(0..=2).contains(&raw.value) {
                    return None;
                }
                let index = code as usize;
                if code == 58 && raw.value == 1 && !self.pressed[index] {
                    self.caps = !self.caps;
                }
                if raw.value == 0 && self.repeat_code == Some(code) {
                    self.repeat_code = None;
                }
                if raw.value == 1 && !matches!(code, 29 | 42 | 54 | 56 | 58 | 97 | 100 | 125 | 126)
                {
                    self.repeat_code = Some(code);
                    self.repeat_at = now.saturating_add(self.repeat_delay);
                }
                self.pressed[index] = raw.value != 0;
                let shift = self.pressed[42] || self.pressed[54];
                let control = self.pressed[29] || self.pressed[97];
                let alt = self.pressed[56] || self.pressed[100];
                let meta = self.pressed[125] || self.pressed[126];
                return Some(Event::Key(Key {
                    device: self.id,
                    code: code as u32,
                    state: raw.value as u8,
                    modifiers: u32::from(shift)
                        | (u32::from(control) << 1)
                        | (u32::from(alt) << 2)
                        | (u32::from(meta) << 3),
                    unicode: if raw.value == 0 || control || alt || meta {
                        0
                    } else {
                        self.keymap.translate(code as u32, shift, self.caps)
                    },
                }));
            }
            (1, code @ 272..=279) => {
                let bit = 1 << (code - 272);
                if raw.value == 0 {
                    self.pointer.buttons &= !bit;
                } else if raw.value == 1 {
                    self.pointer.buttons |= bit;
                }
                self.changed = true;
            }
            (2, 0) => {
                self.pointer.x = (self.pointer.x + self.accelerate(raw.value))
                    .max(0.)
                    .min((width - 1.).max(0.));
                self.changed = true;
            }
            (2, 1) => {
                self.pointer.y = (self.pointer.y + self.accelerate(raw.value))
                    .max(0.)
                    .min((height - 1.).max(0.));
                self.changed = true;
            }
            (3, 0) => {
                self.pointer.x = crate::touch::axis(raw.value, self.axes[0], self.axes[1], width);
                self.changed = true;
            }
            (3, 1) => {
                self.pointer.y = crate::touch::axis(raw.value, self.axes[2], self.axes[3], height);
                self.changed = true;
            }
            (1, 330) if !self.touch.enabled => {
                if raw.value == 0 {
                    self.pointer.buttons &= !1
                } else if raw.value == 1 {
                    self.pointer.buttons |= 1
                }
                self.changed = true;
            }
            (2, 8) => {
                self.pointer.scroll_y += raw.value as f32;
                self.changed = true;
            }
            (2, 6) => {
                self.pointer.scroll_x += raw.value as f32;
                self.changed = true;
            }
            (0, 0) if self.changed => {
                self.pointer.phase = if self.old_buttons == 0 && self.pointer.buttons != 0 {
                    Phase::Down
                } else if self.old_buttons != 0 && self.pointer.buttons == 0 {
                    Phase::Up
                } else if self.pointer.buttons != 0 {
                    Phase::Move
                } else {
                    Phase::Hover
                };
                self.old_buttons = self.pointer.buttons;
                self.changed = false;
                let event = Event::Pointer(self.pointer);
                self.pointer.scroll_x = 0.;
                self.pointer.scroll_y = 0.;
                return Some(event);
            }
            _ => {}
        }
        None
    }
}

impl Device {
    fn accelerate(&self, value: i32) -> f64 {
        let gain = if self.acceleration.is_finite() {
            self.acceleration.clamp(0., 4.)
        } else {
            0.
        };
        value as f64 * (1. + gain * (value as f64).abs().min(16.) / 16.)
    }
    pub fn feed_report(
        &mut self,
        raw: RawEvent,
        width: f64,
        height: f64,
        now: u64,
        mut emit: impl FnMut(Event),
    ) {
        if !self.dropping && self.touch.feed(raw) {
            return;
        }
        if raw.kind == 0 && raw.code == 0 && !self.dropping {
            self.touch
                .flush(self.id, width, height, |p| emit(Event::Pointer(p)));
        }
        if let Some(e) = self.feed_at(raw, width, height, now) {
            emit(e)
        }
    }
    pub fn repeat(&mut self, now: u64, width: f64, height: f64) -> Option<Event> {
        let code = self.repeat_code?;
        if now < self.repeat_at || !self.pressed[code as usize] {
            return None;
        }
        self.repeat_at = now.saturating_add(self.repeat_interval);
        self.feed_at(
            RawEvent {
                kind: 1,
                code,
                value: 2,
            },
            width,
            height,
            now,
        )
    }
}
