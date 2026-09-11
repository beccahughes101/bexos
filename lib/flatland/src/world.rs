//! Shared inter-view topology, committed geometry and flattened paint order.
use crate::{
    Error, Surface,
    resolved::{Item, Rect, Snapshot, Transform},
    scene::{Graph, Session},
};
use alloc::{collections::BTreeMap, vec::Vec};

pub trait GraphSource {
    fn graph(&self, id: u64) -> Option<&Graph>;
    fn graphs(&self) -> impl Iterator<Item = (u64, &Graph)>;
}
impl GraphSource for BTreeMap<u64, Session> {
    fn graph(&self, id: u64) -> Option<&Graph> {
        self.get(&id).map(|s| &s.committed)
    }
    fn graphs(&self) -> impl Iterator<Item = (u64, &Graph)> {
        self.iter().map(|(id, s)| (*id, &s.committed))
    }
}
impl GraphSource for BTreeMap<u64, &Graph> {
    fn graph(&self, id: u64) -> Option<&Graph> {
        self.get(&id).copied()
    }
    fn graphs(&self) -> impl Iterator<Item = (u64, &Graph)> {
        self.iter().map(|(id, g)| (*id, *g))
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Paint {
    pub view: u64,
    pub item: usize,
}
#[derive(Default)]
pub struct World {
    pub paint: Vec<Paint>,
    scratch: Vec<Paint>,
}
impl World {
    /// A view may have one embedding parent. Top-level order comes from the
    /// privileged shell, while children paint immediately after host content.
    pub fn compile(
        &mut self,
        graphs: &impl GraphSource,
        roots: &[u64],
        target: Surface,
        snapshots: &mut BTreeMap<u64, Snapshot>,
    ) -> Result<(), Error> {
        target.validate(u64::MAX)?;
        Self::validate(graphs)?;
        Self::validate_geometry(graphs, roots)?;
        if roots.iter().any(|root| {
            graphs
                .graphs()
                .any(|(_, g)| g.nodes.values().any(|n| n.embedded == Some(*root)))
        }) {
            return Err(Error::Invalid);
        }
        self.scratch.clear();
        let mut visited = [0; 16];
        let mut count = 0;
        let full = Rect {
            x: 0.,
            y: 0.,
            width: target.width as f64,
            height: target.height as f64,
        };
        for root in roots {
            self.view(
                *root,
                graphs,
                target,
                snapshots,
                Transform::default(),
                full,
                1.,
                &mut visited,
                &mut count,
            )?;
        }
        for (id, snapshot) in snapshots.iter_mut() {
            if !visited[..count].contains(id) {
                snapshot.items.clear();
            }
        }
        core::mem::swap(&mut self.paint, &mut self.scratch);
        Ok(())
    }
    pub fn validate(graphs: &impl GraphSource) -> Result<(), Error> {
        if graphs.graphs().count() > 16 {
            return Err(Error::Bounds);
        }
        let mut children = [0; 16];
        let mut count = 0;
        for graph in graphs.graphs().map(|(_, g)| g) {
            graph.validate()?;
            for node in graph.nodes.values() {
                if let Some(child) = node.embedded {
                    // Missing children become empty content after disconnect.
                    if !graphs.graph(child).is_some() {
                        continue;
                    }
                    if count == 16 || children[..count].contains(&child) {
                        return Err(Error::Invalid);
                    }
                    children[count] = child;
                    count += 1;
                }
            }
        }
        fn visit(
            id: u64,
            graphs: &impl GraphSource,
            path: &mut [u64; 16],
            depth: usize,
        ) -> Result<(), Error> {
            if depth == 16 || path[..depth].contains(&id) {
                return Err(Error::Invalid);
            }
            path[depth] = id;
            for node in graphs.graph(id).unwrap().nodes.values() {
                if let Some(child) = node.embedded.filter(|child| graphs.graph(*child).is_some()) {
                    visit(child, graphs, path, depth + 1)?;
                }
            }
            Ok(())
        }
        for (id, _) in graphs.graphs() {
            visit(id, graphs, &mut [0; 16], 0)?;
        }
        Ok(())
    }
    pub fn validate_geometry(graphs: &impl GraphSource, roots: &[u64]) -> Result<(), Error> {
        fn node(
            view: u64,
            id: u64,
            parent: Transform,
            graphs: &impl GraphSource,
            depth: usize,
        ) -> Result<(), Error> {
            if depth >= 256 {
                return Err(Error::Bounds);
            }
            let n = &graphs.graph(view).unwrap().nodes[&id];
            let t = parent.child(n.translation, n.scale)?;
            if !(t.x + 4096. * t.sx).is_finite() || !(t.y + 4096. * t.sy).is_finite() {
                return Err(Error::Bounds);
            }
            if let Some(child) = n.embedded {
                if let Some(root) = graphs.graph(child).and_then(|g| g.root) {
                    node(child, root, t, graphs, depth + 1)?;
                }
            }
            for child in &n.children {
                node(view, *child, t, graphs, depth + 1)?;
            }
            Ok(())
        }
        for (index, root) in roots.iter().enumerate() {
            if roots[..index].contains(root) {
                return Err(Error::Invalid);
            }
            if let Some(node_id) = graphs.graph(*root).and_then(|g| g.root) {
                node(*root, node_id, Transform::default(), graphs, 0)?;
            }
        }
        Ok(())
    }
    fn view(
        &mut self,
        id: u64,
        graphs: &impl GraphSource,
        target: Surface,
        snapshots: &mut BTreeMap<u64, Snapshot>,
        parent: Transform,
        clip: Rect,
        opacity: f32,
        visited: &mut [u64; 16],
        count: &mut usize,
    ) -> Result<(), Error> {
        let Some(graph) = graphs.graph(id) else {
            return Ok(());
        };
        if visited[..*count].contains(&id) || *count == 16 {
            return Err(Error::Invalid);
        }
        visited[*count] = id;
        *count += 1;
        snapshots
            .entry(id)
            .or_default()
            .compile_in(graph, target, parent, opacity, clip)?;
        if let Some(root) = graph.root {
            self.node(id, root, graphs, target, snapshots, visited, count, &mut 0)?;
        }
        Ok(())
    }
    fn node(
        &mut self,
        view: u64,
        node: u64,
        graphs: &impl GraphSource,
        target: Surface,
        snapshots: &mut BTreeMap<u64, Snapshot>,
        visited: &mut [u64; 16],
        count: &mut usize,
        cursor: &mut usize,
    ) -> Result<(), Error> {
        let graph = graphs.graph(view).unwrap();
        let n = &graph.nodes[&node];
        let snapshot = &snapshots[&view];
        let Some((transform, clip, opacity)) = snapshot.inherited_geometry(node) else {
            return Ok(());
        };
        if opacity == 0. || clip.width == 0. || clip.height == 0. {
            return Ok(());
        }
        if snapshot
            .items
            .get(*cursor)
            .is_some_and(|item| item.node == node)
        {
            self.scratch.push(Paint {
                view,
                item: *cursor,
            });
            *cursor += 1;
        }
        if let Some(child) = n.embedded {
            self.view(
                child, graphs, target, snapshots, transform, clip, opacity, visited, count,
            )?;
        }
        for child in &n.children {
            self.node(
                view, *child, graphs, target, snapshots, visited, count, cursor,
            )?;
        }
        Ok(())
    }
    pub fn items<'a>(
        &'a self,
        snapshots: &'a BTreeMap<u64, Snapshot>,
    ) -> impl DoubleEndedIterator<Item = (u64, &'a Item)> {
        self.paint.iter().filter_map(|p| {
            snapshots
                .get(&p.view)?
                .items
                .get(p.item)
                .map(|i| (p.view, i))
        })
    }
}
