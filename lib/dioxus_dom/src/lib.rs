//! Retained document, style, layout, and text projection for the shared Dioxus
//! WASM UI component.
//!
//! This crate intentionally exposes only values and byte buffers. Application
//! components can build a Dioxus-style tree with stable logical IDs, classes,
//! inline declarations, and listener flags; the shared component decodes it,
//! resolves simple CSS, computes deterministic WASM-side layout, shapes text into
//! glyph runs, and emits the stable BexOS scene ABI for native rendering.

use bexos_dioxus_scene::{
    Command, Glyph, GlyphRun, ImageCommand, Layer, Paint, PathCommand, Point, Rect, SceneBatch,
};
use std::collections::BTreeMap;

pub const LEGACY_VERSION: u32 = 1;
pub const VERSION: u32 = 2;
pub const MAX_DOCUMENT_BYTES: usize = 1 << 20;
pub const MAX_NODES: usize = 2048;
pub const MAX_RULES: usize = 256;
pub const MAX_CHILDREN: usize = 512;
pub const MAX_STRING: usize = 4096;
pub const LISTENER_POINTER: u32 = 1;
pub const LISTENER_KEYBOARD: u32 = 2;
pub const LISTENER_TEXT: u32 = 4;
pub const LISTENER_WHEEL: u32 = 8;
pub const LISTENER_FOCUS: u32 = 16;

