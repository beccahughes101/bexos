//! Checked ELF loading into an already assigned domain bank. Authentication
//! must precede this parser; accepting a valid ELF is not signature approval.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImageError {
    Header,
    Architecture,
    Range,
    Overlap,
    Entry,
    Destination,
}
#[derive(Clone, Copy)]
struct Segment {
    flags: u32,
    offset: usize,
    guest: usize,
    file: usize,
    memory: usize,
}
const EMPTY: Segment = Segment {
    flags: 0,
    offset: 0,
    guest: 0,
    file: 0,
    memory: 0,
};
pub struct Image<'a> {
    bytes: &'a [u8],
    segments: [Segment; 64],
    count: usize,
    entry: u64,
    limit: usize,
    identity_linked: bool,
}
impl<'a> Image<'a> {
    pub fn parse(bytes: &'a [u8], limit: usize) -> Result<Self, ImageError> {
        if bytes.len() < 64
            || &bytes[..7] != b"\x7fELF\x02\x01\x01"
            || u16_at(bytes, 16) != 2
            || u32_at(bytes, 20) != 1
            || u16_at(bytes, 52) != 64
            || u16_at(bytes, 54) != 56
        {
            return Err(ImageError::Header);
        }
        if u16_at(bytes, 18) != 62 {
            return Err(ImageError::Architecture);
        }
        let headers = usize::try_from(u64_at(bytes, 32)).map_err(|_| ImageError::Range)?;
        let count = u16_at(bytes, 56) as usize;
        if count == 0 || count > 64 || !inside(headers, count * 56, bytes.len()) {
            return Err(ImageError::Header);
        }
        let requested_entry = u64_at(bytes, 24);
        let mut result = Self {
            bytes,
            segments: [EMPTY; 64],
            count: 0,
            entry: 0,
            limit,
            identity_linked: true,
        };
        let mut entry = None;
        for index in 0..count {
            let h = &bytes[headers + index * 56..headers + (index + 1) * 56];
            if u32_at(h, 0) != 1 {
                continue;
            }
            let flags = u32_at(h, 4);
            let offset = usize::try_from(u64_at(h, 8)).map_err(|_| ImageError::Range)?;
            let virtual_address = u64_at(h, 16);
            let guest = usize::try_from(u64_at(h, 24)).map_err(|_| ImageError::Range)?;
            let file = usize::try_from(u64_at(h, 32)).map_err(|_| ImageError::Range)?;
            let memory = usize::try_from(u64_at(h, 40)).map_err(|_| ImageError::Range)?;
            let alignment = u64_at(h, 48);
            if memory == 0 {
                continue;
            }
            if guest < 0x200000
                || file > memory
                || flags & !7 != 0
                || !inside(offset, file, bytes.len())
                || !inside(guest, memory, limit)
                || virtual_address.checked_add(memory as u64).is_none()
                || (alignment > 1
                    && (!alignment.is_power_of_two()
                        || virtual_address % alignment != offset as u64 % alignment))
            {
                return Err(ImageError::Range);
            }
            if result.segments[..result.count]
                .iter()
                .any(|s| guest < s.guest + s.memory && s.guest < guest + memory)
            {
                return Err(ImageError::Overlap);
            }
            if flags & 1 != 0 {
                let resolved = if requested_entry >= virtual_address
                    && requested_entry - virtual_address < file as u64
                {
                    Some(guest as u64 + requested_entry - virtual_address)
                } else if requested_entry >= guest as u64
                    && requested_entry - (guest as u64) < file as u64
                {
                    // Pinned Trusty's ELF entry is physical; PT_LOAD addresses
                    // are also available at its high-half linked addresses.
                    Some(requested_entry)
                } else {
                    None
                };
                if let Some(resolved) = resolved {
                    if entry.replace(resolved).is_some() {
                        return Err(ImageError::Entry);
                    }
                }
            }
            result.segments[result.count] = Segment {
                flags,
                offset,
                guest,
                file,
                memory,
            };
            result.identity_linked &= virtual_address == guest as u64;
            result.count += 1;
        }
        result.entry = entry.ok_or(ImageError::Entry)?;
        Ok(result)
    }
    pub fn entry(&self) -> u64 {
        self.entry
    }
    /// Borrow file-backed bytes at an identity-linked image address. BSS is
    /// never interpreted as an entry descriptor, and ranges cannot straddle
    /// segment boundaries.
    pub fn bytes_at(&self, address: usize, length: usize) -> Result<&'a [u8], ImageError> {
        self.require_identity_linked()?;
        for segment in &self.segments[..self.count] {
            if address >= segment.guest && inside(address - segment.guest, length, segment.file) {
                let start = segment.offset + address - segment.guest;
                return Ok(&self.bytes[start..start + length]);
            }
        }
        Err(ImageError::Range)
    }
    pub fn executable_at(&self, address: usize, length: usize) -> Result<(), ImageError> {
        self.bytes_at(address, length)?;
        if self.segments[..self.count].iter().any(|s| {
            s.flags & 3 == 1 && address >= s.guest && inside(address - s.guest, length, s.file)
        }) {
            Ok(())
        } else {
            Err(ImageError::Entry)
        }
    }
    /// Replacement images may declare retained code but cannot replace it.
    /// Require byte-for-byte compatibility with the installed recovery entry
    /// points, including all executable resident segments.
    pub fn require_resident_code(&self, base: usize, code: &[u8]) -> Result<(), ImageError> {
        self.require_identity_linked()?;
        let mut found = false;
        for segment in &self.segments[..self.count] {
            if segment.guest < base || segment.flags & 1 == 0 {
                continue;
            }
            if segment.flags & 2 != 0
                || segment.file != segment.memory
                || !inside(segment.guest - base, segment.file, code.len())
                || self.bytes[segment.offset..segment.offset + segment.file]
                    != code[segment.guest - base..segment.guest - base + segment.file]
            {
                return Err(ImageError::Destination);
            }
            found = true;
        }
        if found {
            Ok(())
        } else {
            Err(ImageError::Destination)
        }
    }
    /// The resident nucleus owns the omitted extent. Validate every segment
    /// before copying; no malformed trailing segment can partially replace
    /// the inactive bank, and no resident bytes are copied or zeroed.
    pub fn load_replaceable(
        &self,
        base: usize,
        bank: &mut [u8],
        resident: core::ops::Range<usize>,
    ) -> Result<(), ImageError> {
        let length = bank.len();
        self.load_replaceable_chunk(base, bank, resident, 0, length)
    }
    /// Bounded preparation while the old owner continues servicing guests.
    /// The immutable authenticated source and destination must remain owned
    /// across calls; every byte must be prepared before admitting execution.
    pub fn load_replaceable_chunk(
        &self,
        base: usize,
        bank: &mut [u8],
        resident: core::ops::Range<usize>,
        offset: usize,
        length: usize,
    ) -> Result<(), ImageError> {
        self.require_identity_linked()?;
        let end = base.checked_add(bank.len()).ok_or(ImageError::Range)?;
        if length == 0
            || !inside(offset, length, bank.len())
            || resident.start >= resident.end
            || resident.end > self.limit
            || end > resident.start
        {
            return Err(ImageError::Destination);
        }
        for segment in &self.segments[..self.count] {
            let segment_end = segment.guest + segment.memory;
            if !((segment.guest >= base && segment_end <= end)
                || (segment.guest >= resident.start && segment_end <= resident.end))
            {
                return Err(ImageError::Destination);
            }
        }
        bank[offset..offset + length].fill(0);
        let chunk_start = base + offset;
        let chunk_end = chunk_start + length;
        for segment in &self.segments[..self.count] {
            if segment.guest >= resident.start {
                continue;
            }
            let start = chunk_start.max(segment.guest);
            let end = chunk_end.min(segment.guest + segment.file);
            if start < end {
                let source = segment.offset + start - segment.guest;
                bank[start - base..end - base]
                    .copy_from_slice(&self.bytes[source..source + end - start]);
            }
        }
        Ok(())
    }
    /// Root replacement code requires an identity address space. A Trusty
    /// ELF may instead use its own high-half virtual mappings.
    pub fn require_identity_linked(&self) -> Result<(), ImageError> {
        if self.identity_linked {
            Ok(())
        } else {
            Err(ImageError::Destination)
        }
    }
    /// Reject any load segment, including its zero-filled tail, that would
    /// overwrite boot state owned by the loader. Validate the reservation even
    /// when no segment intersects it; callers must not copy outside the bank.
    pub fn reserve(&self, start: usize, length: usize) -> Result<(), ImageError> {
        if length == 0 || !inside(start, length, self.limit) {
            return Err(ImageError::Range);
        }
        if self.segments[..self.count]
            .iter()
            .any(|segment| start < segment.guest + segment.memory && segment.guest < start + length)
        {
            return Err(ImageError::Overlap);
        }
        Ok(())
    }
    pub fn load(&self, bank: &mut [u8]) -> Result<(), ImageError> {
        if bank.len() < self.limit {
            return Err(ImageError::Destination);
        }
        self.load_region(0, bank)
    }

    /// Load an assigned subregion without constructing a mutable slice over
    /// the running owner's memory. Segment addresses remain absolute within
    /// the image's address space; this does not relocate executable code.
    /// All destinations, including BSS, are checked before the first write.
    pub fn load_region(&self, base: usize, bank: &mut [u8]) -> Result<(), ImageError> {
        let end = base
            .checked_add(bank.len())
            .ok_or(ImageError::Destination)?;
        if self.segments[..self.count]
            .iter()
            .any(|segment| segment.guest < base || segment.guest + segment.memory > end)
        {
            return Err(ImageError::Destination);
        }
        for segment in &self.segments[..self.count] {
            let start = segment.guest - base;
            bank[start..start + segment.file]
                .copy_from_slice(&self.bytes[segment.offset..segment.offset + segment.file]);
            bank[start + segment.file..start + segment.memory].fill(0);
        }
        Ok(())
    }
}
fn inside(start: usize, length: usize, limit: usize) -> bool {
    start.checked_add(length).is_some_and(|end| end <= limit)
}
fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
}
fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}
fn u64_at(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}

#[cfg(test)]
#[path = "image_tests.rs"]
mod tests;
