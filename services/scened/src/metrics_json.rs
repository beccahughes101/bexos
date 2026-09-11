//! Fixed storage keeps diagnostic formatting outside allocator measurements.
use core::fmt;
pub struct Number(pub Option<u64>);
impl fmt::Display for Number {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Some(n) => write!(f, "{n}"),
            None => f.write_str("null"),
        }
    }
}
pub struct Line {
    bytes: [u8; 4096],
    len: usize,
}
impl Line {
    pub fn new() -> Self {
        Self {
            bytes: [0; 4096],
            len: 0,
        }
    }
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).unwrap()
    }
}
impl fmt::Write for Line {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let end = self.len.checked_add(s.len()).ok_or(fmt::Error)?;
        self.bytes
            .get_mut(self.len..end)
            .ok_or(fmt::Error)?
            .copy_from_slice(s.as_bytes());
        self.len = end;
        Ok(())
    }
}
