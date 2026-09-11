//! Configurable scan-code mapping; the default is the US physical keyboard.
#[derive(Clone, Debug)]
pub struct Keymap {
    pub normal: [u32; 128],
    pub shifted: [u32; 128],
}
impl Default for Keymap {
    fn default() -> Self {
        let mut m = Self {
            normal: [0; 128],
            shifted: [0; 128],
        };
        for (start, a, b) in [
            (2, "1234567890-=", "!@#$%^&*()_+"),
            (16, "qwertyuiop[]", "QWERTYUIOP{}"),
            (30, "asdfghjkl;'", "ASDFGHJKL:\""),
            (44, "zxcvbnm,./", "ZXCVBNM<>?"),
        ] {
            for (i, c) in a.bytes().enumerate() {
                m.normal[start + i] = c as u32;
            }
            for (i, c) in b.bytes().enumerate() {
                m.shifted[start + i] = c as u32;
            }
        }
        for (code, a, b) in [
            (28, 10, 10),
            (15, 9, 9),
            (57, 32, 32),
            (43, 92, 124),
            (41, 96, 126),
        ] {
            m.normal[code] = a;
            m.shifted[code] = b;
        }
        m
    }
}
impl Keymap {
    pub fn translate(&self, code: u32, shift: bool, caps: bool) -> u32 {
        let Some(&normal) = self.normal.get(code as usize) else {
            return 0;
        };
        let uppercase = shift ^ (caps && (b'a' as u32..=b'z' as u32).contains(&normal));
        if uppercase {
            self.shifted[code as usize]
        } else {
            normal
        }
    }
}
