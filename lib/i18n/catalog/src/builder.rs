//! Build-side table writer, shared with malformed-artifact tests.
use crate::{
    Error, Node,
    format::{MAGIC, MAX_BYTES, MAX_NODES, hash},
};
use alloc::{
    collections::BTreeMap,
    string::{String, ToString},
    vec,
    vec::Vec,
};
#[derive(Default)]
pub struct Builder {
    pub nodes: Vec<Node>,
    pub edges: Vec<u32>,
    strings: Vec<u8>,
    intern: BTreeMap<String, (u32, u32)>,
    entries: BTreeMap<(String, String), u32>,
}
impl Builder {
    pub fn text(&mut self, s: &str) -> (u32, u32) {
        if let Some(v) = self.intern.get(s) {
            return *v;
        }
        let v = (self.strings.len() as u32, s.len() as u32);
        self.strings.extend_from_slice(s.as_bytes());
        self.intern.insert(s.to_string(), v);
        v
    }
    pub fn node(&mut self, n: Node) -> u32 {
        let id = self.nodes.len() as u32;
        self.nodes.push(n);
        id
    }
    pub fn string_node(&mut self, kind: u32, s: &str) -> u32 {
        let (a, b) = self.text(s);
        self.node(Node {
            kind,
            a,
            b,
            c: 0,
            d: 0,
        })
    }
    pub fn edge_list(&mut self, items: &[u32]) -> (u32, u32) {
        let at = self.edges.len() as u32;
        self.edges.extend_from_slice(items);
        (at, items.len() as u32)
    }
    pub fn entry(&mut self, locale: &str, key: &str, node: u32) -> Result<(), Error> {
        if locale.len() > 32 || key.is_empty() || key.len() > 256 {
            return Err(Error::Bounds);
        }
        if self
            .entries
            .insert((locale.to_string(), key.to_string()), node)
            .is_some()
        {
            return Err(Error::Malformed);
        }
        Ok(())
    }
    pub fn finish(mut self, default_locale: &str) -> Result<Vec<u8>, Error> {
        if self.entries.is_empty() || self.nodes.len() > MAX_NODES || self.entries.len() > MAX_NODES
        {
            return Err(Error::Bounds);
        }
        let (dl, dl_len) = self.text(default_locale);
        let entries = core::mem::take(&mut self.entries);
        if !entries.keys().any(|(l, _)| l == default_locale) {
            return Err(Error::Missing);
        }
        let slots = (entries.len() * 2).next_power_of_two();
        let mut table = vec![0u32; slots];
        let mut records = Vec::new();
        for (index, ((locale, key), node)) in entries.into_iter().enumerate() {
            let mut slot = hash(&locale, &key) as usize & (slots - 1);
            while table[slot] != 0 {
                slot = (slot + 1) & (slots - 1);
            }
            table[slot] = index as u32 + 1;
            let (a, b) = self.text(&locale);
            let (c, d) = self.text(&key);
            records.extend([a, b, c, d, node]);
        }
        let mut out = MAGIC.to_vec();
        let header = [
            slots as u32,
            records.len() as u32 / 5,
            self.nodes.len() as u32,
            self.edges.len() as u32,
            dl,
            dl_len,
        ];
        for w in header.into_iter().chain(table).chain(records) {
            out.extend_from_slice(&w.to_le_bytes());
        }
        for n in self.nodes {
            for w in [n.kind, n.a, n.b, n.c, n.d] {
                out.extend_from_slice(&w.to_le_bytes());
            }
        }
        for w in self.edges {
            out.extend_from_slice(&w.to_le_bytes());
        }
        out.extend(self.strings);
        if out.len() > MAX_BYTES {
            return Err(Error::Bounds);
        }
        crate::Catalog::parse(&out)?;
        Ok(out)
    }
}
