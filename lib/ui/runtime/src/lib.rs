//! Native retained document rendering for shell Dioxus documents.

use bexos_dioxus_dom::{Document, Node, NodeKind, StyleOrigin};
use bexos_dioxus_scene::{
    Command, Glyph, GlyphRun, Layer, Paint, PathCommand, Point, Rect, SceneBatch,
};
use bexos_flatland_style::{
    Resolver, StylesheetSet, Theme,
    dom::Node as StyleNode,
    metrics::DefaultFontMetrics,
    style::properties::{
        ComputedValues, longhands::flex_direction::computed_value::T as FlexDirection,
    },
    style::values::{computed::Size, specified::box_::DisplayInside},
};
use bexos_ui_theme::ThemePreferences;
use std::collections::BTreeMap;
use std::sync::Arc;

pub const LATIN_FONT_ASSET: u32 = 2048;
pub const ARABIC_FONT_ASSET: u32 = 2047;
pub const DEVANAGARI_FONT_ASSET: u32 = 2046;

struct ShapingFont {
    engine: bexos_flatland_text::TextEngine,
    asset: u32,
}

pub struct FontSet {
    latin: ShapingFont,
    arabic: ShapingFont,
    devanagari: ShapingFont,
    families: BTreeMap<String, ShapingFont>,
    metrics: bexos_flatland_text::metrics::Metrics,
}

impl FontSet {
    pub fn from_shared<T>(latin: Arc<T>, arabic: Arc<T>, devanagari: Arc<T>) -> Result<Self, Error>
    where
        T: AsRef<[u8]> + Send + Sync + 'static,
    {
        let metrics = bexos_flatland_text::metrics::Metrics::from_font(latin.as_ref().as_ref())
            .map_err(|_| Error::UnsupportedLayout)?;
        Ok(Self {
            latin: shared_font(latin, LATIN_FONT_ASSET)?,
            arabic: shared_font(arabic, ARABIC_FONT_ASSET)?,
            devanagari: shared_font(devanagari, DEVANAGARI_FONT_ASSET)?,
            families: BTreeMap::new(),
            metrics,
        })
    }

    #[cfg(test)]
    fn pinned() -> Result<Self, Error> {
        let latin = include_bytes!(env!("INTER")).to_vec();
        let arabic = include_bytes!(env!("NOTO_ARABIC")).to_vec();
        let devanagari = include_bytes!(env!("NOTO_DEVANAGARI")).to_vec();
        let metrics = bexos_flatland_text::metrics::Metrics::from_font(&latin)
            .map_err(|_| Error::UnsupportedLayout)?;
        Ok(Self {
            latin: owned_font(latin, LATIN_FONT_ASSET)?,
            arabic: owned_font(arabic, ARABIC_FONT_ASSET)?,
            devanagari: owned_font(devanagari, DEVANAGARI_FONT_ASSET)?,
            families: BTreeMap::new(),
            metrics,
        })
    }

    pub fn add_shared_family<T>(
        &mut self,
        family: &str,
        font: Arc<T>,
        asset: u32,
    ) -> Result<(), Error>
    where
        T: AsRef<[u8]> + Send + Sync + 'static,
    {
        let family = normalize_family(family).ok_or(Error::UnsupportedLayout)?;
        self.families.insert(family, shared_font(font, asset)?);
        Ok(())
    }

    pub fn has_family(&self, family: &str) -> bool {
        normalize_family(family).is_some_and(|family| self.families.contains_key(&family))
    }

    fn select(&mut self, text: &str, family_stack: Option<&str>) -> &mut ShapingFont {
        if let Some(stack) = family_stack {
            for family in stack.split(',').filter_map(normalize_family) {
                let family = if family == "monospace" {
                    "jetbrains mono".into()
                } else {
                    family
                };
                if self.families.contains_key(&family) {
                    return self.families.get_mut(&family).unwrap();
                }
            }
        }
        if text
            .chars()
            .any(|value| ('\u{0600}'..='\u{06ff}').contains(&value))
        {
            &mut self.arabic
        } else if text
            .chars()
            .any(|value| ('\u{0900}'..='\u{097f}').contains(&value))
        {
            &mut self.devanagari
        } else {
            &mut self.latin
        }
    }
}

