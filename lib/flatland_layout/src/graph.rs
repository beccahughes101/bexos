//! Layout is prepared with the transaction, before it enters the frame queue.
//! Unchanged properties/topology reuse Taffy's retained layout results.
use crate::{Dimension, Display, FlexDirection, LayoutTree, Size, Style};
use bexos_flatland::{Error, Surface, scene::Graph};
use std::collections::BTreeMap;
use taffy::{
    geometry::Rect,
    prelude::{TaffyAuto, fr},
    style::{LengthPercentage, Position},
};
#[derive(Clone, PartialEq)]
struct Key {
    properties: bexos_flatland::layout::Properties,
    children: Vec<u64>,
    content_size: Option<(u32, u32)>,
    parent_manual: bool,
}
#[derive(Default)]
pub struct Cache {
    tree: LayoutTree,
    keys: BTreeMap<u64, Key>,
    target: Option<Surface>,
    root: Option<u64>,
}
impl Cache {
    pub fn prepare(&mut self, graph: &Graph, target: Surface) -> Result<Graph, Error> {
        graph.validate()?;
        let Some(root) = graph.root else {
            return Ok(graph.clone());
        };
        if !graph.nodes.values().any(|n| n.layout != Default::default()) {
            return Ok(graph.clone());
        }
        let removed: Vec<_> = self
            .keys
            .keys()
            .filter(|id| !graph.nodes.contains_key(id))
            .copied()
            .collect();
        if !removed.is_empty() {
            // Removing/reparenting invalidates Taffy's parent relation. The
            // bounded topology is rebuilt outside the steady-state frame loop.
            self.tree = LayoutTree::default();
            self.keys.clear();
        }
        let topology_changed = graph
            .nodes
            .iter()
            .any(|(id, n)| self.keys.get(id).is_some_and(|k| k.children != n.children));
        if topology_changed {
            self.tree = LayoutTree::default();
            self.keys.clear();
        }
        let mut changed = self.target != Some(target) || self.root != Some(root);
        for (id, node) in &graph.nodes {
            let p = node.layout;
            let key = Key {
                properties: p,
                children: node.children.clone(),
                content_size: node.content.map(|(_, s)| (s.width, s.height)),
                parent_manual: graph
                    .nodes
                    .values()
                    .any(|parent| parent.children.contains(id) && parent.layout.mode == 0),
            };
            if self.keys.get(id) == Some(&key) && self.target == Some(target) {
                continue;
            }
            let auto = |value: f32, fallback: Option<f32>| {
                if value >= 0. {
                    Dimension::length(value)
                } else {
                    fallback.map_or(Dimension::AUTO, Dimension::length)
                }
            };
            let size = node.content.map(|(_, s)| (s.width as f32, s.height as f32));
            let mut style = Style {
                display: if p.mode == 4 {
                    Display::None
                } else if p.mode == 3 {
                    Display::Grid
                } else {
                    Display::Flex
                },
                flex_direction: if p.mode == 5 {
                    FlexDirection::RowReverse
                } else if p.mode == 6 {
                    FlexDirection::ColumnReverse
                } else if p.mode == 2 {
                    FlexDirection::Column
                } else {
                    FlexDirection::Row
                },
                size: Size {
                    width: auto(
                        p.width,
                        if *id == root {
                            Some(target.width as f32)
                        } else {
                            size.map(|v| v.0)
                        },
                    ),
                    height: auto(
                        p.height,
                        if *id == root {
                            Some(target.height as f32)
                        } else {
                            size.map(|v| v.1)
                        },
                    ),
                },
                flex_grow: p.grow,
                gap: Size {
                    width: LengthPercentage::length(p.gap),
                    height: LengthPercentage::length(p.gap),
                },
                padding: Rect {
                    left: LengthPercentage::length(p.padding),
                    right: LengthPercentage::length(p.padding),
                    top: LengthPercentage::length(p.padding),
                    bottom: LengthPercentage::length(p.padding),
                },
                ..Default::default()
            };
            if key.parent_manual {
                style.position = Position::Absolute;
            }
            if p.mode == 3 {
                style.grid_template_columns = (0..p.columns).map(|_| fr(1.)).collect();
            }
            if self.keys.contains_key(id) {
                self.tree.set_style(*id, style)?;
            } else {
                self.tree.create(*id, style)?;
            }
            self.keys.insert(*id, key);
            changed = true;
        }
        if changed {
            for (id, node) in &graph.nodes {
                self.tree.set_children(*id, &node.children)?;
            }
            self.tree
                .compute(root, target.width as f32, target.height as f32)?;
            self.target = Some(target);
            self.root = Some(root);
        }
        let mut result = graph.clone();
        for (id, node) in &mut result.nodes {
            let bounds = self.tree.bounds(*id)?;
            node.layout_offset = (bounds.x, bounds.y);
            node.layout_size = Some((bounds.width, bounds.height));
        }
        Ok(result)
    }
}
