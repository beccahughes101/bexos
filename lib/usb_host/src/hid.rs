#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BootKeyboardReport {
    pub modifiers: u8,
    pub keys: [u8; 6],
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BootMouseReport {
    pub buttons: u8,
    pub x: i8,
    pub y: i8,
    pub wheel: i8,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KeyTransition {
    pub code: u16,
    pub pressed: bool,
}

pub fn keyboard_transitions(
    previous: BootKeyboardReport,
    next: BootKeyboardReport,
    out: &mut [KeyTransition],
) -> usize {
    let mut count = 0;
    for bit in 0..8 {
        let mask = 1u8 << bit;
        if previous.modifiers & mask != next.modifiers & mask && count < out.len() {
            out[count] = KeyTransition {
                code: 0xe0 + bit as u16,
                pressed: next.modifiers & mask != 0,
            };
            count += 1;
        }
    }
    for key in previous.keys {
        if key != 0 && !next.keys.contains(&key) && count < out.len() {
            out[count] = KeyTransition {
                code: key as u16,
                pressed: false,
            };
            count += 1;
        }
    }
    for key in next.keys {
        if key != 0 && !previous.keys.contains(&key) && count < out.len() {
            out[count] = KeyTransition {
                code: key as u16,
                pressed: true,
            };
            count += 1;
        }
    }
    count
}

pub fn release_all(report: BootKeyboardReport, out: &mut [KeyTransition]) -> usize {
    keyboard_transitions(report, BootKeyboardReport::default(), out)
}
