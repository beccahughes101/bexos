//! A bounded retained Flatland tree adapter for Stylo. Arena references never
//! escape a resolution pass, and no DOM or computed-style pointer is migrated.
use selectors::matching::QuirksMode;
use selectors::{
    Element as SelectorElement, OpaqueElement,
    attr::{AttrSelectorOperation, CaseSensitivity, NamespaceConstraint},
    bloom::BloomFilter,
    matching::{ElementSelectorFlags, MatchingContext, VisitedHandlingMode},
    parser::SelectorImpl as SelectorTypes,
};
use std::{
    cell::Cell,
    collections::{BTreeMap, HashMap},
    fmt,
    hash::{Hash, Hasher},
};
use style::{
    Atom as WeakAtom, LocalName, Namespace,
    applicable_declarations::ApplicableDeclarationBlock,
    context::SharedStyleContext,
    data::{ElementData, ElementDataMut, ElementDataRef, ElementDataWrapper},
    dom::{LayoutIterator, NodeInfo, OpaqueNode, TDocument, TElement, TNode, TShadowRoot},
    properties::PropertyDeclarationBlock,
    selector_parser::{AttrValue, Lang, NonTSPseudoClass, PseudoElement, SelectorImpl},
    servo_arc::{Arc, ArcBorrow},
    shared_lock::{Locked, SharedRwLock},
    stylist::CascadeData,
    values::AtomIdent,
};
use stylo_dom::ElementState;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
    pub id: u64,
    pub parent: Option<u64>,
    pub tag: String,
    pub classes: String,
    pub style: String,
    pub attributes: BTreeMap<String, String>,
    pub state: u64,
}
pub struct Arena {
    pub nodes: Vec<Data>,
    pub lock: SharedRwLock,
}
pub struct Data {
    pub id: u64,
    pub parent: Option<usize>,
    pub children: Vec<usize>,
    pub tag: LocalName,
    pub namespace: Namespace,
    pub identifier: Option<AtomIdent>,
    pub classes: Vec<AtomIdent>,
    pub attributes: HashMap<LocalName, String>,
    pub inline: Arc<Locked<PropertyDeclarationBlock>>,
    pub state: ElementState,
    pub data: ElementDataWrapper,
    pub has_data: Cell<bool>,
    pub dirty: Cell<bool>,
    pub handled: Cell<bool>,
    pub children_to_process: Cell<isize>,
    pub flags: Cell<ElementSelectorFlags>,
}
#[derive(Clone, Copy)]
pub struct Element<'a> {
    pub arena: &'a Arena,
    pub index: usize,
}
impl fmt::Debug for Element<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("FlatlandStyleNode")
            .field(&self.node().id)
            .finish()
    }
}
impl PartialEq for Element<'_> {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.arena, other.arena) && self.index == other.index
    }
}
impl Eq for Element<'_> {}
impl Hash for Element<'_> {
    fn hash<H: Hasher>(&self, h: &mut H) {
        (self.arena as *const Arena).hash(h);
        self.index.hash(h);
    }
}
impl<'a> Element<'a> {
    pub fn node(&self) -> &'a Data {
        &self.arena.nodes[self.index]
    }
    fn at(&self, index: usize) -> Self {
        Self {
            arena: self.arena,
            index,
        }
    }
    fn sibling(&self, offset: isize) -> Option<Self> {
        let p = self.node().parent?;
        let children = &self.arena.nodes[p].children;
        let index = children.iter().position(|v| *v == self.index)? as isize + offset;
        if index < 0 {
            return None;
        }
        children.get(index as usize).copied().map(|i| self.at(i))
    }
}
impl NodeInfo for Element<'_> {
    fn is_element(&self) -> bool {
        self.index != 0
    }
    fn is_text_node(&self) -> bool {
        false
    }
}
impl<'a> TNode for Element<'a> {
    type ConcreteElement = Self;
    type ConcreteDocument = Self;
    type ConcreteShadowRoot = NoShadow<'a>;
    fn parent_node(&self) -> Option<Self> {
        self.node().parent.map(|i| self.at(i))
    }
    fn first_child(&self) -> Option<Self> {
        self.node().children.first().copied().map(|i| self.at(i))
    }
    fn last_child(&self) -> Option<Self> {
        self.node().children.last().copied().map(|i| self.at(i))
    }
    fn prev_sibling(&self) -> Option<Self> {
        self.sibling(-1)
    }
    fn next_sibling(&self) -> Option<Self> {
        self.sibling(1)
    }
    fn owner_doc(&self) -> Self {
        self.at(0)
    }
    fn is_in_document(&self) -> bool {
        true
    }
    fn traversal_parent(&self) -> Option<Self> {
        SelectorElement::parent_element(self)
    }
    fn opaque(&self) -> OpaqueNode {
        OpaqueNode(self.node() as *const Data as usize)
    }
    fn debug_id(self) -> usize {
        self.node().id as usize
    }
    fn as_element(&self) -> Option<Self> {
        (self.index != 0).then_some(*self)
    }
    fn as_document(&self) -> Option<Self> {
        (self.index == 0).then_some(*self)
    }
    fn as_shadow_root(&self) -> Option<NoShadow<'a>> {
        None
    }
}
impl TDocument for Element<'_> {
    type ConcreteNode = Self;
    fn as_node(&self) -> Self {
        self.at(0)
    }
    fn is_html_document(&self) -> bool {
        false
    }
    fn quirks_mode(&self) -> QuirksMode {
        QuirksMode::NoQuirks
    }
    fn shared_lock(&self) -> &SharedRwLock {
        &self.arena.lock
    }
}
/// Flatland has no shadow roots. This type is uninhabited, not a fake host.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NoShadow<'a> {
    Never(std::convert::Infallible, std::marker::PhantomData<&'a ()>),
}
impl<'a> TShadowRoot for NoShadow<'a> {
    type ConcreteNode = Element<'a>;
    fn as_node(&self) -> Element<'a> {
        match *self {
            Self::Never(never, _) => match never {},
        }
    }
    fn host(&self) -> Element<'a> {
        match *self {
            Self::Never(never, _) => match never {},
        }
    }
    fn style_data<'b>(&self) -> Option<&'b CascadeData>
    where
        Self: 'b,
    {
        match *self {
            Self::Never(never, _) => match never {},
        }
    }
}
impl SelectorElement for Element<'_> {
    type Impl = SelectorImpl;
    fn opaque(&self) -> OpaqueElement {
        OpaqueElement::new(self.node())
    }
    fn parent_element(&self) -> Option<Self> {
        self.parent_node().filter(|e| e.index != 0)
    }
    fn parent_node_is_shadow_root(&self) -> bool {
        false
    }
    fn containing_shadow_host(&self) -> Option<Self> {
        None
    }
    fn is_pseudo_element(&self) -> bool {
        false
    }
    fn prev_sibling_element(&self) -> Option<Self> {
        self.sibling(-1)
    }
    fn next_sibling_element(&self) -> Option<Self> {
        self.sibling(1)
    }
    fn first_element_child(&self) -> Option<Self> {
        self.first_child()
    }
    fn is_html_element_in_html_document(&self) -> bool {
        false
    }
    fn has_local_name(&self, name: &<SelectorImpl as SelectorTypes>::BorrowedLocalName) -> bool {
        &self.node().tag.0 == name
    }
    fn has_namespace(&self, ns: &<SelectorImpl as SelectorTypes>::BorrowedNamespaceUrl) -> bool {
        &self.node().namespace.0 == ns
    }
    fn is_same_type(&self, other: &Self) -> bool {
        self.node().tag == other.node().tag && self.node().namespace == other.node().namespace
    }
    fn attr_matches(
        &self,
        ns: &NamespaceConstraint<&Namespace>,
        name: &LocalName,
        op: &AttrSelectorOperation<&AttrValue>,
    ) -> bool {
        if matches!(ns, NamespaceConstraint::Specific(ns) if !ns.0.is_empty()) {
            return false;
        }
        self.node()
            .attributes
            .get(name)
            .is_some_and(|v| op.eval_str(v))
    }
    fn match_non_ts_pseudo_class(
        &self,
        pseudo: &NonTSPseudoClass,
        _: &mut MatchingContext<SelectorImpl>,
    ) -> bool {
        match pseudo {
            NonTSPseudoClass::Lang(language) => self.match_element_lang(None, language),
            NonTSPseudoClass::CustomState(_) | NonTSPseudoClass::ServoNonZeroBorder => false,
            _ => {
                let flag = pseudo.state_flag();
                !flag.is_empty() && self.node().state.intersects(flag)
            }
        }
    }
    fn match_pseudo_element(
        &self,
        _: &PseudoElement,
        _: &mut MatchingContext<SelectorImpl>,
    ) -> bool {
        false
    }
    fn apply_selector_flags(&self, flags: ElementSelectorFlags) {
        self.node().flags.set(self.node().flags.get() | flags);
    }
    fn is_link(&self) -> bool {
        false
    }
    fn is_html_slot_element(&self) -> bool {
        false
    }
    fn has_id(&self, id: &AtomIdent, case: CaseSensitivity) -> bool {
        self.node()
            .identifier
            .as_ref()
            .is_some_and(|v| case.eq(v.0.as_bytes(), id.0.as_bytes()))
    }
    fn has_class(&self, class: &AtomIdent, case: CaseSensitivity) -> bool {
        self.node()
            .classes
            .iter()
            .any(|v| case.eq(v.0.as_bytes(), class.0.as_bytes()))
    }
    fn has_custom_state(&self, _: &AtomIdent) -> bool {
        false
    }
    fn imported_part(&self, _: &AtomIdent) -> Option<AtomIdent> {
        None
    }
    fn is_part(&self, _: &AtomIdent) -> bool {
        false
    }
    fn is_empty(&self) -> bool {
        self.node().children.is_empty()
    }
    fn is_root(&self) -> bool {
        self.node().parent == Some(0)
    }
    // Omitting a bloom filter is conservative; selectors still match the tree.
    fn add_element_unique_hashes(&self, _: &mut BloomFilter) -> bool {
        false
    }
}
impl Arena {
    pub fn build(source: &[Node], lock: SharedRwLock) -> Result<Self, crate::Error> {
        if source.len() > 256 {
            return Err(crate::Error::TooLarge);
        }
        let mut indices = BTreeMap::new();
        let mut bytes = 0usize;
        for (index, node) in source.iter().enumerate() {
            if node.id == 0 || indices.insert(node.id, index + 1).is_some() {
                return Err(crate::Error::InvalidTree);
            }
            bytes = bytes.saturating_add(node.tag.len() + node.classes.len() + node.style.len());
            for (key, value) in &node.attributes {
                bytes = bytes.saturating_add(key.len() + value.len());
            }
            if bytes > 128 * 1024 || node.attributes.len() > 32 || node.tag.is_empty() {
                return Err(crate::Error::TooLarge);
            }
        }
        let url = style::stylesheets::UrlExtraData(Arc::new("bexos:style".parse().unwrap()));
        let empty = Node {
            id: 0,
            parent: None,
            tag: "document".into(),
            classes: String::new(),
            style: String::new(),
            attributes: BTreeMap::new(),
            state: 0,
        };
        let mut nodes = Vec::with_capacity(source.len() + 1);
        for node in std::iter::once(&empty).chain(source) {
            let parent = if node.id == 0 {
                None
            } else {
                Some(match node.parent {
                    Some(id) => *indices.get(&id).ok_or(crate::Error::InvalidTree)?,
                    None => 0,
                })
            };
            let mut attributes: HashMap<LocalName, String> = node
                .attributes
                .iter()
                .map(|(k, v)| (k.as_str().into(), v.clone()))
                .collect();
            attributes.insert("class".into(), node.classes.clone());
            attributes.insert("style".into(), node.style.clone());
            let inline = style::properties::parse_style_attribute(
                &node.style,
                &url,
                None,
                QuirksMode::NoQuirks,
                style::stylesheets::CssRuleType::Style,
            );
            nodes.push(Data {
                id: node.id,
                parent,
                children: Vec::new(),
                tag: node.tag.as_str().into(),
                namespace: "".into(),
                identifier: node.attributes.get("id").map(|v| v.as_str().into()),
                classes: node
                    .classes
                    .split_ascii_whitespace()
                    .map(Into::into)
                    .collect(),
                attributes,
                inline: Arc::new(lock.wrap(inline)),
                state: ElementState::from_bits_truncate(node.state),
                data: ElementDataWrapper::default(),
                has_data: Cell::new(false),
                dirty: Cell::new(false),
                handled: Cell::new(false),
                children_to_process: Cell::new(0),
                flags: Cell::new(ElementSelectorFlags::empty()),
            });
        }
        for index in 1..nodes.len() {
            let mut ancestor = nodes[index].parent;
            let mut depth = 0;
            while let Some(parent) = ancestor {
                if parent == index || depth > source.len() {
                    return Err(crate::Error::InvalidTree);
                }
                ancestor = nodes[parent].parent;
                depth += 1;
            }
            let parent = nodes[index].parent.unwrap();
            nodes[parent].children.push(index);
        }
        Ok(Self { nodes, lock })
    }
    pub fn preorder(&self) -> Vec<usize> {
        let mut stack: Vec<_> = self.nodes[0].children.iter().rev().copied().collect();
        let mut out = Vec::with_capacity(self.nodes.len() - 1);
        while let Some(index) = stack.pop() {
            out.push(index);
            stack.extend(self.nodes[index].children.iter().rev().copied());
        }
        out
    }
}
impl<'a> TElement for Element<'a> {
    type ConcreteNode = Self;
    type TraversalChildrenIterator = std::vec::IntoIter<Self>;
    fn as_node(&self) -> Self {
        *self
    }
    fn traversal_children(&self) -> LayoutIterator<Self::TraversalChildrenIterator> {
        LayoutIterator(
            self.node()
                .children
                .iter()
                .map(|i| self.at(*i))
                .collect::<Vec<_>>()
                .into_iter(),
        )
    }
    fn is_html_element(&self) -> bool {
        false
    }
    fn is_mathml_element(&self) -> bool {
        false
    }
    fn is_svg_element(&self) -> bool {
        false
    }
    fn style_attribute(&self) -> Option<ArcBorrow<'_, Locked<PropertyDeclarationBlock>>> {
        Some(self.node().inline.borrow_arc())
    }
    fn animation_rule(
        &self,
        _: &SharedStyleContext,
    ) -> Option<Arc<Locked<PropertyDeclarationBlock>>> {
        None
    }
    fn transition_rule(
        &self,
        _: &SharedStyleContext,
    ) -> Option<Arc<Locked<PropertyDeclarationBlock>>> {
        None
    }
    fn state(&self) -> ElementState {
        self.node().state
    }
    fn has_part_attr(&self) -> bool {
        false
    }
    fn exports_any_part(&self) -> bool {
        false
    }
    fn id(&self) -> Option<&WeakAtom> {
        self.node().identifier.as_ref().map(|i| &i.0)
    }
    fn each_class<F: FnMut(&AtomIdent)>(&self, mut f: F) {
        for class in &self.node().classes {
            f(class);
        }
    }
    fn each_custom_state<F: FnMut(&AtomIdent)>(&self, _: F) {}
    fn each_attr_name<F: FnMut(&LocalName)>(&self, mut f: F) {
        for name in self.node().attributes.keys() {
            f(name);
        }
    }
    fn has_dirty_descendants(&self) -> bool {
        self.node().dirty.get()
    }
    fn has_snapshot(&self) -> bool {
        false
    }
    fn handled_snapshot(&self) -> bool {
        self.node().handled.get()
    }
    unsafe fn set_handled_snapshot(&self) {
        self.node().handled.set(true);
    }
    unsafe fn set_dirty_descendants(&self) {
        self.node().dirty.set(true);
    }
    unsafe fn unset_dirty_descendants(&self) {
        self.node().dirty.set(false);
    }
    fn store_children_to_process(&self, n: isize) {
        self.node().children_to_process.set(n);
    }
    fn did_process_child(&self) -> isize {
        let n = self.node().children_to_process.get() - 1;
        self.node().children_to_process.set(n);
        n
    }
    unsafe fn ensure_data(&self) -> ElementDataMut<'_> {
        self.node().has_data.set(true);
        self.node().data.borrow_mut()
    }
    unsafe fn clear_data(&self) {
        *self.node().data.borrow_mut() = ElementData::default();
        self.node().has_data.set(false);
    }
    fn has_data(&self) -> bool {
        self.node().has_data.get()
    }
    fn borrow_data(&self) -> Option<ElementDataRef<'_>> {
        self.has_data().then(|| self.node().data.borrow())
    }
    fn mutate_data(&self) -> Option<ElementDataMut<'_>> {
        self.has_data().then(|| self.node().data.borrow_mut())
    }
    fn skip_item_display_fixup(&self) -> bool {
        false
    }
    fn may_have_animations(&self) -> bool {
        false
    }
    fn has_animations(&self, _: &SharedStyleContext) -> bool {
        false
    }
    fn has_css_animations(&self, _: &SharedStyleContext, _: Option<PseudoElement>) -> bool {
        false
    }
    fn has_css_transitions(&self, _: &SharedStyleContext, _: Option<PseudoElement>) -> bool {
        false
    }
    fn shadow_root(&self) -> Option<NoShadow<'a>> {
        None
    }
    fn containing_shadow(&self) -> Option<NoShadow<'a>> {
        None
    }
    fn lang_attr(&self) -> Option<AttrValue> {
        let mut e = Some(*self);
        let name: LocalName = "lang".into();
        while let Some(node) = e {
            if let Some(v) = node.node().attributes.get(&name) {
                return Some(v.as_str().into());
            }
            e = SelectorElement::parent_element(&node);
        }
        None
    }
    fn match_element_lang(&self, override_lang: Option<Option<AttrValue>>, value: &Lang) -> bool {
        let lang = override_lang.unwrap_or_else(|| self.lang_attr());
        lang.is_some_and(|lang| {
            let lang: &str = lang.as_ref();
            lang.eq_ignore_ascii_case(value)
                || lang
                    .get(..value.len())
                    .is_some_and(|prefix| prefix.eq_ignore_ascii_case(value))
                    && lang.as_bytes().get(value.len()) == Some(&b'-')
        })
    }
    fn is_html_document_body_element(&self) -> bool {
        false
    }
    fn synthesize_presentational_hints_for_legacy_attributes<
        V: selectors::sink::Push<ApplicableDeclarationBlock>,
    >(
        &self,
        _: VisitedHandlingMode,
        _: &mut V,
    ) {
    }
    fn local_name(&self) -> &<SelectorImpl as SelectorTypes>::BorrowedLocalName {
        &self.node().tag.0
    }
    fn namespace(&self) -> &<SelectorImpl as SelectorTypes>::BorrowedNamespaceUrl {
        &self.node().namespace.0
    }
    fn query_container_size(
        &self,
        _: &style::values::computed::Display,
    ) -> euclid::default::Size2D<Option<app_units::Au>> {
        euclid::default::Size2D::new(None, None)
    }
    fn has_selector_flags(&self, flags: ElementSelectorFlags) -> bool {
        self.node().flags.get().contains(flags)
    }
    fn relative_selector_search_direction(&self) -> ElementSelectorFlags {
        self.node().flags.get()
    }
    fn get_attr(&self, attr: &LocalName, namespace: &Namespace) -> Option<String> {
        if !namespace.0.is_empty() {
            return None;
        }
        self.node().attributes.get(attr).cloned()
    }
}
