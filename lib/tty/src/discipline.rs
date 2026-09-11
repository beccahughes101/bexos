//! Provider-side line discipline. No syscall, process, or shell policy is hidden here.
use crate::Signal;
use alloc::vec::Vec;
pub const LIMIT: usize = 16 * 1024;
#[derive(Clone, Debug)]
pub struct Discipline {
    pub mode: u32,
    pub pending: Vec<u8>,
}
pub struct Input {
    pub bytes: Vec<u8>,
    pub echo: Vec<u8>,
    pub signals: Vec<Signal>,
    pub eof: bool,
    pub consumed: usize,
}
impl Default for Discipline {
    fn default() -> Self {
        Self {
            mode: 7,
            pending: Vec::new(),
        }
    }
}
impl Discipline {
    pub fn feed(&mut self, bytes: &[u8]) -> Input {
        let mut r = Input {
            bytes: Vec::new(),
            echo: Vec::new(),
            signals: Vec::new(),
            eof: false,
            consumed: 0,
        };
        if self.mode & 2 == 0 {
            r.bytes.append(&mut self.pending);
        }
        for &b in bytes {
            // Release a full canonical chunk so a long line cannot deadlock input.
            if self.pending.len() == LIMIT && r.bytes.is_empty() {
                r.bytes.append(&mut self.pending);
            }
            if r.bytes.len() + self.pending.len() + 1 > LIMIT
                || r.signals.len() >= 32
                || r.echo.len() + 3 > LIMIT
            {
                break;
            }
            r.consumed += 1;
            if self.mode & 4 != 0 && (b == 3 || b == 26) {
                r.signals.push(if b == 3 {
                    Signal::Interrupt
                } else {
                    Signal::Suspend
                });
                self.pending.clear();
                continue;
            }
            if self.mode & 2 == 0 {
                r.bytes.push(b);
                if self.mode & 1 != 0 {
                    r.echo.push(b);
                }
                continue;
            }
            match b {
                4 => {
                    if self.pending.is_empty() {
                        r.eof = true;
                    } else {
                        r.bytes.append(&mut self.pending);
                    }
                }
                8 | 127 => {
                    if self.pending.pop().is_some() && self.mode & 1 != 0 {
                        r.echo.extend_from_slice(b"\x08 \x08");
                    }
                }
                b'\r' | b'\n' => {
                    self.pending.push(b'\n');
                    r.bytes.append(&mut self.pending);
                    if self.mode & 1 != 0 {
                        r.echo.extend_from_slice(b"\r\n");
                    }
                }
                _ => {
                    self.pending.push(b);
                    if self.mode & 1 != 0 {
                        r.echo.push(b);
                    }
                }
            }
        }
        r
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn canonical_editing_and_eof() {
        let mut d = Discipline::default();
        let r = d.feed(b"ab\x7fc\r");
        assert_eq!(r.bytes, b"ac\n");
        assert!(d.feed(&[4]).eof);
    }
    #[test]
    fn raw_and_signals() {
        let mut d = Discipline::default();
        assert_eq!(d.feed(&[3]).signals, vec![Signal::Interrupt]);
        d.mode = 0;
        assert_eq!(d.feed(&[3, 4, 0]).bytes, vec![3, 4, 0]);
    }
    #[test]
    fn full_canonical_line_makes_progress() {
        let mut d = Discipline::default();
        d.mode = 2;
        assert_eq!(d.feed(&vec![b'a'; LIMIT]).consumed, LIMIT);
        let first = d.feed(b"\n");
        assert_eq!(first.bytes.len(), LIMIT);
        assert_eq!(first.consumed, 0);
        let second = d.feed(b"\n");
        assert_eq!(second.bytes, b"\n");
        assert_eq!(second.consumed, 1);
    }
    #[test]
    fn bounded() {
        let mut d = Discipline::default();
        assert!(d.feed(&vec![b'a'; LIMIT * 2]).consumed <= LIMIT);
    }
}