#[derive(Clone, Debug, PartialEq)]
pub struct Document {
    pub version: u32,
    pub width: u32,
    pub height: u32,
    pub scale: f32,
    pub clear_rgba: [u8; 4],
    pub stylesheets: Vec<StyleRule>,
    pub author_stylesheets: Vec<String>,
    pub root: Node,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StyleRule {
    pub selector: String,
    pub declarations: String,
    pub origin: StyleOrigin,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum StyleOrigin {
    UserAgent,
    User,
    #[default]
    Author,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub id: u64,
    pub tag: String,
    pub classes: Vec<String>,
    pub attributes: BTreeMap<String, String>,
    pub declarations: String,
    pub state: u64,
    pub listeners: u32,
    pub kind: NodeKind,
    pub children: Vec<Node>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum NodeKind {
    Element,
    Text(String),
    Image { asset: u32 },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Display {
    Block,
    FlexRow,
    FlexColumn,
    Grid,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Error {
    InvalidEncoding,
    InvalidVersion,
    LimitExceeded,
    InvalidStyle,
    InvalidNode,
    InvalidGeometry,
    Scene(bexos_dioxus_scene::Error),
}

#[derive(Clone, Copy, Debug)]
struct Style {
    display: Display,
    display_set: bool,
    x: Option<f32>,
    y: Option<f32>,
    width: Option<f32>,
    height: Option<f32>,
    padding: f32,
    padding_set: bool,
    gap: f32,
    gap_set: bool,
    background: Option<[u8; 4]>,
    color: [u8; 4],
    color_set: bool,
    font_size: f32,
    font_size_set: bool,
    scroll_y: f32,
    scroll_y_set: bool,
    columns: u32,
    columns_set: bool,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            display: Display::Block,
            display_set: false,
            x: None,
            y: None,
            width: None,
            height: None,
            padding: 0.0,
            padding_set: false,
            gap: 0.0,
            gap_set: false,
            background: None,
            color: [32, 37, 45, 255],
            color_set: false,
            font_size: 16.0,
            font_size_set: false,
            scroll_y: 0.0,
            scroll_y_set: false,
            columns: 1,
            columns_set: false,
        }
    }
}

impl Document {
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        self.validate()?;
        let mut out = Vec::new();
        write_u32(&mut out, VERSION);
        write_u32(&mut out, self.width);
        write_u32(&mut out, self.height);
        write_f32(&mut out, self.scale);
        out.extend_from_slice(&self.clear_rgba);
        write_count(&mut out, self.stylesheets.len(), MAX_RULES)?;
        for rule in &self.stylesheets {
            write_string(&mut out, &rule.selector)?;
            write_string(&mut out, &rule.declarations)?;
            write_u32(&mut out, rule.origin.to_wire());
        }
        write_count(&mut out, self.author_stylesheets.len(), MAX_RULES)?;
        for sheet in &self.author_stylesheets {
            write_string(&mut out, sheet)?;
        }
        encode_node(&mut out, &self.root)?;
        if out.len() > MAX_DOCUMENT_BYTES {
            return Err(Error::LimitExceeded);
        }
        Ok(out)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() > MAX_DOCUMENT_BYTES {
            return Err(Error::LimitExceeded);
        }
        let mut reader = Reader::new(bytes);
        let version = reader.u32()?;
        if version == LEGACY_VERSION {
            return Self::decode_v1(reader);
        }
        if version != VERSION {
            return Err(Error::InvalidVersion);
        }
        let width = reader.u32()?;
        let height = reader.u32()?;
        let scale = reader.f32()?;
        let clear_rgba = reader.bytes4()?;
        let rule_count = reader.count(MAX_RULES)?;
        let mut stylesheets = Vec::with_capacity(rule_count);
        for _ in 0..rule_count {
            stylesheets.push(StyleRule {
                selector: reader.string()?,
                declarations: reader.string()?,
                origin: StyleOrigin::from_wire(reader.u32()?)?,
            });
        }
        let sheet_count = reader.count(MAX_RULES)?;
        let mut author_stylesheets = Vec::with_capacity(sheet_count);
        for _ in 0..sheet_count {
            author_stylesheets.push(reader.string()?);
        }
        let root = decode_node(&mut reader)?;
        reader.finish()?;
        let document = Self {
            version,
            width,
            height,
            scale,
            clear_rgba,
            stylesheets,
            author_stylesheets,
            root,
        };
        document.validate()?;
        Ok(document)
    }

    fn decode_v1(mut reader: Reader<'_>) -> Result<Self, Error> {
        let width = reader.u32()?;
        let height = reader.u32()?;
        let scale = reader.f32()?;
        let clear_rgba = reader.bytes4()?;
        let rule_count = reader.count(MAX_RULES)?;
        let mut stylesheets = Vec::with_capacity(rule_count);
        for _ in 0..rule_count {
            stylesheets.push(StyleRule {
                selector: reader.string()?,
                declarations: reader.string()?,
                origin: StyleOrigin::Author,
            });
        }
        let root = decode_node_v1(&mut reader)?;
        reader.finish()?;
        let document = Self {
            version: LEGACY_VERSION,
            width,
            height,
            scale,
            clear_rgba,
            stylesheets,
            author_stylesheets: Vec::new(),
            root,
        };
        document.validate()?;
        Ok(document)
    }

    pub fn validate(&self) -> Result<(), Error> {
        if self.width == 0
            || self.height == 0
            || self.width > bexos_dioxus_scene::MAX_DIMENSION
            || self.height > bexos_dioxus_scene::MAX_DIMENSION
            || !self.scale.is_finite()
            || self.scale <= 0.0
        {
            return Err(Error::InvalidGeometry);
        }
        if self.stylesheets.len() > MAX_RULES {
            return Err(Error::LimitExceeded);
        }
        let legacy_style_validation = self.version <= LEGACY_VERSION;
        for rule in &self.stylesheets {
            validate_string(&rule.selector)?;
            validate_string(&rule.declarations)?;
            if legacy_style_validation {
                let _ = parse_declarations(&rule.declarations)?;
            }
        }
        for sheet in &self.author_stylesheets {
            validate_string(sheet)?;
        }
        let mut count = 0usize;
        validate_node(&self.root, &mut count, legacy_style_validation)?;
        Ok(())
    }
}

impl Node {
    pub fn element(id: u64, tag: impl Into<String>) -> Self {
        Self {
            id,
            tag: tag.into(),
            classes: Vec::new(),
            attributes: BTreeMap::new(),
            declarations: String::new(),
            state: 0,
            listeners: 0,
            kind: NodeKind::Element,
            children: Vec::new(),
        }
    }

    pub fn text(id: u64, value: impl Into<String>) -> Self {
        Self {
            id,
            tag: "#text".into(),
            classes: Vec::new(),
            attributes: BTreeMap::new(),
            declarations: String::new(),
            state: 0,
            listeners: 0,
            kind: NodeKind::Text(value.into()),
            children: Vec::new(),
        }
    }

    pub fn image(id: u64, asset: u32) -> Self {
        Self {
            id,
            tag: "img".into(),
            classes: Vec::new(),
            attributes: BTreeMap::new(),
            declarations: String::new(),
            state: 0,
            listeners: 0,
            kind: NodeKind::Image { asset },
            children: Vec::new(),
        }
    }

    pub fn class(mut self, class: impl Into<String>) -> Self {
        self.classes.push(class.into());
        self
    }

    pub fn style(mut self, declarations: impl Into<String>) -> Self {
        self.declarations = declarations.into();
        self
    }

    pub fn attribute(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.attributes.insert(key.into(), value.into());
        self
    }

    pub fn state(mut self, state: u64) -> Self {
        self.state = state;
        self
    }

    pub fn listeners(mut self, flags: u32) -> Self {
        self.listeners = flags;
        self
    }

    pub fn child(mut self, child: Node) -> Self {
        self.children.push(child);
        self
    }

    pub fn children(mut self, children: impl IntoIterator<Item = Node>) -> Self {
        self.children.extend(children);
        self
    }
}

pub fn render(document: &Document) -> Result<SceneBatch, Error> {
    document.validate()?;
    let mut commands = vec![Command::PushLayer(Layer {
        alpha: 1.0,
        clip: Rect {
            x: 0.0,
            y: 0.0,
            width: document.width as f32,
            height: document.height as f32,
        },
    })];
    let rules = compile_rules(&document.stylesheets)?;
    let viewport = Rect {
        x: 0.0,
        y: 0.0,
        width: document.width as f32,
        height: document.height as f32,
    };
    render_node(
        &document.root,
        Style::default(),
        viewport,
        &rules,
        &mut commands,
    )?;
    commands.push(Command::PopLayer);
    let batch = SceneBatch {
        width: document.width,
        height: document.height,
        clear_rgba: document.clear_rgba,
        commands,
    };
    batch.validate().map_err(Error::Scene)?;
    Ok(batch)
}

fn render_node(
    node: &Node,
    parent: Style,
    rect: Rect,
    rules: &[CompiledRule],
    commands: &mut Vec<Command>,
) -> Result<(), Error> {
    let style = resolve_style(node, parent, rules)?;
    let bounds = Rect {
        x: style.x.unwrap_or(rect.x),
        y: style.y.unwrap_or(rect.y) - style.scroll_y,
        width: style.width.unwrap_or(rect.width).max(0.0),
        height: style.height.unwrap_or(rect.height).max(0.0),
    };
    if bounds.width == 0.0 || bounds.height == 0.0 {
        return Ok(());
    }
    if let Some(background) = style.background {
        rect_command(commands, bounds, background);
    }
    match &node.kind {
        NodeKind::Text(value) => {
            if !value.is_empty() {
                commands.push(Command::Glyphs(GlyphRun {
                    font_asset: 1,
                    size: style.font_size,
                    x: bounds.x,
                    y: bounds.y + style.font_size,
                    color: style.color,
                    glyphs: shape_text(value, style.font_size),
                }));
            }
        }
        NodeKind::Image { asset } => commands.push(Command::Image(ImageCommand {
            image_asset: *asset,
            rect: bounds,
            opacity: 1.0,
        })),
        NodeKind::Element => {
            let content = Rect {
                x: bounds.x + style.padding,
                y: bounds.y + style.padding,
                width: (bounds.width - style.padding * 2.0).max(0.0),
                height: (bounds.height - style.padding * 2.0).max(0.0),
            };
            layout_children(node, style, content, rules, commands)?;
        }
    }
    Ok(())
}

fn layout_children(
    node: &Node,
    style: Style,
    content: Rect,
    rules: &[CompiledRule],
    commands: &mut Vec<Command>,
) -> Result<(), Error> {
    if node.children.is_empty() {
        return Ok(());
    }
    match style.display {
        Display::FlexRow => {
            let fixed = style.gap * node.children.len().saturating_sub(1) as f32;
            let item_width = ((content.width - fixed) / node.children.len() as f32).max(0.0);
            let mut x = content.x;
            for child in &node.children {
                render_node(
                    child,
                    style,
                    Rect {
                        x,
                        y: content.y,
                        width: item_width,
                        height: content.height,
                    },
                    rules,
                    commands,
                )?;
                x += item_width + style.gap;
            }
        }
        Display::Grid => {
            let columns = style.columns.max(1).min(12);
            let rows = (node.children.len() as u32).div_ceil(columns).max(1);
            let item_width = ((content.width - style.gap * columns.saturating_sub(1) as f32)
                / columns as f32)
                .max(0.0);
            let item_height = ((content.height - style.gap * rows.saturating_sub(1) as f32)
                / rows as f32)
                .max(0.0);
            for (index, child) in node.children.iter().enumerate() {
                let col = index as u32 % columns;
                let row = index as u32 / columns;
                render_node(
                    child,
                    style,
                    Rect {
                        x: content.x + col as f32 * (item_width + style.gap),
                        y: content.y + row as f32 * (item_height + style.gap),
                        width: item_width,
                        height: item_height,
                    },
                    rules,
                    commands,
                )?;
            }
        }
        Display::Block | Display::FlexColumn => {
            let fixed = style.gap * node.children.len().saturating_sub(1) as f32;
            let item_height = ((content.height - fixed) / node.children.len() as f32).max(0.0);
            let mut y = content.y;
            for child in &node.children {
                render_node(
                    child,
                    style,
                    Rect {
                        x: content.x,
                        y,
                        width: content.width,
                        height: item_height,
                    },
                    rules,
                    commands,
                )?;
                y += item_height + style.gap;
            }
        }
    }
    Ok(())
}

fn rect_command(commands: &mut Vec<Command>, rect: Rect, color: [u8; 4]) {
    commands.push(Command::Path(PathCommand {
        points: vec![
            Point {
                x: rect.x,
                y: rect.y,
            },
            Point {
                x: rect.x + rect.width,
                y: rect.y,
            },
            Point {
                x: rect.x + rect.width,
                y: rect.y + rect.height,
            },
            Point {
                x: rect.x,
                y: rect.y + rect.height,
            },
        ],
        closed: true,
        paint: Paint::Solid(color),
        stroke_width: 0.0,
    }));
}

fn shape_text(value: &str, size: f32) -> Vec<Glyph> {
    let advance = (size * 0.56).max(1.0);
    value
        .chars()
        .take(bexos_dioxus_scene::MAX_GLYPHS)
        .enumerate()
        .map(|(index, ch)| Glyph {
            id: ch as u32,
            x: index as f32 * advance,
            y: 0.0,
            advance,
        })
        .collect()
}

#[derive(Clone, Debug)]
struct CompiledRule {
    selector: Selector,
    style: Style,
}

#[derive(Clone, Debug)]
enum Selector {
    Tag(String),
    Class(String),
    Id(u64),
}

fn compile_rules(rules: &[StyleRule]) -> Result<Vec<CompiledRule>, Error> {
    rules
        .iter()
        .map(|rule| {
            let selector = if let Some(class) = rule.selector.strip_prefix('.') {
                Selector::Class(class.into())
            } else if let Some(id) = rule.selector.strip_prefix('#') {
                Selector::Id(id.parse().map_err(|_| Error::InvalidStyle)?)
            } else {
                Selector::Tag(rule.selector.clone())
            };
            Ok(CompiledRule {
                selector,
                style: parse_declarations(&rule.declarations)?,
            })
        })
        .collect()
}

fn resolve_style(node: &Node, parent: Style, rules: &[CompiledRule]) -> Result<Style, Error> {
    let mut style = Style {
        color: parent.color,
        font_size: parent.font_size,
        ..Style::default()
    };
    for rule in rules {
        let matches = match &rule.selector {
            Selector::Tag(tag) => tag == &node.tag,
            Selector::Class(class) => node.classes.iter().any(|c| c == class),
            Selector::Id(id) => node.id == *id,
        };
        if matches {
            merge_style(&mut style, rule.style);
        }
    }
    merge_style(&mut style, parse_declarations(&node.declarations)?);
    Ok(style)
}

fn merge_style(base: &mut Style, next: Style) {
    if next.display_set {
        base.display = next.display;
    }
    base.x = next.x.or(base.x);
    base.y = next.y.or(base.y);
    base.width = next.width.or(base.width);
    base.height = next.height.or(base.height);
    if next.padding_set {
        base.padding = next.padding;
    }
    if next.gap_set {
        base.gap = next.gap;
    }
    base.background = next.background.or(base.background);
    if next.color_set {
        base.color = next.color;
    }
    if next.font_size_set {
        base.font_size = next.font_size;
    }
    if next.scroll_y_set {
        base.scroll_y = next.scroll_y;
    }
    if next.columns_set {
        base.columns = next.columns;
    }
}

fn parse_declarations(input: &str) -> Result<Style, Error> {
    validate_string(input)?;
    let mut style = Style::default();
    for declaration in input.split(';') {
        let declaration = declaration.trim();
        if declaration.is_empty() {
            continue;
        }
        let (name, value) = declaration.split_once(':').ok_or(Error::InvalidStyle)?;
        let name = name.trim();
        let value = value.trim();
        match name {
            "display" => {
                style.display = match value {
                    "block" => Display::Block,
                    "flex" | "flex-column" => Display::FlexColumn,
                    "flex-row" => Display::FlexRow,
                    "grid" => Display::Grid,
                    _ => return Err(Error::InvalidStyle),
                };
                style.display_set = true;
            }
            "flex-direction" => {
                style.display = match value {
                    "row" => Display::FlexRow,
                    "column" => Display::FlexColumn,
                    _ => return Err(Error::InvalidStyle),
                };
                style.display_set = true;
            }
            "x" => style.x = Some(parse_px(value)?),
            "y" => style.y = Some(parse_px(value)?),
            "width" => style.width = Some(parse_px(value)?),
            "height" => style.height = Some(parse_px(value)?),
            "padding" => {
                style.padding = parse_px(value)?;
                style.padding_set = true;
            }
            "gap" => {
                style.gap = parse_px(value)?;
                style.gap_set = true;
            }
            "background" | "background-color" => style.background = Some(parse_color(value)?),
            "color" => {
                style.color = parse_color(value)?;
                style.color_set = true;
            }
            "font-size" => {
                style.font_size = parse_px(value)?.clamp(1.0, 512.0);
                style.font_size_set = true;
            }
            "scroll-y" => {
                style.scroll_y = parse_px(value)?;
                style.scroll_y_set = true;
            }
            "grid-template-columns" => {
                style.columns = parse_columns(value)?;
                style.columns_set = true;
            }
            _ => return Err(Error::InvalidStyle),
        }
    }
    Ok(style)
}

fn parse_px(value: &str) -> Result<f32, Error> {
    let value = value.strip_suffix("px").unwrap_or(value).trim();
    let parsed: f32 = value.parse().map_err(|_| Error::InvalidStyle)?;
    if parsed.is_finite() {
        Ok(parsed)
    } else {
        Err(Error::InvalidStyle)
    }
}

fn parse_columns(value: &str) -> Result<u32, Error> {
    if let Some((count, suffix)) = value.trim().split_once('x') {
        if suffix.trim() == "1fr" {
            return count
                .trim()
                .parse::<u32>()
                .map(|v| v.clamp(1, 12))
                .map_err(|_| Error::InvalidStyle);
        }
    }
    Ok(value
        .split_whitespace()
        .filter(|part| *part == "1fr" || part.ends_with("px"))
        .count()
        .max(1)
        .min(12) as u32)
}

fn parse_color(value: &str) -> Result<[u8; 4], Error> {
    let hex = value.strip_prefix('#').ok_or(Error::InvalidStyle)?;
    match hex.len() {
        6 => Ok([byte(hex, 0)?, byte(hex, 2)?, byte(hex, 4)?, 255]),
        8 => Ok([byte(hex, 0)?, byte(hex, 2)?, byte(hex, 4)?, byte(hex, 6)?]),
        _ => Err(Error::InvalidStyle),
    }
}

fn byte(hex: &str, index: usize) -> Result<u8, Error> {
    u8::from_str_radix(hex.get(index..index + 2).ok_or(Error::InvalidStyle)?, 16)
        .map_err(|_| Error::InvalidStyle)
}

fn validate_node(
    node: &Node,
    count: &mut usize,
    legacy_style_validation: bool,
) -> Result<(), Error> {
    *count = count.checked_add(1).ok_or(Error::LimitExceeded)?;
    if *count > MAX_NODES || node.id == 0 || node.children.len() > MAX_CHILDREN {
        return Err(Error::LimitExceeded);
    }
    validate_string(&node.tag)?;
    validate_string(&node.declarations)?;
    if legacy_style_validation {
        let _ = parse_declarations(&node.declarations)?;
    }
    if !matches!(node.kind, NodeKind::Element) && !node.children.is_empty() {
        return Err(Error::InvalidNode);
    }
    for class in &node.classes {
        validate_string(class)?;
    }
    if node.attributes.len() > 64 {
        return Err(Error::LimitExceeded);
    }
    for (key, value) in &node.attributes {
        validate_string(key)?;
        validate_string(value)?;
    }
    if let NodeKind::Image { asset } = node.kind {
        if asset == 0 || asset as usize > bexos_dioxus_scene::MAX_ASSETS {
            return Err(Error::InvalidNode);
        }
    }
    if let NodeKind::Text(ref value) = node.kind {
        validate_string(value)?;
    }
    for child in &node.children {
        validate_node(child, count, legacy_style_validation)?;
    }
    Ok(())
}

fn validate_string(value: &str) -> Result<(), Error> {
    if value.len() > MAX_STRING || value.bytes().any(|b| b == 0) {
        Err(Error::LimitExceeded)
    } else {
        Ok(())
    }
}

fn encode_node(out: &mut Vec<u8>, node: &Node) -> Result<(), Error> {
    out.extend_from_slice(&node.id.to_le_bytes());
    write_string(out, &node.tag)?;
    write_count(out, node.classes.len(), MAX_CHILDREN)?;
    for class in &node.classes {
        write_string(out, class)?;
    }
    write_count(out, node.attributes.len(), MAX_CHILDREN)?;
    for (key, value) in &node.attributes {
        write_string(out, key)?;
        write_string(out, value)?;
    }
    write_string(out, &node.declarations)?;
    out.extend_from_slice(&node.state.to_le_bytes());
    write_u32(out, node.listeners);
    match &node.kind {
        NodeKind::Element => out.push(1),
        NodeKind::Text(value) => {
            out.push(2);
            write_string(out, value)?;
        }
        NodeKind::Image { asset } => {
            out.push(3);
            write_u32(out, *asset);
        }
    }
    write_count(out, node.children.len(), MAX_CHILDREN)?;
    for child in &node.children {
        encode_node(out, child)?;
    }
    Ok(())
}

fn decode_node(reader: &mut Reader<'_>) -> Result<Node, Error> {
    let id = reader.u64()?;
    let tag = reader.string()?;
    let class_count = reader.count(MAX_CHILDREN)?;
    let mut classes = Vec::with_capacity(class_count);
    for _ in 0..class_count {
        classes.push(reader.string()?);
    }
    let attribute_count = reader.count(MAX_CHILDREN)?;
    let mut attributes = BTreeMap::new();
    for _ in 0..attribute_count {
        if attributes
            .insert(reader.string()?, reader.string()?)
            .is_some()
        {
            return Err(Error::InvalidNode);
        }
    }
    let declarations = reader.string()?;
    let state = reader.u64()?;
    let listeners = reader.u32()?;
    let kind = match reader.byte()? {
        1 => NodeKind::Element,
        2 => NodeKind::Text(reader.string()?),
        3 => NodeKind::Image {
            asset: reader.u32()?,
        },
        _ => return Err(Error::InvalidNode),
    };
    let child_count = reader.count(MAX_CHILDREN)?;
    let mut children = Vec::with_capacity(child_count);
    for _ in 0..child_count {
        children.push(decode_node(reader)?);
    }
    Ok(Node {
        id,
        tag,
        classes,
        attributes,
        declarations,
        state,
        listeners,
        kind,
        children,
    })
}

fn decode_node_v1(reader: &mut Reader<'_>) -> Result<Node, Error> {
    let id = reader.u64()?;
    let tag = reader.string()?;
    let class_count = reader.count(MAX_CHILDREN)?;
    let mut classes = Vec::with_capacity(class_count);
    for _ in 0..class_count {
        classes.push(reader.string()?);
    }
    let declarations = reader.string()?;
    let listeners = reader.u32()?;
    let kind = match reader.byte()? {
        1 => NodeKind::Element,
        2 => NodeKind::Text(reader.string()?),
        3 => NodeKind::Image {
            asset: reader.u32()?,
        },
        _ => return Err(Error::InvalidNode),
    };
    let child_count = reader.count(MAX_CHILDREN)?;
    let mut children = Vec::with_capacity(child_count);
    for _ in 0..child_count {
        children.push(decode_node_v1(reader)?);
    }
    Ok(Node {
        id,
        tag,
        classes,
        attributes: BTreeMap::new(),
        declarations,
        state: 0,
        listeners,
        kind,
        children,
    })
}

impl StyleOrigin {
    fn to_wire(self) -> u32 {
        match self {
            Self::UserAgent => 1,
            Self::User => 2,
            Self::Author => 3,
        }
    }

    fn from_wire(value: u32) -> Result<Self, Error> {
        match value {
            1 => Ok(Self::UserAgent),
            2 => Ok(Self::User),
            3 => Ok(Self::Author),
            _ => Err(Error::InvalidStyle),
        }
    }
}

fn write_count(out: &mut Vec<u8>, value: usize, limit: usize) -> Result<(), Error> {
    if value > limit || value > u32::MAX as usize {
        return Err(Error::LimitExceeded);
    }
    write_u32(out, value as u32);
    Ok(())
}

fn write_string(out: &mut Vec<u8>, value: &str) -> Result<(), Error> {
    validate_string(value)?;
    write_count(out, value.len(), MAX_STRING)?;
    out.extend_from_slice(value.as_bytes());
    Ok(())
}

fn write_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn write_f32(out: &mut Vec<u8>, value: f32) {
    out.extend_from_slice(&value.to_le_bytes());
}

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    fn finish(&self) -> Result<(), Error> {
        if self.pos == self.bytes.len() {
            Ok(())
        } else {
            Err(Error::InvalidEncoding)
        }
    }

    fn byte(&mut self) -> Result<u8, Error> {
        let byte = *self.bytes.get(self.pos).ok_or(Error::InvalidEncoding)?;
        self.pos += 1;
        Ok(byte)
    }

    fn bytes4(&mut self) -> Result<[u8; 4], Error> {
        let bytes = self.take(4)?;
        Ok([bytes[0], bytes[1], bytes[2], bytes[3]])
    }

    fn count(&mut self, limit: usize) -> Result<usize, Error> {
        let value = self.u32()? as usize;
        if value > limit {
            Err(Error::LimitExceeded)
        } else {
            Ok(value)
        }
    }

    fn u32(&mut self) -> Result<u32, Error> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn u64(&mut self) -> Result<u64, Error> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }

