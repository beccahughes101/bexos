use crate::{Error, Surface};
use alloc::{collections::BTreeMap, vec::Vec};
#[derive(Clone, Debug)]
pub struct Node {
    pub style: crate::style::Properties,
    pub effects: crate::effects::Effects,
    pub layout: crate::layout::Properties,
    pub layout_offset: (f32, f32),
    pub layout_size: Option<(f32, f32)>,
    pub children: Vec<u64>,
    pub translation: (i32, i32),
    pub scale: (f32, f32),
    pub clip: Option<(u32, u32)>,
    pub opacity: f32,
    pub content: Option<(u64, Surface)>,
    /// Service-resolved child view identity. A protocol token must be validated
    /// before setting this; numeric identities alone carry no authority.
    pub embedded: Option<u64>,
}
impl Default for Node {
    fn default() -> Self {
        Self {
            style: Default::default(),
            effects: Default::default(),
            layout: Default::default(),
            layout_offset: (0., 0.),
            layout_size: None,
            children: Vec::new(),
            translation: (0, 0),
            scale: (1., 1.),
            clip: None,
            opacity: 1.,
            content: None,
            embedded: None,
        }
    }
}
#[derive(Clone, Debug, Default)]
pub struct Graph {
    pub nodes: BTreeMap<u64, Node>,
    pub root: Option<u64>,
}
impl Graph {
    pub fn remove(&mut self, id: u64) -> Result<(), Error> {
        if !self.nodes.contains_key(&id) {
            return Err(Error::Invalid);
        }
        for n in self.nodes.values_mut() {
            n.children.retain(|c| *c != id);
        }
        self.nodes.remove(&id);
        if self.root == Some(id) {
            self.root = None;
        }
        Ok(())
    }
    pub fn detach(&mut self, parent: u64, child: u64) -> Result<(), Error> {
        let node = self.node(parent)?;
        let index = node
            .children
            .iter()
            .position(|id| *id == child)
            .ok_or(Error::Invalid)?;
        node.children.remove(index);
        Ok(())
    }
    pub fn reorder(&mut self, parent: u64, child: u64, index: u32) -> Result<(), Error> {
        let node = self.node(parent)?;
        if index as usize >= node.children.len() {
            return Err(Error::Bounds);
        }
        let old = node
            .children
            .iter()
            .position(|id| *id == child)
            .ok_or(Error::Invalid)?;
        node.children.remove(old);
        node.children.insert(index as usize, child);
        Ok(())
    }
    pub fn create(&mut self, id: u64) -> Result<(), Error> {
        if id == 0 || self.nodes.contains_key(&id) || self.nodes.len() >= 256 {
            return Err(Error::Invalid);
        }
        self.nodes.insert(id, Node::default());
        Ok(())
    }
    pub fn node(&mut self, id: u64) -> Result<&mut Node, Error> {
        self.nodes.get_mut(&id).ok_or(Error::Invalid)
    }
    pub fn attach(&mut self, parent: u64, child: u64) -> Result<(), Error> {
        if parent == child
            || !self.nodes.contains_key(&child)
            || self.nodes.values().any(|n| n.children.contains(&child))
        {
            return Err(Error::Invalid);
        }
        self.node(parent)?.children.push(child);
        if self.validate().is_err() {
            self.node(parent)?.children.pop();
            return Err(Error::Invalid);
        }
        Ok(())
    }
    pub fn validate(&self) -> Result<(), Error> {
        // The protocol admits at most 256 nodes. Validate topology and world
        // geometry with bounded stack storage, without per-frame tree sets.
        if self.nodes.len() > 256 || self.nodes.contains_key(&0) {
            return Err(Error::Bounds);
        }
        let mut ids = [0; 256];
        for (out, id) in ids.iter_mut().zip(self.nodes.keys()) {
            *out = *id;
        }
        let ids = &ids[..self.nodes.len()];
        let mut parents = [false; 256];
        if self.nodes.values().map(|n| n.style.bytes()).sum::<usize>() > 32 * 1024 {
            return Err(Error::Bounds);
        }
        for node in self.nodes.values() {
            node.style.validate()?;
            if node.embedded == Some(0) {
                return Err(Error::Invalid);
            }
            node.effects.validate()?;
            node.layout.validate()?;
            if node.layout_size.is_some_and(|(w, h)| {
                !w.is_finite()
                    || !h.is_finite()
                    || !(0.0..=65536.0).contains(&w)
                    || !(0.0..=65536.0).contains(&h)
            }) {
                return Err(Error::Bounds);
            }
            if ![node.layout_offset.0, node.layout_offset.1]
                .iter()
                .all(|v| v.is_finite() && v.abs() <= 65536.)
            {
                return Err(Error::Bounds);
            }
            if !node.opacity.is_finite()
                || !(0.0..=1.0).contains(&node.opacity)
                || !node.scale.0.is_finite()
                || !node.scale.1.is_finite()
                || node.scale.0 <= 0.
                || node.scale.1 <= 0.
                || node.scale.0 > 64.
                || node.scale.1 > 64.
            {
                return Err(Error::Invalid);
            }
            if let Some((_, surface)) = node.content {
                surface.validate(u64::MAX)?;
            }
            for child in &node.children {
                let index = ids.binary_search(child).map_err(|_| Error::Invalid)?;
                if core::mem::replace(&mut parents[index], true) {
                    return Err(Error::Invalid);
                }
            }
        }
        if let Some(root) = self.root {
            let index = ids.binary_search(&root).map_err(|_| Error::Invalid)?;
            if parents[index] {
                return Err(Error::Invalid);
            }
        }
        fn visit(
            g: &Graph,
            ids: &[u64],
            seen: &mut [bool; 256],
            index: usize,
            world: (f64, f64, f64, f64),
        ) -> Result<(), Error> {
            if core::mem::replace(&mut seen[index], true) {
                return Err(Error::Invalid);
            }
            let n = &g.nodes[&ids[index]];
            let (x, y, sx, sy) = world;
            let x = x + n.translation.0 as f64 * sx;
            let y = y + n.translation.1 as f64 * sy;
            let sx = sx * n.scale.0 as f64;
            let sy = sy * n.scale.1 as f64;
            if !x.is_finite()
                || !y.is_finite()
                || !sx.is_finite()
                || !sy.is_finite()
                || sx <= 0.
                || sy <= 0.
                || !(1. / sx).is_finite()
                || !(1. / sy).is_finite()
                || !(x + 4096. * sx).is_finite()
                || !(y + 4096. * sy).is_finite()
            {
                return Err(Error::Bounds);
            }
            for child in &n.children {
                visit(
                    g,
                    ids,
                    seen,
                    ids.binary_search(child).map_err(|_| Error::Invalid)?,
                    (x, y, sx, sy),
                )?;
            }
            Ok(())
        }
        let mut seen = [false; 256];
        for index in 0..ids.len() {
            if !parents[index] {
                visit(self, ids, &mut seen, index, (0., 0., 1., 1.))?;
            }
        }
        // A component with no parentless node is a detached cycle.
        if seen[..ids.len()].contains(&false) {
            return Err(Error::Invalid);
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Default)]
pub struct Session {
    pub pending: Graph,
    pub committed: Graph,
    pub presentation: u64,
}
impl Session {
    pub fn present(&mut self, time: u64) -> Result<(), Error> {
        self.pending.validate()?;
        if time < self.presentation {
            return Err(Error::Stale);
        }
        self.committed = self.pending.clone();
        self.presentation = time;
        Ok(())
    }
}
/// Compatibility entry point. Long-lived compositors should retain a resolved
/// snapshot instead of compiling a graph for every frame.
pub fn composite<B: AsRef<[u8]>>(
    graph: &Graph,
    target: Surface,
    dst: &mut [u8],
    buffer: impl FnMut(u64) -> Option<B>,
) -> Result<(), Error> {
    let mut snapshot = crate::resolved::Snapshot::default();
    snapshot.compile(graph, target)?;
    crate::composition::composite(&snapshot, target, dst, target.full(), buffer)
}