fn normalize_family(value: &str) -> Option<String> {
    let value = value
        .trim()
        .trim_matches(['\'', '"'])
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    (!value.is_empty() && value.len() <= 64).then_some(value)
}

fn shared_font<T>(font: Arc<T>, asset: u32) -> Result<ShapingFont, Error>
where
    T: AsRef<[u8]> + Send + Sync + 'static,
{
    let mut engine = bexos_flatland_text::TextEngine::default();
    engine
        .register_shared_font(font)
        .map_err(|_| Error::UnsupportedLayout)?;
    Ok(ShapingFont { engine, asset })
}

#[cfg(test)]
fn owned_font(font: Vec<u8>, asset: u32) -> Result<ShapingFont, Error> {
    let mut engine = bexos_flatland_text::TextEngine::default();
    engine
        .register_font(font)
        .map_err(|_| Error::UnsupportedLayout)?;
    Ok(ShapingFont { engine, asset })
}

#[derive(Clone, Debug, PartialEq)]
pub struct RuntimeTheme {
    pub generation: u64,
    pub preferences: ThemePreferences,
    pub user_css: String,
}

impl Default for RuntimeTheme {
    fn default() -> Self {
        let preferences = ThemePreferences::default();
        Self {
            generation: 0,
            user_css: preferences.user_css(),
            preferences,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RenderedDocument {
    pub batch: SceneBatch,
    pub theme_generation: u64,
    pub document_version: u32,
}

#[derive(Debug, PartialEq)]
pub enum Error {
    Document(bexos_dioxus_dom::Error),
    Style(bexos_flatland_style::Error),
    Layout(bexos_flatland::Error),
    Scene(bexos_dioxus_scene::Error),
    MissingRoot,
    UnsupportedLayout,
}

#[cfg(test)]
pub fn render_document(
    document: &Document,
    theme: &RuntimeTheme,
) -> Result<RenderedDocument, Error> {
    render_document_with_fonts(document, theme, &mut FontSet::pinned()?)
}

pub fn render_document_with_fonts(
    document: &Document,
    theme: &RuntimeTheme,
    fonts: &mut FontSet,
) -> Result<RenderedDocument, Error> {
    document.validate().map_err(Error::Document)?;
    let stylesheets = stylesheet_set(document, theme);
    let mut resolver = Resolver::new(
        Theme::from_stylesheets(&stylesheets).map_err(Error::Style)?,
        document.width as f32,
        document.height as f32,
        document.scale,
        !matches!(
            theme.preferences.color_scheme,
            bexos_ui_theme::ColorScheme::Light
        ),
        Box::new(DefaultFontMetrics(fonts.metrics)),
    )
    .map_err(Error::Style)?;
    let style_nodes = flatten(document)?;
    resolver.resolve(&style_nodes).map_err(Error::Style)?;
    let mut layout = bexos_flatland_layout::LayoutTree::default();
    let mut nodes = Vec::new();
    collect_nodes(&document.root, &mut nodes);
    for node in &nodes {
        let computed = resolver.get(node.id).ok_or(Error::UnsupportedLayout)?;
        layout
            .create(node.id, taffy_style(computed, node, document)?)
            .map_err(Error::Layout)?;
    }
    for node in &nodes {
        let ids = node
            .children
            .iter()
            .filter(|child| !matches!(child.kind, NodeKind::Text(_)))
            .map(|child| child.id)
            .collect::<Vec<_>>();
        layout.set_children(node.id, &ids).map_err(Error::Layout)?;
    }
    layout
        .compute(
            document.root.id,
            document.width as f32,
            document.height as f32,
        )
        .map_err(Error::Layout)?;
    let mut commands = vec![Command::PushLayer(Layer {
        alpha: 1.0,
        clip: Rect {
            x: 0.0,
            y: 0.0,
            width: document.width as f32,
            height: document.height as f32,
        },
    })];
    emit_node(
        &document.root,
        None,
        0.0,
        0.0,
        theme,
        &layout,
        &resolver,
        fonts,
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
    Ok(RenderedDocument {
        batch,
        theme_generation: theme.generation,
        document_version: document.version,
    })
}

pub fn stylesheet_set(document: &Document, theme: &RuntimeTheme) -> StylesheetSet {
    let mut set = StylesheetSet::new();
    set.set_user_agent(bexos_ui_core::base_css());
    set.set_user(&theme.user_css);
    for css in &document.author_stylesheets {
        set.push_author(css);
    }
    for rule in &document.stylesheets {
        match rule.origin {
            StyleOrigin::UserAgent => {
                set.push_user_agent(&format!("{} {{ {} }}", rule.selector, rule.declarations))
            }
            StyleOrigin::User => {
                set.push_user(&format!("{} {{ {} }}", rule.selector, rule.declarations))
            }
            StyleOrigin::Author => {
                set.push_author(&format!("{} {{ {} }}", rule.selector, rule.declarations))
            }
        }
    }
    set
}

fn flatten(document: &Document) -> Result<Vec<StyleNode>, Error> {
    let mut out = Vec::new();
    flatten_node(&document.root, None, &mut out)?;
    Ok(out)
}

fn flatten_node(node: &Node, parent: Option<u64>, out: &mut Vec<StyleNode>) -> Result<(), Error> {
    if matches!(node.kind, NodeKind::Text(_)) {
        return Ok(());
    }
    let classes = node.classes.join(" ");
    let mut attributes = node.attributes.clone();
    if !attributes.contains_key("id") {
        attributes.insert("data-node-id".into(), node.id.to_string());
    }
    out.push(StyleNode {
        id: node.id,
        parent,
        tag: node.tag.clone(),
        classes,
        style: node.declarations.clone(),
        attributes,
        state: node.state,
    });
    for child in &node.children {
        flatten_node(child, Some(node.id), out)?;
    }
    Ok(())
}

fn collect_nodes<'a>(node: &'a Node, out: &mut Vec<&'a Node>) {
    if !matches!(node.kind, NodeKind::Text(_)) {
        out.push(node);
        for child in &node.children {
            collect_nodes(child, out);
        }
    }
}

fn taffy_style(
    computed: &ComputedValues,
    node: &Node,
    document: &Document,
) -> Result<bexos_flatland_layout::Style, Error> {
    let mut style = bexos_flatland_layout::Style::default();
    let position = computed.get_position();
    style.display = match computed.get_box().clone_display().inside() {
        DisplayInside::None => bexos_flatland_layout::Display::None,
        DisplayInside::Flex => bexos_flatland_layout::Display::Flex,
        DisplayInside::Grid => bexos_flatland_layout::Display::Grid,
        _ => bexos_flatland_layout::Display::Block,
    };
    style.direction = if computed.get_inherited_box().clone_direction()
        == bexos_flatland_style::style::properties::longhands::direction::computed_value::T::Rtl
    {
        bexos_flatland_layout::Direction::Rtl
    } else {
        bexos_flatland_layout::Direction::Ltr
    };
    style.flex_direction = match position.clone_flex_direction() {
        FlexDirection::Row => bexos_flatland_layout::FlexDirection::Row,
        FlexDirection::Column => bexos_flatland_layout::FlexDirection::Column,
        FlexDirection::RowReverse => bexos_flatland_layout::FlexDirection::RowReverse,
        FlexDirection::ColumnReverse => bexos_flatland_layout::FlexDirection::ColumnReverse,
    };
    style.size.width = taffy_dimension(position.clone_width(), document.width as f32)?;
    style.size.height = taffy_dimension(position.clone_height(), document.height as f32)?;
    if node.id == document.root.id {
        style.size.width = bexos_flatland_layout::Dimension::length(document.width as f32);
        style.size.height = bexos_flatland_layout::Dimension::length(document.height as f32);
    }
    let padding = computed.get_padding();
    let pad = [
        length_percent(padding.clone_padding_top())?,
        length_percent(padding.clone_padding_right())?,
        length_percent(padding.clone_padding_bottom())?,
        length_percent(padding.clone_padding_left())?,
    ];
    style.padding = bexos_flatland_layout::Rect {
        top: pad[0],
        right: pad[1],
        bottom: pad[2],
        left: pad[3],
    };
    Ok(style)
}

fn taffy_dimension(value: Size, fallback: f32) -> Result<bexos_flatland_layout::Dimension, Error> {
    Ok(match value {
        Size::Auto => bexos_flatland_layout::Dimension::auto(),
        Size::LengthPercentage(v) => bexos_flatland_layout::Dimension::length(
            v.0.to_length().map(|v| v.px()).unwrap_or(fallback),
        ),
        _ => return Err(Error::UnsupportedLayout),
    })
}

fn length_percent(
    value: bexos_flatland_style::style::values::computed::NonNegativeLengthPercentage,
) -> Result<bexos_flatland_layout::LengthPercentage, Error> {
    Ok(bexos_flatland_layout::LengthPercentage::length(
        value
            .0
            .to_length()
            .map(|v| v.px())
            .ok_or(Error::UnsupportedLayout)?,
    ))
}

fn emit_node(
    node: &Node,
    language: Option<&str>,
    parent_x: f32,
    parent_y: f32,
    theme: &RuntimeTheme,
    layout: &bexos_flatland_layout::LayoutTree,
    resolver: &Resolver,
    fonts: &mut FontSet,
    commands: &mut Vec<Command>,
) -> Result<(), Error> {
    if matches!(node.kind, NodeKind::Text(_)) {
        return Ok(());
    }
    let language = node.attributes.get("lang").map(String::as_str).or(language);
    let bounds = layout.bounds(node.id).map_err(Error::Layout)?;
    let overrides = GeometryOverrides::parse(&node.declarations);
    let x = parent_x + overrides.x.unwrap_or(bounds.x);
    let y = parent_y + overrides.y.unwrap_or(bounds.y);
    let rect = Rect {
        x,
        y,
        width: overrides.width.unwrap_or(bounds.width),
        height: overrides.height.unwrap_or(bounds.height),
    };
    let computed = resolver.get(node.id).ok_or(Error::UnsupportedLayout)?;
    let opacity = computed.get_effects().clone_opacity();
    if opacity > 0.0 {
        if let Some(background) = declaration_color(&node.declarations, theme) {
            rect_command(commands, rect, background);
        }
    }
    let font_size = computed.get_font().clone_font_size().computed_size().px();
    let color = computed
        .get_inherited_text()
        .clone_color()
        .to_nscolor()
        .to_le_bytes();
    let family_stack = declaration_value(&node.declarations, "font-family");
    for child in &node.children {
        match &child.kind {
            NodeKind::Text(value) => {
                if !value.is_empty() {
                    shaped_text(
                        commands,
                        rect.x,
                        rect.y,
                        rect.width.max(1.0),
                        value,
                        font_size,
                        color,
                        family_stack,
                        language,
                        computed.get_inherited_box().clone_direction() == bexos_flatland_style::style::properties::longhands::direction::computed_value::T::Rtl,
                        fonts,
                    )?;
                }
            }
            NodeKind::Image { asset } => {
                commands.push(Command::Image(bexos_dioxus_scene::ImageCommand {
                    image_asset: *asset,
                    rect,
                    opacity,
                }));
            }
            NodeKind::Element => emit_node(
                child, language, x, y, theme, layout, resolver, fonts, commands,
            )?,
        }
    }
    Ok(())
}

fn declaration_color(input: &str, theme: &RuntimeTheme) -> Option<[u8; 4]> {
    for declaration in input.split(';') {
        let Some((name, value)) = declaration.split_once(':') else {
            continue;
        };
        let name = name.trim();
        if name != "background" && name != "background-color" {
            continue;
        }
        if let Some(color) = parse_color(value.trim(), theme) {
            return Some(color);
        }
    }
    None
}

fn parse_color(value: &str, theme: &RuntimeTheme) -> Option<[u8; 4]> {
    let value = value
        .split("!important")
        .next()
        .unwrap_or(value)
        .trim()
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_end_matches(',');
    if let Some(hex) = value.strip_prefix('#') {
        return parse_hex_color(hex);
    }
    if let Some(variable) = value.strip_prefix("var(").and_then(|v| v.strip_suffix(')')) {
        let name = variable.split(',').next()?.trim();
        return theme_color(name, theme);
    }
    None
}

fn parse_hex_color(hex: &str) -> Option<[u8; 4]> {
    let parse = |range: core::ops::Range<usize>| u8::from_str_radix(&hex[range], 16).ok();
    match hex.len() {
        6 => Some([parse(0..2)?, parse(2..4)?, parse(4..6)?, 255]),
        8 => Some([parse(0..2)?, parse(2..4)?, parse(4..6)?, parse(6..8)?]),
        _ => None,
    }
}

fn theme_color(name: &str, theme: &RuntimeTheme) -> Option<[u8; 4]> {
    let dark = !matches!(
        theme.preferences.color_scheme,
        bexos_ui_theme::ColorScheme::Light
    );
    match (name, theme.preferences.color_scheme) {
        ("--bex-bg", bexos_ui_theme::ColorScheme::Light) => Some([0xf5, 0xf7, 0xfa, 255]),
        ("--bex-panel", bexos_ui_theme::ColorScheme::Light) => Some([255, 255, 255, 255]),
        ("--bex-bg", bexos_ui_theme::ColorScheme::HighContrast) => Some([0, 0, 0, 255]),
        ("--bex-panel", bexos_ui_theme::ColorScheme::HighContrast) => Some([0x10, 0x10, 0x10, 255]),
        ("--bex-bg", _) if dark => Some([0x12, 0x1a, 0x2a, 255]),
        ("--bex-panel", _) if dark => Some([0x23, 0x30, 0x44, 255]),
        _ => None,
    }
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

#[derive(Default)]
struct GeometryOverrides {
    x: Option<f32>,
    y: Option<f32>,
    width: Option<f32>,
    height: Option<f32>,
}

impl GeometryOverrides {
    fn parse(input: &str) -> Self {
        let mut out = Self::default();
        for declaration in input.split(';') {
            let Some((name, value)) = declaration.split_once(':') else {
                continue;
            };
            let Some(value) = parse_px(value) else {
                continue;
            };
            match name.trim() {
                "x" | "left" => out.x = Some(value),
                "y" | "top" => out.y = Some(value),
                "width" => out.width = Some(value),
                "height" => out.height = Some(value),
                _ => {}
            }
        }
        out
    }
}

fn parse_px(value: &str) -> Option<f32> {
    let value = value.trim().strip_suffix("px").unwrap_or(value.trim());
    value.parse::<f32>().ok().filter(|v| v.is_finite())
}

fn shaped_text(
    commands: &mut Vec<Command>,
    x: f32,
    y: f32,
    width: f32,
    value: &str,
    size: f32,
    color: [u8; 4],
    family_stack: Option<&str>,
    language: Option<&str>,
    rtl: bool,
    fonts: &mut FontSet,
) -> Result<(), Error> {
    let font = fonts.select(value, family_stack);
    let layout = font
        .engine
        .shape(
            value,
            bexos_flatland_text::TextStyle {
                size,
                width,
                color,
                language: language.map(str::to_string),
                rtl,
                ..Default::default()
            },
        )
        .map_err(|_| Error::UnsupportedLayout)?;
    for line in layout.lines() {
        for item in line.items() {
            if let bexos_flatland_text::PositionedLayoutItem::GlyphRun(run) = item {
                commands.push(Command::Glyphs(GlyphRun {
                    font_asset: font.asset,
                    size: run.run().font_size(),
                    x: x + run.offset(),
                    y: y + run.baseline(),
                    color,
                    glyphs: run
                        .glyphs()
                        .map(|glyph| Glyph {
                            id: glyph.id,
                            x: glyph.x,
                            y: -glyph.y,
                            advance: glyph.advance,
                        })
                        .collect(),
                }));
            }
        }
    }
    Ok(())
}

fn declaration_value<'a>(input: &'a str, wanted: &str) -> Option<&'a str> {
    input.split(';').find_map(|declaration| {
        let (name, value) = declaration.split_once(':')?;
        (name.trim() == wanted).then_some(value.trim())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use bexos_dioxus_dom::StyleRule;

    #[test]
    fn native_runtime_uses_theme_author_and_inline_css() {
        let document = Document {
            version: bexos_dioxus_dom::VERSION,
            width: 160,
            height: 80,
            scale: 1.0,
            clear_rgba: [0, 0, 0, 0],
            stylesheets: vec![StyleRule {
                selector: ".label".into(),
                declarations: "font-size:20px; color:#123456;".into(),
                origin: StyleOrigin::Author,
            }],
            author_stylesheets: vec!["main { display:flex; flex-direction:column; }".into()],
            root: Node::element(1, "main")
                .style("width:160px; height:80px; padding:8px; background: var(--bex-bg);")
                .child(
                    Node::element(2, "div")
                        .class("label")
                        .child(Node::text(3, "hello")),
                ),
        };
        let rendered = render_document(&document, &RuntimeTheme::default()).unwrap();
        assert_eq!(rendered.batch.width, 160);
        assert!(
            rendered
                .batch
                .commands
                .iter()
                .any(|command| matches!(command, Command::Glyphs(run) if run.glyphs.iter().all(|glyph| glyph.id != 0)))
        );
    }

    #[test]
    fn native_runtime_shapes_arabic_and_devanagari() {
        for text in ["مرحبا", "नमस्ते"] {
            let document = Document {
                version: bexos_dioxus_dom::VERSION,
                width: 240,
                height: 80,
                scale: 1.0,
                clear_rgba: [0, 0, 0, 0],
                stylesheets: Vec::new(),
                author_stylesheets: Vec::new(),
                root: Node::element(1, "main")
                    .style("width:240px;height:80px;font-size:24px")
                    .child(Node::text(2, text)),
            };
            let rendered = render_document(&document, &RuntimeTheme::default()).unwrap();
            assert!(rendered.batch.commands.iter().any(|command| {
                matches!(command, Command::Glyphs(run) if run.glyphs.iter().all(|glyph| glyph.id != 0))
            }));
        }
    }
    #[test]
    fn rtl_direction_moves_layout_and_neutral_text_to_the_right() {
        let doc = |direction| Document {
            version: bexos_dioxus_dom::VERSION,
            width: 240,
            height: 80,
            scale: 1.0,
            clear_rgba: [0; 4],
            stylesheets: Vec::new(),
            author_stylesheets: Vec::new(),
            root: Node::element(1, "main")
                .attribute("lang", "ar")
                .attribute("dir", direction)
                .style(format!(
                    "display:flex;direction:{direction};width:240px;height:80px"
                ))
                .child(
                    Node::element(2, "div")
                        .style("width:100px;height:40px;font-size:24px")
                        .child(Node::text(3, "123")),
                ),
        };
        let left = render_document(&doc("ltr"), &RuntimeTheme::default()).unwrap();
        let right = render_document(&doc("rtl"), &RuntimeTheme::default()).unwrap();
        let x = |rendered: &RenderedDocument| {
            rendered
                .batch
                .commands
                .iter()
                .filter_map(|c| match c {
                    Command::Glyphs(run) => Some(run.x),
                    _ => None,
                })
                .fold(f32::INFINITY, f32::min)
        };
        assert!(
            x(&right) > x(&left) + 100.,
            "RTL must move the child and align its neutral text at the logical start"
        );
    }
}