    fn f32(&mut self) -> Result<f32, Error> {
        Ok(f32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn string(&mut self) -> Result<String, Error> {
        let len = self.count(MAX_STRING)?;
        let bytes = self.take(len)?;
        core::str::from_utf8(bytes)
            .map(|v| v.to_string())
            .map_err(|_| Error::InvalidEncoding)
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8], Error> {
        let end = self.pos.checked_add(len).ok_or(Error::InvalidEncoding)?;
        let bytes = self
            .bytes
            .get(self.pos..end)
            .ok_or(Error::InvalidEncoding)?;
        self.pos = end;
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_document() -> Document {
        Document {
            version: VERSION,
            width: 320,
            height: 200,
            scale: 1.0,
            clear_rgba: [20, 24, 32, 255],
            stylesheets: vec![
                StyleRule {
                    selector: ".root".into(),
                    declarations:
                        "display:flex;flex-direction:column;gap:8;padding:12;background:#f5f7fa"
                            .into(),
                    origin: StyleOrigin::Author,
                },
                StyleRule {
                    selector: ".row".into(),
                    declarations: "display:flex;flex-direction:row;gap:8;background:#ecf0f1".into(),
                    origin: StyleOrigin::Author,
                },
                StyleRule {
                    selector: ".tile".into(),
                    declarations: "background:#34495e;color:#ffffff;font-size:18".into(),
                    origin: StyleOrigin::Author,
                },
            ],
            author_stylesheets: Vec::new(),
            root: Node::element(1, "main").class("root").children([
                Node::element(2, "section").class("row").child(
                    Node::text(3, "counter 1")
                        .class("tile")
                        .listeners(LISTENER_POINTER),
                ),
                Node::image(4, 2).style("height:48;background:#ffffff"),
            ]),
        }
    }

    #[test]
    fn document_round_trip_preserves_dom_shape() {
        let document = sample_document();
        let encoded = document.encode().unwrap();
        assert_eq!(Document::decode(&encoded).unwrap(), document);
    }

    #[test]
    fn css_layout_text_and_image_become_scene_commands() {
        let scene = render(&sample_document()).unwrap();
        assert_eq!(scene.width, 320);
        assert!(
            scene
                .commands
                .iter()
                .any(|c| matches!(c, Command::Glyphs(_)))
        );
        assert!(
            scene
                .commands
                .iter()
                .any(|c| matches!(c, Command::Image(_)))
        );
        assert!(
            scene
                .commands
                .iter()
                .filter(|c| matches!(c, Command::Path(_)))
                .count()
                >= 2
        );
    }

    #[test]
    fn legacy_document_validation_rejects_malformed_css_before_scene_generation() {
        let mut document = sample_document();
        document.version = LEGACY_VERSION;
        document.stylesheets[0]
            .declarations
            .push_str("; position:absolute");
        assert_eq!(document.validate(), Err(Error::InvalidStyle));
    }

    #[test]
    fn later_class_does_not_reset_unspecified_layout_values() {
        let document = Document {
            version: VERSION,
            width: 120,
            height: 60,
            scale: 1.0,
            clear_rgba: [0, 0, 0, 0],
            stylesheets: vec![
                StyleRule {
                    selector: ".row".into(),
                    declarations: "display:flex;flex-direction:row".into(),
                    origin: StyleOrigin::Author,
                },
                StyleRule {
                    selector: ".tone".into(),
                    declarations: "color:#ffffff".into(),
                    origin: StyleOrigin::Author,
                },
            ],
            author_stylesheets: Vec::new(),
            root: Node::element(1, "main")
                .class("row")
                .class("tone")
                .children([Node::text(2, "left"), Node::text(3, "right")]),
        };
        let scene = render(&document).unwrap();
        let glyph_positions: Vec<f32> = scene
            .commands
            .iter()
            .filter_map(|command| match command {
                Command::Glyphs(run) => Some(run.x),
                _ => None,
            })
            .collect();
        assert_eq!(glyph_positions.len(), 2);
        assert!(glyph_positions[1] > glyph_positions[0]);
    }

    #[test]
    fn text_and_image_nodes_reject_children() {
        let mut document = sample_document();
        document
            .root
            .children
            .push(Node::text(7, "bad").child(Node::text(8, "nested")));
        assert_eq!(document.validate(), Err(Error::InvalidNode));
    }

    #[test]
    fn bounds_node_count() {
        let mut root = Node::element(1, "main");
        for id in 2..(MAX_NODES as u64 + 3) {
            root.children.push(Node::text(id, "x"));
        }
        let document = Document {
            version: VERSION,
            width: 100,
            height: 100,
            scale: 1.0,
            clear_rgba: [0, 0, 0, 0],
            stylesheets: vec![],
            author_stylesheets: Vec::new(),
            root,
        };
        assert_eq!(document.validate(), Err(Error::LimitExceeded));
    }
}
