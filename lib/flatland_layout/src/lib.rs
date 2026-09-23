//! Retained Taffy layout shared by compositor chrome and application clients.
use bexos_flatland::Error;
use std::collections::BTreeMap;
use taffy::{NodeId, TaffyTree};
pub use taffy::{
    Style,
    geometry::{Rect, Size},
    style::{
        AlignItems, AvailableSpace, Dimension, Direction, Display, FlexDirection, JustifyContent,
        LengthPercentage, LengthPercentageAuto,
    },
};
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Bounds {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}
#[derive(Default)]
pub struct LayoutTree {
    tree: TaffyTree<()>,
    nodes: BTreeMap<u64, NodeId>,
    generation: u64,
}
impl LayoutTree {
    pub fn create(&mut self, id: u64, style: Style) -> Result<(), Error> {
        if id == 0 || self.nodes.contains_key(&id) || self.nodes.len() >= 4096 {
            return Err(Error::Invalid);
        }
        let node = self.tree.new_leaf(style).map_err(|_| Error::Invalid)?;
        self.nodes.insert(id, node);
        Ok(())
    }
    fn node(&self, id: u64) -> Result<NodeId, Error> {
        self.nodes.get(&id).copied().ok_or(Error::Invalid)
    }
    pub fn set_style(&mut self, id: u64, style: Style) -> Result<(), Error> {
        self.tree
            .set_style(self.node(id)?, style)
            .map_err(|_| Error::Invalid)
    }
    pub fn set_children(&mut self, id: u64, children: &[u64]) -> Result<(), Error> {
        let parent = self.node(id)?;
        let ids: Vec<_> = children
            .iter()
            .map(|c| self.node(*c))
            .collect::<Result<_, _>>()?;
        // Taffy assumes callers provide a tree. Reject cycles and duplicates
        // before handing it topology from an IPC client.
        for (index, child) in ids.iter().enumerate() {
            if ids[..index].contains(child) {
                return Err(Error::Invalid);
            }
            let mut ancestor = Some(parent);
            while let Some(node) = ancestor {
                if node == *child {
                    return Err(Error::Invalid);
                }
                ancestor = self.tree.parent(node);
            }
            if self.tree.parent(*child).is_some_and(|p| p != parent) {
                return Err(Error::Invalid);
            }
        }
        self.tree
            .set_children(parent, &ids)
            .map_err(|_| Error::Invalid)
    }
    pub fn remove(&mut self, id: u64) -> Result<(), Error> {
        let node = self.node(id)?;
        self.tree.remove(node).map_err(|_| Error::Invalid)?;
        self.nodes.remove(&id);
        Ok(())
    }
    /// Taffy retains per-node layout caches between computations.
    pub fn compute(&mut self, root: u64, width: f32, height: f32) -> Result<u64, Error> {
        if !width.is_finite() || !height.is_finite() || width <= 0. || height <= 0. {
            return Err(Error::Bounds);
        }
        self.tree
            .compute_layout(
                self.node(root)?,
                Size {
                    width: AvailableSpace::Definite(width),
                    height: AvailableSpace::Definite(height),
                },
            )
            .map_err(|_| Error::Invalid)?;
        self.generation = self.generation.wrapping_add(1);
        Ok(self.generation)
    }
    pub fn bounds(&self, id: u64) -> Result<Bounds, Error> {
        let l = self
            .tree
            .layout(self.node(id)?)
            .map_err(|_| Error::Invalid)?;
        Ok(Bounds {
            x: l.location.x,
            y: l.location.y,
            width: l.size.width,
            height: l.size.height,
        })
    }
}

pub mod graph;
