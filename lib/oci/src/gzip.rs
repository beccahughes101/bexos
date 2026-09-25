use super::Error;

const CODE_LENGTH_ORDER: [usize; 19] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];
const LENGTH_BASE: [usize; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LENGTH_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DISTANCE_BASE: [usize; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DISTANCE_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];

pub(super) fn decompress(bytes: &[u8], limit: usize) -> Result<Vec<u8>, Error> {
    let mut at = 0usize;
    let mut output = Vec::new();
    while at < bytes.len() {
        let member_start = output.len();
        let body = header(bytes, &mut at)?;
        let (inflated, consumed) = inflate(body, limit.saturating_sub(output.len()))?;
        at = at.checked_add(consumed).ok_or(Error::ResourceExhausted)?;
        let trailer = bytes.get(at..at + 8).ok_or(Error::InvalidLayout)?;
        at += 8;
        let expected_crc = u32::from_le_bytes(trailer[..4].try_into().unwrap());
        let expected_size = u32::from_le_bytes(trailer[4..].try_into().unwrap());
        if crc32(&inflated) != expected_crc || inflated.len() as u32 != expected_size {
            return Err(Error::InvalidDigest);
        }
        output.extend_from_slice(&inflated);
        if output.len() > limit || output.len() < member_start {
            return Err(Error::ResourceExhausted);
        }
    }
    Ok(output)
}

fn header<'a>(bytes: &'a [u8], at: &mut usize) -> Result<&'a [u8], Error> {
    let fixed = bytes.get(*at..*at + 10).ok_or(Error::InvalidLayout)?;
    if fixed[..3] != [0x1f, 0x8b, 8] || fixed[3] & 0xe0 != 0 {
        return Err(Error::InvalidLayout);
    }
    let flags = fixed[3];
    *at += 10;
    if flags & 4 != 0 {
        let length = bytes.get(*at..*at + 2).ok_or(Error::InvalidLayout)?;
        *at += 2;
        let length = u16::from_le_bytes(length.try_into().unwrap()) as usize;
        *at = at.checked_add(length).ok_or(Error::ResourceExhausted)?;
        bytes.get(..*at).ok_or(Error::InvalidLayout)?;
    }
    for flag in [8, 16] {
        if flags & flag != 0 {
            let tail = bytes.get(*at..).ok_or(Error::InvalidLayout)?;
            let length = tail
                .iter()
                .position(|byte| *byte == 0)
                .ok_or(Error::InvalidLayout)?;
            *at = at.checked_add(length + 1).ok_or(Error::ResourceExhausted)?;
        }
    }
    if flags & 2 != 0 {
        *at = at.checked_add(2).ok_or(Error::ResourceExhausted)?;
        bytes.get(..*at).ok_or(Error::InvalidLayout)?;
    }
    bytes.get(*at..).ok_or(Error::InvalidLayout)
}

fn inflate(bytes: &[u8], limit: usize) -> Result<(Vec<u8>, usize), Error> {
    let mut bits = Bits::new(bytes);
    let mut output = Vec::new();
    loop {
        let final_block = bits.read(1)? != 0;
        match bits.read(2)? {
            0 => stored(&mut bits, &mut output, limit)?,
            1 => {
                let (literal, distance) = fixed_trees()?;
                compressed(&mut bits, &mut output, limit, &literal, &distance)?;
            }
            2 => {
                let (literal, distance) = dynamic_trees(&mut bits)?;
                compressed(&mut bits, &mut output, limit, &literal, &distance)?;
            }
            _ => return Err(Error::InvalidLayout),
        }
        if final_block {
            break;
        }
    }
    Ok((output, bits.consumed_bytes()))
}

fn stored(bits: &mut Bits<'_>, output: &mut Vec<u8>, limit: usize) -> Result<(), Error> {
    bits.align();
    let length = bits.read(16)? as u16;
    let inverse = bits.read(16)? as u16;
    if length != !inverse {
        return Err(Error::InvalidLayout);
    }
    let length = length as usize;
    if output.len().saturating_add(length) > limit {
        return Err(Error::ResourceExhausted);
    }
    output.extend_from_slice(bits.bytes(length)?);
    Ok(())
}

fn fixed_trees() -> Result<(Huffman, Huffman), Error> {
    let mut lengths = vec![0; 288];
    lengths[..144].fill(8);
    lengths[144..256].fill(9);
    lengths[256..280].fill(7);
    lengths[280..].fill(8);
    Ok((Huffman::new(&lengths)?, Huffman::new(&[5; 32])?))
}

fn dynamic_trees(bits: &mut Bits<'_>) -> Result<(Huffman, Huffman), Error> {
    let literal_count = bits.read(5)? as usize + 257;
    let distance_count = bits.read(5)? as usize + 1;
    let code_count = bits.read(4)? as usize + 4;
    let mut code_lengths = [0u8; 19];
    for symbol in CODE_LENGTH_ORDER.iter().take(code_count) {
        code_lengths[*symbol] = bits.read(3)? as u8;
    }
    let code_tree = Huffman::new(&code_lengths)?;
    let total = literal_count + distance_count;
    let mut lengths = Vec::with_capacity(total);
    while lengths.len() < total {
        match code_tree.decode(bits)? {
            value @ 0..=15 => lengths.push(value as u8),
            16 => {
                let previous = *lengths.last().ok_or(Error::InvalidLayout)?;
                let count = bits.read(2)? as usize + 3;
                if lengths.len() + count > total {
                    return Err(Error::InvalidLayout);
                }
                lengths.resize(lengths.len() + count, previous);
            }
            17 => {
                let count = bits.read(3)? as usize + 3;
                if lengths.len() + count > total {
                    return Err(Error::InvalidLayout);
                }
                lengths.resize(lengths.len() + count, 0);
            }
            18 => {
                let count = bits.read(7)? as usize + 11;
                if lengths.len() + count > total {
                    return Err(Error::InvalidLayout);
                }
                lengths.resize(lengths.len() + count, 0);
            }
            _ => return Err(Error::InvalidLayout),
        }
    }
    if lengths.get(256).copied() == Some(0) {
        return Err(Error::InvalidLayout);
    }
    Ok((
        Huffman::new(&lengths[..literal_count])?,
        Huffman::new(&lengths[literal_count..])?,
    ))
}

