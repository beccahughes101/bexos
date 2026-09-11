//! Selector matching and the CSS cascade run only when logical inputs change.
//! Computed values stay process-local and are rebuilt after replacement.
use crate::{
    Error, Theme,
    dom::{Arena, Element, Node},
};
use selectors::context::{
    MatchingContext, MatchingForInvalidation, MatchingMode, NeedsSelectorFlags, SelectorCaches,
};
use std::collections::BTreeMap;
use style::{
    applicable_declarations::ApplicableDeclarationList,
    context::{CascadeInputs, QuirksMode, TreeCountingCaches},
    device::{Device, servo::FontMetricsProvider},
    dom::TElement,
    media_queries::MediaType,
    properties::{ComputedValues, FirstLineReparenting, style_structs::Font},
    queries::values::PrefersColorScheme,
    rule_cache::RuleCacheConditions,
    servo::media_features::PointerCapabilities,
    servo_arc::Arc,
    shared_lock::StylesheetGuards,
    stylist::{RuleInclusion, Stylist},
    values::specified::position::PositionTryFallbacksTryTactic,
};
pub struct Resolver {
    theme: Theme,
    stylist: Stylist,
    previous: Vec<Node>,
    computed: BTreeMap<u64, Arc<ComputedValues>>,
    invalidated: bool,
    generation: u64,
    initial: Arc<ComputedValues>,
}
impl Resolver {
    pub fn new(
        theme: Theme,
        width: f32,
        height: f32,
        scale: f32,
        dark: bool,
        metrics: Box<dyn FontMetricsProvider>,
    ) -> Result<Self, Error> {
        if !width.is_finite()
            || !height.is_finite()
            || !scale.is_finite()
            || !(1. ..=16384.).contains(&width)
            || !(1. ..=16384.).contains(&height)
            || !(0.1..=16.).contains(&scale)
        {
            return Err(Error::InvalidViewport);
        }
        let initial = ComputedValues::initial_values_with_font_override(Font::initial_values());
        let device = Device::new(
            MediaType::screen(),
            QuirksMode::NoQuirks,
            euclid::Size2D::new(width, height),
            euclid::Size2D::new(width * scale, height * scale),
            euclid::Scale::new(scale),
            metrics,
            initial.clone(),
            if dark {
                PrefersColorScheme::Dark
            } else {
                PrefersColorScheme::Light
            },
            PointerCapabilities::default(),
            PointerCapabilities::default(),
        );
        let mut stylist = Stylist::new(device, QuirksMode::NoQuirks);
        {
            let guard = theme.lock().read();
            stylist.append_stylesheet(
                style::stylesheets::DocumentStyleSheet(theme.stylesheet().clone()),
                &guard,
            );
            stylist.flush(&StylesheetGuards::same(&guard));
        }
        Ok(Self {
            theme,
            stylist,
            previous: Vec::new(),
            computed: BTreeMap::new(),
            invalidated: true,
            generation: 0,
            initial,
        })
    }
    pub fn update_theme(&mut self, css: &str) -> Result<bool, Error> {
        let old = self.theme.stylesheet().clone();
        if !self.theme.update(css)? {
            return Ok(false);
        }
        let guard = self.theme.lock().read();
        self.stylist
            .remove_stylesheet(style::stylesheets::DocumentStyleSheet(old), &guard);
        self.stylist.append_stylesheet(
            style::stylesheets::DocumentStyleSheet(self.theme.stylesheet().clone()),
            &guard,
        );
        self.stylist.flush(&StylesheetGuards::same(&guard));
        self.invalidated = true;
        Ok(true)
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn get(&self, id: u64) -> Option<&ComputedValues> {
        self.computed.get(&id).map(|v| &**v)
    }
    /// Sibling order is the order of `nodes`; parent IDs may precede or follow
    /// children. Validation and all cascades complete before replacing results.
    pub fn resolve(&mut self, nodes: &[Node]) -> Result<bool, Error> {
        if !self.invalidated && self.previous == nodes {
            return Ok(false);
        }
        let arena = Arena::build(nodes, self.theme.lock().clone())?;
        let guard = self.theme.lock().read();
        let guards = StylesheetGuards::same(&guard);
        let mut computed: BTreeMap<u64, Arc<ComputedValues>> = BTreeMap::new();
        let mut selector_caches = SelectorCaches::default();
        let mut counting = TreeCountingCaches::default();
        for index in arena.preorder() {
            let element = Element {
                arena: &arena,
                index,
            };
            if element.node().parent == Some(0) {
                self.stylist.device().set_root_style(&self.initial);
            }
            let mut declarations = ApplicableDeclarationList::new();
            let mut context = MatchingContext::new(
                MatchingMode::Normal,
                None,
                &mut selector_caches,
                QuirksMode::NoQuirks,
                NeedsSelectorFlags::No,
                MatchingForInvalidation::No,
            );
            self.stylist.push_applicable_declarations(
                element,
                None,
                element.style_attribute(),
                None,
                Default::default(),
                RuleInclusion::All,
                &mut declarations,
                &mut context,
            );
            let inputs = CascadeInputs {
                rules: Some(
                    self.stylist
                        .rule_tree()
                        .compute_rule_node(&mut declarations, &guards),
                ),
                flags: context.extra_data.cascade_input_flags,
                ..Default::default()
            };
            let parent = element
                .node()
                .parent
                .filter(|i| *i != 0)
                .and_then(|i| computed.get(&arena.nodes[i].id))
                .map(|v| &**v);
            let result = self.stylist.cascade_style_and_visited(
                Some(element),
                None,
                &inputs,
                &guards,
                parent,
                parent,
                FirstLineReparenting::No,
                &PositionTryFallbacksTryTactic::default(),
                None,
                &mut RuleCacheConditions::default(),
                &mut counting,
            );
            if element.node().parent == Some(0) {
                self.stylist.device().set_root_style(&result);
            }
            computed.insert(element.node().id, result);
        }
        self.computed = computed;
        self.previous = nodes.to_vec();
        self.invalidated = false;
        self.generation = self.generation.wrapping_add(1);
        Ok(true)
    }
}
