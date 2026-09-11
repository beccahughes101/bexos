//! Bounded input normalization and focus/capture routing, shared by driver and
//! compositor. Device IDs and view identities are assigned by capability owners.
#![no_std]
extern crate alloc;
pub mod gestures;
pub mod keymap;
pub mod migration;
pub mod queue;
pub mod router;
pub mod settings;
pub mod touch;
pub mod virtio;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum Phase {
    Down = 1,
    Move = 2,
    Up = 3,
    #[default]
    Hover = 4,
    Cancel = 5,
}
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pointer {
    pub device: u64,
    pub id: u32,
    pub x: f64,
    pub y: f64,
    pub phase: Phase,
    pub buttons: u32,
    pub scroll_x: f32,
    pub scroll_y: f32,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Key {
    pub device: u64,
    pub code: u32,
    pub state: u8,
    pub modifiers: u32,
    pub unicode: u32,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Event {
    Pointer(Pointer),
    Key(Key),
    Reset,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Delivery {
    pub view: u64,
    pub event: Event,
}
