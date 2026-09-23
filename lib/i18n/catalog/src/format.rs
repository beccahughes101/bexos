use core::str;

pub const MAGIC: &[u8; 8] = b"BEXRES\0\x01";
pub const HEADER: usize = 32;
pub const NODE: usize = 20;
pub const ENTRY: usize = 20;
pub const MAX_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_NODES: usize = 262_144;
pub mod kind {
    pub const TEXT: u32 = 0;
    pub const NUMBER: u32 = 1;
    pub const VARIABLE: u32 = 2;
    pub const PATTERN: u32 = 3;
    pub const MESSAGE: u32 = 4;
    pub const TERM: u32 = 5;
    pub const FUNCTION: u32 = 6;
    pub const SELECT: u32 = 7;
    pub const VARIANT: u32 = 8;
    pub const ARGUMENT: u32 = 9;
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Malformed,
    Bounds,
    Missing,
    MissingVariant,
    MissingArgument,
    Unsupported,
    EvaluationLimit,
    InvalidNumber,
}
#[derive(Clone, Copy, Debug)]
pub struct Node {
    pub kind: u32,
    pub a: u32,
    pub b: u32,
    pub c: u32,
    pub d: u32,
}
#[derive(Clone, Copy)]
pub struct Catalog<'a> {
    bytes: &'a [u8],
    slots: usize,
    entries: usize,
    nodes: usize,
    edge_count: usize,
    entry_start: usize,
    node_start: usize,
    edge_start: usize,
    string_start: usize,
}
pub fn hash(locale: &str, key: &str) -> u64 {
    locale
        .bytes()
        .chain(core::iter::once(0))
        .chain(key.bytes())
        .fold(0xcbf29ce484222325, |h, b| {
            (h ^ u64::from(b)).wrapping_mul(0x100000001b3)
        })
}
pub fn word(bytes: &[u8], at: usize) -> Result<u32, Error> {
    Ok(u32::from_le_bytes(
        bytes
            .get(at..at.checked_add(4).ok_or(Error::Bounds)?)
            .ok_or(Error::Malformed)?
            .try_into()
            .map_err(|_| Error::Malformed)?,
    ))
}
impl<'a> Catalog<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, Error> {
        if bytes.len() > MAX_BYTES || bytes.get(..8) != Some(MAGIC) {
            return Err(Error::Malformed);
        }
        let slots = word(bytes, 8)? as usize;
        let entries = word(bytes, 12)? as usize;
        let nodes = word(bytes, 16)? as usize;
        let edge_count = word(bytes, 20)? as usize;
        if entries == 0
            || !slots.is_power_of_two()
            || slots > MAX_NODES * 4
            || entries > slots / 2
            || nodes > MAX_NODES
            || edge_count > MAX_NODES * 8
        {
            return Err(Error::Bounds);
        }
        let entry_start = HEADER + slots * 4;
        let node_start = entry_start + entries * ENTRY;
        let edge_start = node_start + nodes * NODE;
        let string_start = edge_start + edge_count * 4;
        if string_start > bytes.len() {
            return Err(Error::Malformed);
        }
        let out = Self {
            bytes,
            slots,
            entries,
            nodes,
            edge_count,
            entry_start,
            node_start,
            edge_start,
            string_start,
        };
        out.string(word(bytes, 24)?, word(bytes, 28)?)?;
        let mut seen = alloc::vec![false; entries];
        for s in 0..slots {
            let index = word(bytes, HEADER + s * 4)? as usize;
            if index > entries {
                return Err(Error::Malformed);
            }
            if index != 0 {
                if core::mem::replace(&mut seen[index - 1], true) {
                    return Err(Error::Malformed);
                }
            }
        }
        if seen.iter().any(|s| !s) {
            return Err(Error::Malformed);
        }
        let mut previous = None;
        for i in 0..entries {
            let (locale, key, node) = out.entry(i)?;
            if locale.is_empty()
                || locale.len() > 32
                || key.is_empty()
                || key.len() > 256
                || previous.is_some_and(|p| p >= (locale, key))
            {
                return Err(Error::Malformed);
            }
            previous = Some((locale, key));
            if node as usize >= nodes || out.find(locale, key)? != node {
                return Err(Error::Malformed);
            }
        }
        for i in 0..nodes {
            out.validate_node(out.node(i as u32)?)?;
        }
        Ok(out)
    }
    fn validate_node(&self, n: Node) -> Result<(), Error> {
        match n.kind {
            kind::TEXT | kind::NUMBER | kind::VARIABLE => {
                self.string(n.a, n.b)?;
            }
            kind::PATTERN => {
                for i in self.edges(n.a, n.b)? {
                    self.node(i?)?;
                }
            }
            kind::MESSAGE | kind::TERM | kind::FUNCTION => {
                self.string(n.a, n.b)?;
                for i in self.edges(n.c, n.d)? {
                    if self.node(i?)?.kind != kind::ARGUMENT {
                        return Err(Error::Malformed);
                    }
                }
            }
            kind::SELECT => {
                self.node(n.a)?;
                if n.d >= n.c {
                    return Err(Error::Malformed);
                }
                for i in self.edges(n.b, n.c)? {
                    if self.node(i?)?.kind != kind::VARIANT {
                        return Err(Error::Malformed);
                    }
                }
            }
            kind::VARIANT => {
                let key = self.node(n.a)?;
                if !matches!(key.kind, kind::TEXT | kind::NUMBER) {
                    return Err(Error::Malformed);
                }
                self.node(n.b)?;
            }
            kind::ARGUMENT => {
                self.string(n.a, n.b)?;
                self.node(n.c)?;
            }
            _ => return Err(Error::Malformed),
        }
        Ok(())
    }
    pub fn default_locale(&self) -> &'a str {
        self.string(word(self.bytes, 24).unwrap(), word(self.bytes, 28).unwrap())
            .unwrap()
    }
    pub fn string(&self, offset: u32, len: u32) -> Result<&'a str, Error> {
        let start = self
            .string_start
            .checked_add(offset as usize)
            .ok_or(Error::Bounds)?;
        str::from_utf8(
            self.bytes
                .get(start..start.checked_add(len as usize).ok_or(Error::Bounds)?)
                .ok_or(Error::Malformed)?,
        )
        .map_err(|_| Error::Malformed)
    }
    fn entry(&self, index: usize) -> Result<(&'a str, &'a str, u32), Error> {
        if index >= self.entries {
            return Err(Error::Bounds);
        }
        let at = self.entry_start + index * ENTRY;
        Ok((
            self.string(word(self.bytes, at)?, word(self.bytes, at + 4)?)?,
            self.string(word(self.bytes, at + 8)?, word(self.bytes, at + 12)?)?,
            word(self.bytes, at + 16)?,
        ))
    }
    pub fn find(&self, locale: &str, key: &str) -> Result<u32, Error> {
        let mut slot = hash(locale, key) as usize & (self.slots - 1);
        for _ in 0..self.slots.min(64) {
            let index = word(self.bytes, HEADER + slot * 4)?;
            if index == 0 {
                return Err(Error::Missing);
            }
            let (l, k, n) = self.entry(index as usize - 1)?;
            if l == locale && k == key {
                return Ok(n);
            }
            slot = (slot + 1) & (self.slots - 1);
        }
        Err(Error::Malformed)
    }
    pub fn node(&self, index: u32) -> Result<Node, Error> {
        if index as usize >= self.nodes {
            return Err(Error::Bounds);
        }
        let at = self.node_start + index as usize * NODE;
        Ok(Node {
            kind: word(self.bytes, at)?,
            a: word(self.bytes, at + 4)?,
            b: word(self.bytes, at + 8)?,
            c: word(self.bytes, at + 12)?,
            d: word(self.bytes, at + 16)?,
        })
    }
    pub fn edges(
        &self,
        start: u32,
        len: u32,
    ) -> Result<impl Iterator<Item = Result<u32, Error>> + '_, Error> {
        let end = start.checked_add(len).ok_or(Error::Bounds)?;
        if end as usize > self.edge_count {
            return Err(Error::Bounds);
        }
        Ok((start..end).map(|i| word(self.bytes, self.edge_start + i as usize * 4)))
    }
}
