//! Theme/style work belongs to transaction preparation, never the frame loop.
use crate::desktop_config::DesktopConfig;
use bexos_flatland_style::{Resolver, Theme, dom::Node, metrics::DefaultFontMetrics};
use bexos_graphics::{Error, Surface, scene::Graph};
use std::collections::BTreeMap;
pub fn element(id: u64, parent: Option<u64>, tag: &str, classes: &str, inline: &str) -> Node {
    Node {
        id,
        parent,
        tag: tag.into(),
        classes: classes.into(),
        style: inline.into(),
        attributes: BTreeMap::new(),
        state: 0,
    }
}
pub fn resolver(
    target: Surface,
    config: &DesktopConfig<'_>,
    metrics: bexos_flatland_text::metrics::Metrics,
) -> Result<Resolver, Error> {
    // Legacy configurations retain their explicit text appearance.
    let css = format!(
        "desktop {{ font-size: {}px; color: rgba({}, {}, {}, {}); }}\n{}",
        config.font_size,
        config.color[0],
        config.color[1],
        config.color[2],
        f32::from(config.color[3]) / 255.,
        config.theme_css
    );
    Resolver::new(
        Theme::new(&css).map_err(|_| Error::Invalid)?,
        target.width as f32,
        target.height as f32,
        1.,
        true,
        Box::new(DefaultFontMetrics(metrics)),
    )
    .map_err(|_| Error::Invalid)
}
pub struct Cache {
    target: Surface,
    resolver: Resolver,
    metrics: bexos_flatland_text::metrics::Metrics,
}
pub fn prepare(
    caches: &mut BTreeMap<u64, Cache>,
    view: u64,
    graph: &Graph,
    target: Surface,
    metrics: bexos_flatland_text::metrics::Metrics,
) -> Result<Graph, Error> {
    if !graph.nodes.values().any(|n| n.style.enabled()) {
        return Ok(graph.clone());
    }
    if !caches.contains_key(&view) {
        caches.insert(view, Cache::new(target, metrics)?);
    }
    caches.get_mut(&view).unwrap().prepare(graph, target)
}
impl Cache {
    pub fn new(
        target: Surface,
        metrics: bexos_flatland_text::metrics::Metrics,
    ) -> Result<Self, Error> {
        let config = DesktopConfig::decode(include_bytes!(env!("DESKTOP_CONFIG")))?;
        Ok(Self {
            target,
            resolver: resolver(target, &config, metrics)?,
            metrics,
        })
    }
    pub fn prepare(&mut self, graph: &Graph, target: Surface) -> Result<Graph, Error> {
        graph.validate()?;
        if !graph.nodes.values().any(|n| n.style.enabled()) {
            return Ok(graph.clone());
        }
        if self.target != target {
            *self = Self::new(target, self.metrics)?;
        }
        let mut nodes = Vec::with_capacity(graph.nodes.len());
        let parents: BTreeMap<_, _> = graph
            .nodes
            .iter()
            .flat_map(|(id, n)| n.children.iter().map(move |c| (*c, *id)))
            .collect();
        // Preserve explicit sibling order for structural selectors.
        let mut stack: Vec<_> = graph
            .nodes
            .keys()
            .filter(|id| !parents.contains_key(id))
            .rev()
            .copied()
            .collect();
        while let Some(id) = stack.pop() {
            let n = &graph.nodes[&id];
            let mut node = element(
                id,
                parents.get(&id).copied(),
                "node",
                &n.style.classes,
                &n.style.inline,
            );
            if !n.style.identifier.is_empty() {
                node.attributes
                    .insert("id".into(), n.style.identifier.clone());
            }
            nodes.push(node);
            stack.extend(n.children.iter().rev().copied());
        }
        self.resolver.resolve(&nodes).map_err(|_| Error::Invalid)?;
        let mut result = graph.clone();
        for (id, node) in &mut result.nodes {
            if !node.style.enabled() {
                continue;
            }
            let computed = self.resolver.get(*id).ok_or(Error::Invalid)?;
            bexos_flatland_style::properties::apply(
                computed,
                &mut node.layout,
                &mut node.opacity,
                &mut node.effects,
            )
            .map_err(|_| Error::Invalid)?;
        }
        result.validate()?;
        Ok(result)
    }
}