fn compressed(
    bits: &mut Bits<'_>,
    output: &mut Vec<u8>,
    limit: usize,
    literal: &Huffman,
    distance: &Huffman,
) -> Result<(), Error> {
    loop {
        match literal.decode(bits)? {
            byte @ 0..=255 => {
                if output.len() == limit {
                    return Err(Error::ResourceExhausted);
                }
                output.push(byte as u8);
            }
            256 => return Ok(()),
            symbol @ 257..=285 => {
                let index = symbol as usize - 257;
                let length = LENGTH_BASE[index] + bits.read(LENGTH_EXTRA[index])? as usize;
                let distance_symbol = distance.decode(bits)? as usize;
                if distance_symbol >= DISTANCE_BASE.len() {
                    return Err(Error::InvalidLayout);
                }
                let distance = DISTANCE_BASE[distance_symbol]
                    + bits.read(DISTANCE_EXTRA[distance_symbol])? as usize;
                if distance == 0
                    || distance > output.len()
                    || output.len().saturating_add(length) > limit
                {
                    return Err(if output.len().saturating_add(length) > limit {
                        Error::ResourceExhausted
                    } else {
                        Error::InvalidLayout
                    });
                }
                for _ in 0..length {
                    output.push(output[output.len() - distance]);
                }
            }
            _ => return Err(Error::InvalidLayout),
        }
    }
}

struct Huffman {
    entries: Vec<(u16, u8, u16)>,
    maximum: u8,
}

impl Huffman {
    fn new(lengths: &[u8]) -> Result<Self, Error> {
        let maximum = lengths.iter().copied().max().unwrap_or(0);
        if maximum == 0 || maximum > 15 {
            return Err(Error::InvalidLayout);
        }
        let mut counts = [0u16; 16];
        for length in lengths.iter().copied().filter(|length| *length != 0) {
            counts[length as usize] = counts[length as usize]
                .checked_add(1)
                .ok_or(Error::InvalidLayout)?;
        }
        let mut left = 1i32;
        for count in counts.iter().skip(1) {
            left = left * 2 - i32::from(*count);
            if left < 0 {
                return Err(Error::InvalidLayout);
            }
        }
        let mut next = [0u16; 16];
        let mut code = 0u16;
        for length in 1..=15 {
            code = (code + counts[length - 1]) << 1;
            next[length] = code;
        }
        let mut entries = Vec::new();
        for (symbol, length) in lengths.iter().copied().enumerate() {
            if length == 0 {
                continue;
            }
            let code = next[length as usize];
            next[length as usize] += 1;
            entries.push((reverse(code, length), length, symbol as u16));
        }
        Ok(Self { entries, maximum })
    }

    fn decode(&self, bits: &mut Bits<'_>) -> Result<u16, Error> {
        let mut code = 0u16;
        for length in 1..=self.maximum {
            code |= (bits.read(1)? as u16) << (length - 1);
            if let Some((_, _, symbol)) = self
                .entries
                .iter()
                .find(|(candidate, width, _)| *width == length && *candidate == code)
            {
                return Ok(*symbol);
            }
        }
        Err(Error::InvalidLayout)
    }
}

fn reverse(mut value: u16, width: u8) -> u16 {
    let mut out = 0;
    for _ in 0..width {
        out = out << 1 | value & 1;
        value >>= 1;
    }
    out
}

struct Bits<'a> {
    bytes: &'a [u8],
    bit: usize,
}

impl<'a> Bits<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, bit: 0 }
    }

    fn read(&mut self, count: u8) -> Result<u32, Error> {
        if count > 24 || self.bit.saturating_add(count as usize) > self.bytes.len() * 8 {
            return Err(Error::InvalidLayout);
        }
        let mut out = 0u32;
        for index in 0..count {
            let byte = self.bytes[self.bit / 8];
            out |= u32::from((byte >> (self.bit % 8)) & 1) << index;
            self.bit += 1;
        }
        Ok(out)
    }

    fn align(&mut self) {
        self.bit = self.bit.div_ceil(8) * 8;
    }

    fn bytes(&mut self, count: usize) -> Result<&'a [u8], Error> {
        self.align();
        let start = self.bit / 8;
        let end = start.checked_add(count).ok_or(Error::ResourceExhausted)?;
        let bytes = self.bytes.get(start..end).ok_or(Error::InvalidLayout)?;
        self.bit = end * 8;
        Ok(bytes)
    }

    fn consumed_bytes(&self) -> usize {
        self.bit.div_ceil(8)
    }
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = crc >> 1 ^ (0xedb8_8320 & (0u32.wrapping_sub(crc & 1)));
        }
    }
    !crc
}
