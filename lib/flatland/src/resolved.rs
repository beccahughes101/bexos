//! Resolved geometry is shared by rendering and input. No client handles are
//! dereferenced here; the service supplies validated mappings at render time.
use crate::{Damage, Error, Surface, scene::Graph};
use alloc::{collections::BTreeMap, vec::Vec};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}
impl Rect {
    pub fn contains(self, x: f64, y: f64) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.width && y < self.y + self.height
    }
    pub fn intersect(self, other: Self) -> Self {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        Self {
            x,
            y,
            width: (self.x + self.width).min(other.x + other.width).max(x) - x,
            height: (self.y + self.height).min(other.y + other.height).max(y) - y,
        }
    }
    pub fn pixels(self, target: Surface) -> Damage {
        let x = self.x.max(0.).min(target.width as f64) as u32;
        let y = self.y.max(0.).min(target.height as f64) as u32;
        let ceil = |v: f64| {
            let n = v as u32;
            n + u32::from(v > n as f64)
        };
        let right = ceil((self.x + self.width).max(x as f64).min(target.width as f64));
        let bottom = ceil(
            (self.y + self.height)
                .max(y as f64)
                .min(target.height as f64),
        );
        Damage {
            x,
            y,
            width: right - x,
            height: bottom - y,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    pub x: f64,
    pub y: f64,
    pub sx: f64,
    pub sy: f64,
}
impl Default for Transform {
    fn default() -> Self {
        Self {
            x: 0.,
            y: 0.,
            sx: 1.,
            sy: 1.,
        }
    }
}
impl Transform {
    pub fn local(self, x: f64, y: f64) -> (f64, f64) {
        ((x - self.x) / self.sx, (y - self.y) / self.sy)
    }
    pub fn rect(self, width: u32, height: u32) -> Rect {
        Rect {
            x: self.x,
            y: self.y,
            width: width as f64 * self.sx,
            height: height as f64 * self.sy,
        }
    }
    pub(crate) fn child(self, translation: (i32, i32), scale: (f32, f32)) -> Result<Self, Error> {
        let t = Self {
            x: self.x + translation.0 as f64 * self.sx,
            y: self.y + translation.1 as f64 * self.sy,
            sx: self.sx * scale.0 as f64,
            sy: self.sy * scale.1 as f64,
        };
        if !t.x.is_finite()
            || !t.y.is_finite()
            || !t.sx.is_finite()
            || !t.sy.is_finite()
            || t.sx <= 0.
            || t.sy <= 0.
            || !(1. / t.sx).is_finite()
            || !(1. / t.sy).is_finite()
        {
            return Err(Error::Bounds);
        }
        Ok(t)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Item {
    pub effects: crate::effects::Effects,
    pub node: u64,
    pub buffer: u64,
    pub surface: Surface,
    pub transform: Transform,
    pub inverse_scale: (f64, f64),
    pub opacity: f32,
    pub visible: Rect,
    pub pixels: Damage,
}
impl Item {
    pub fn contains(&self, x: f64, y: f64) -> bool {
        let (local_x, local_y) = self.local(x, y);
        self.visible.contains(x, y) && self.effects.contains(self.surface, local_x, local_y)
    }
    pub fn local(&self, x: f64, y: f64) -> (f64, f64) {
        (
            (x - self.transform.x) * self.inverse_scale.0,
            (y - self.transform.y) * self.inverse_scale.1,
        )
    }
}
#[derive(Default, Debug)]
pub struct Snapshot {
    pub items: Vec<Item>,
    cache: BTreeMap<u64, CachedGeometry>,
    scratch: Vec<Item>,
    updates: Vec<(u64, CachedGeometry)>,
    recomputed: usize,
}
#[derive(Clone, Copy, Debug, PartialEq)]
struct GeometryKey {
    effects: crate::effects::Effects,
    translation: (i32, i32),
    layout_offset: (f32, f32),
    layout_size: Option<(f32, f32)>,
    scale: (f32, f32),
    opacity: f32,
    clip: Option<(u32, u32)>,
    content: Option<(u64, Surface)>,
    parent: Transform,
    parent_opacity: f32,
    parent_clip: Rect,
    target: Surface,
}
#[derive(Clone, Copy, Debug)]
struct CachedGeometry {
    key: GeometryKey,
    transform: Transform,
    opacity: f32,
    clip: Rect,
    item: Option<Item>,
}
impl Snapshot {
    /// Compilation is transactional: invalid geometry never replaces the
    /// previously usable snapshot. Scratch storage can be reused by callers.
    pub fn compile(&mut self, graph: &Graph, target: Surface) -> Result<(), Error> {
        self.compile_in(
            graph,
            target,
            Transform::default(),
            1.,
            Rect {
                x: 0.,
                y: 0.,
                width: target.width as f64,
                height: target.height as f64,
            },
        )
    }
    pub fn compile_in(
        &mut self,
        graph: &Graph,
        target: Surface,
        parent: Transform,
        opacity: f32,
        clip: Rect,
    ) -> Result<(), Error> {
        graph.validate()?;
        target.validate(u64::MAX)?;
        self.scratch.clear();
        self.updates.clear();
        if let Some(root) = graph.root {
            self.visit(graph, root, target, parent, opacity, clip)?;
        }
        core::mem::swap(&mut self.items, &mut self.scratch);
        self.recomputed = self.updates.len();
        for (id, entry) in self.updates.drain(..) {
            self.cache.insert(id, entry);
        }
        self.cache.retain(|id, _| graph.nodes.contains_key(id));
        Ok(())
    }
    /// Number of nodes whose inherited/local geometry changed in the last
    /// successful compilation. Traversal still updates flattened paint order.
    pub fn recomputed_nodes(&self) -> usize {
        self.recomputed
    }
    /// Call for a node in the committed graph. Hidden roots return invisible
    /// geometry; consumers must never inspect entries from detached subtrees.
    pub fn geometry(&self, node: u64) -> Option<(Transform, Rect, bool)> {
        let entry = self.cache.get(&node)?;
        Some((
            entry.transform,
            entry.clip,
            entry.opacity > 0. && entry.clip.width > 0. && entry.clip.height > 0.,
        ))
    }
    pub fn inherited_geometry(&self, node: u64) -> Option<(Transform, Rect, f32)> {
        let entry = self.cache.get(&node)?;
        Some((entry.transform, entry.clip, entry.opacity))
    }
    fn visit(
        &mut self,
        graph: &Graph,
        id: u64,
        target: Surface,
        parent: Transform,
        opacity: f32,
        clip: Rect,
    ) -> Result<(), Error> {
        let n = &graph.nodes[&id];
        let key = GeometryKey {
            effects: n.effects,
            translation: n.translation,
            layout_offset: n.layout_offset,
            layout_size: n.layout_size,
            scale: n.scale,
            opacity: n.opacity,
            clip: n.clip,
            content: n.content,
            parent,
            parent_opacity: opacity,
            parent_clip: clip,
            target,
        };
        let entry = if let Some(cached) = self.cache.get(&id).filter(|c| c.key == key) {
            *cached
        } else {
            let mut transform = parent.child(n.translation, n.scale)?;
            transform.x += n.layout_offset.0 as f64 * parent.sx;
            transform.y += n.layout_offset.1 as f64 * parent.sy;
            let opacity = opacity * n.opacity;
            let clip = n
                .clip
                .map_or(clip, |(w, h)| clip.intersect(transform.rect(w, h)));
            let mut item = None;
            if let Some((buffer, surface)) = n.content {
                surface.validate(u64::MAX)?;
                let mut content_transform = transform;
                if let Some((width, height)) = n.layout_size {
                    content_transform.sx *= width as f64 / surface.width as f64;
                    content_transform.sy *= height as f64 / surface.height as f64;
                }
                let visible = clip.intersect(content_transform.rect(surface.width, surface.height));
                let pixels = visible.pixels(target);
                if opacity > 0. && pixels.width != 0 && pixels.height != 0 {
                    item = Some(Item {
                        effects: n.effects,
                        node: id,
                        buffer,
                        surface,
                        transform: content_transform,
                        inverse_scale: (1. / content_transform.sx, 1. / content_transform.sy),
                        opacity,
                        visible,
                        pixels,
                    });
                }
            }
            let entry = CachedGeometry {
                key,
                transform,
                opacity,
                clip,
                item,
            };
            self.updates.push((id, entry));
            entry
        };
        let CachedGeometry {
            transform,
            opacity,
            clip,
            item,
            ..
        } = entry;
        if opacity == 0. || clip.width == 0. || clip.height == 0. {
            return Ok(());
        }
        if let Some(item) = item {
            self.scratch.push(item);
        }
        for child in &n.children {
            self.visit(graph, *child, target, transform, opacity, clip)?;
        }
        Ok(())
    }
    pub fn hit_test(&self, x: f64, y: f64) -> Option<(u64, f64, f64)> {
        self.items
            .iter()
            .rev()
            .find(|item| item.contains(x, y))
            .map(|item| {
                let (x, y) = item.local(x, y);
                (item.node, x, y)
            })
    }
    pub fn bounds(&self) -> Option<Damage> {
        self.items.iter().map(|i| i.pixels).reduce(Damage::union)
    }
}
