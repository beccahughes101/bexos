//! Checked host-visible BAR ranges and bounded first-fit resource placement.
use crate::Error;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Aperture {
    pub base: u64,
    pub size: u64,
}
impl Aperture {
    pub fn new(bar: u64, bar_size: u64, offset: u64, size: u64) -> Result<Self, Error> {
        let base = bar.checked_add(offset).ok_or(Error::Invalid)?;
        if bar == 0
            || size == 0
            || base % 4096 != 0
            || size % 4096 != 0
            || offset.checked_add(size).is_none_or(|end| end > bar_size)
            || base.checked_add(size).is_none()
        {
            return Err(Error::Invalid);
        }
        Ok(Self { base, size })
    }
    /// Allocation is outside the frame loop. At most 65 bounded passes; no
    /// allocation or sorting of the live resource list is needed.
    pub fn allocate(&self, size: u64, occupied: &[(u64, u64)]) -> Result<u64, Error> {
        if size == 0 || size % 4096 != 0 || occupied.len() > 64 {
            return Err(Error::Invalid);
        }
        for (i, &(offset, length)) in occupied.iter().enumerate() {
            if offset % 4096 != 0
                || length == 0
                || length % 4096 != 0
                || offset.checked_add(length).is_none_or(|end| end > self.size)
                || occupied[..i].iter().any(|&(other, len)| {
                    offset < other.saturating_add(len) && other < offset + length
                })
            {
                return Err(Error::Invalid);
            }
        }
        let mut candidate = 0u64;
        for _ in 0..=occupied.len() {
            let end = candidate
                .checked_add(size)
                .filter(|end| *end <= self.size)
                .ok_or(Error::Capacity)?;
            if let Some(&(offset, length)) = occupied
                .iter()
                .find(|&&(offset, length)| candidate < offset + length && offset < end)
            {
                candidate = offset + length;
            } else {
                return Ok(candidate);
            }
        }
        Err(Error::Capacity)
    }
}
