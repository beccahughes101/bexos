use bexos_usb_host::hid::{
    BootKeyboardReport, BootMouseReport, KeyTransition, keyboard_transitions,
};
use input_fidl::RawInputEvent;

pub const EV_KEY: u16 = 1;
pub const EV_REL: u16 = 2;
pub const EV_SYN: u16 = 0;
pub const REL_X: u16 = 0;
pub const REL_Y: u16 = 1;
pub const REL_WHEEL: u16 = 8;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReportBatch {
    pub events: [RawInputEvent; 16],
    pub count: usize,
}

impl Default for ReportBatch {
    fn default() -> Self {
        Self {
            events: [RawInputEvent {
                kind: 0,
                code: 0,
                value: 0,
            }; 16],
            count: 0,
        }
    }
}

pub fn report_events(
    previous_keyboard: BootKeyboardReport,
    keyboard: Option<BootKeyboardReport>,
    previous_mouse: BootMouseReport,
    mouse: Option<BootMouseReport>,
) -> ReportBatch {
    let mut batch = ReportBatch::default();
    if let Some(next) = keyboard {
        let mut transitions = [KeyTransition::default(); 16];
        let count = keyboard_transitions(previous_keyboard, next, &mut transitions);
        for transition in &transitions[..count] {
            push(
                &mut batch,
                RawInputEvent {
                    kind: EV_KEY,
                    code: transition.code,
                    value: transition.pressed as i32,
                },
            );
        }
    }
    if let Some(next) = mouse {
        let changed_buttons = previous_mouse.buttons ^ next.buttons;
        for bit in 0..8 {
            let mask = 1u8 << bit;
            if changed_buttons & mask != 0 {
                push(
                    &mut batch,
                    RawInputEvent {
                        kind: EV_KEY,
                        code: 0x110 + bit,
                        value: (next.buttons & mask != 0) as i32,
                    },
                );
            }
        }
        if next.x != 0 {
            push(
                &mut batch,
                RawInputEvent {
                    kind: EV_REL,
                    code: REL_X,
                    value: next.x as i32,
                },
            );
        }
        if next.y != 0 {
            push(
                &mut batch,
                RawInputEvent {
                    kind: EV_REL,
                    code: REL_Y,
                    value: next.y as i32,
                },
            );
        }
        if next.wheel != 0 {
            push(
                &mut batch,
                RawInputEvent {
                    kind: EV_REL,
                    code: REL_WHEEL,
                    value: next.wheel as i32,
                },
            );
        }
    }
    if batch.count != 0 {
        push(
            &mut batch,
            RawInputEvent {
                kind: EV_SYN,
                code: 0,
                value: 0,
            },
        );
    }
    batch
}

fn push(batch: &mut ReportBatch, event: RawInputEvent) {
    if batch.count < batch.events.len() {
        batch.events[batch.count] = event;
        batch.count += 1;
    }
}
