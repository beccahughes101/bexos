//! Native retained document rendering for shell Dioxus documents.

use bexos_dioxus_dom::{Document, Node, NodeKind, StyleOrigin};
use bexos_dioxus_scene::{Command, Layer, Paint, PathCommand, Point, Rect, SceneBatch};
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

pub fn render_document(
    document: &Document,
    theme: &RuntimeTheme,
) -> Result<RenderedDocument, Error> {
    document.validate().map_err(Error::Document)?;
    let stylesheets = stylesheet_set(document, theme);
    let metrics =
        bexos_flatland_text::metrics::Metrics::from_font(include_bytes!(env!("NOTO_SANS")))
            .map_err(|_| Error::UnsupportedLayout)?;
    let mut resolver = Resolver::new(
        Theme::from_stylesheets(&stylesheets).map_err(Error::Style)?,
        document.width as f32,
        document.height as f32,
        document.scale,
        !matches!(
            theme.preferences.color_scheme,
            bexos_ui_theme::ColorScheme::Light
        ),
        Box::new(DefaultFontMetrics(metrics)),
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
        0.0,
        0.0,
        theme,
        &layout,
        &resolver,
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
    parent_x: f32,
    parent_y: f32,
    theme: &RuntimeTheme,
    layout: &bexos_flatland_layout::LayoutTree,
    resolver: &Resolver,
    commands: &mut Vec<Command>,
) -> Result<(), Error> {
    if matches!(node.kind, NodeKind::Text(_)) {
        return Ok(());
    }
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
    for child in &node.children {
        match &child.kind {
            NodeKind::Text(value) => {
                if !value.is_empty() {
                    vector_text(
                        commands,
                        rect.x,
                        rect.y,
                        value,
                        (font_size / 8.0).max(1.0),
                        color,
                    );
                }
            }
            NodeKind::Image { asset } => {
                commands.push(Command::Image(bexos_dioxus_scene::ImageCommand {
                    image_asset: *asset,
                    rect,
                    opacity,
                }));
            }
            NodeKind::Element => emit_node(child, x, y, theme, layout, resolver, commands)?,
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

fn vector_text(
    commands: &mut Vec<Command>,
    x: f32,
    y: f32,
    value: &str,
    scale: f32,
    color: [u8; 4],
) {
    for (i, c) in value.chars().take(64).enumerate() {
        for (row, bits) in glyph(c).into_iter().enumerate() {
            let mut col = 0;
            while col < 5 {
                if bits & (16 >> col) == 0 {
                    col += 1;
                    continue;
                }
                let start = col;
                while col < 5 && bits & (16 >> col) != 0 {
                    col += 1;
                }
                rect_command(
                    commands,
                    Rect {
                        x: x + i as f32 * 6.0 * scale + start as f32 * scale,
                        y: y + row as f32 * scale,
                        width: (col - start) as f32 * scale,
                        height: scale,
                    },
                    color,
                );
            }
        }
    }
}

fn glyph(c: char) -> [u8; 7] {
    match c.to_ascii_uppercase() {
        'A' => [14, 17, 17, 31, 17, 17, 17],
        'B' => [30, 17, 17, 30, 17, 17, 30],
        'C' => [14, 17, 16, 16, 16, 17, 14],
        'D' => [30, 17, 17, 17, 17, 17, 30],
        'E' => [31, 16, 16, 30, 16, 16, 31],
        'F' => [31, 16, 16, 30, 16, 16, 16],
        'G' => [14, 17, 16, 23, 17, 17, 15],
        'H' => [17, 17, 17, 31, 17, 17, 17],
        'I' => [14, 4, 4, 4, 4, 4, 14],
        'J' => [7, 2, 2, 2, 18, 18, 12],
        'K' => [17, 18, 20, 24, 20, 18, 17],
        'L' => [16, 16, 16, 16, 16, 16, 31],
        'M' => [17, 27, 21, 21, 17, 17, 17],
        'N' => [17, 25, 21, 19, 17, 17, 17],
        'O' => [14, 17, 17, 17, 17, 17, 14],
        'P' => [30, 17, 17, 30, 16, 16, 16],
        'Q' => [14, 17, 17, 17, 21, 18, 13],
        'R' => [30, 17, 17, 30, 20, 18, 17],
        'S' => [15, 16, 16, 14, 1, 1, 30],
        'T' => [31, 4, 4, 4, 4, 4, 4],
        'U' => [17, 17, 17, 17, 17, 17, 14],
        'V' => [17, 17, 17, 17, 17, 10, 4],
        'W' => [17, 17, 17, 21, 21, 21, 10],
        'X' => [17, 17, 10, 4, 10, 17, 17],
        'Y' => [17, 17, 10, 4, 4, 4, 4],
        'Z' => [31, 1, 2, 4, 8, 16, 31],
        '0' => [14, 17, 19, 21, 25, 17, 14],
        '1' => [4, 12, 4, 4, 4, 4, 14],
        '2' => [14, 17, 1, 2, 4, 8, 31],
        '3' => [30, 1, 1, 14, 1, 1, 30],
        '4' => [2, 6, 10, 18, 31, 2, 2],
        '5' => [31, 16, 16, 30, 1, 1, 30],
        '6' => [14, 16, 16, 30, 17, 17, 14],
        '7' => [31, 1, 2, 4, 8, 8, 8],
        '8' => [14, 17, 17, 14, 17, 17, 14],
        '9' => [14, 17, 17, 15, 1, 1, 14],
        '.' => [0, 0, 0, 0, 0, 6, 6],
        ':' => [0, 6, 6, 0, 6, 6, 0],
        '-' => [0, 0, 0, 31, 0, 0, 0],
        '_' => [0, 0, 0, 0, 0, 0, 31],
        '/' => [1, 1, 2, 4, 8, 16, 16],
        '*' => [0, 21, 14, 31, 14, 21, 0],
        '>' => [16, 8, 4, 2, 4, 8, 16],
        '<' => [1, 2, 4, 8, 4, 2, 1],
        ' ' => [0; 7],
        _ => [14, 17, 1, 2, 4, 0, 4],
    }
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
                .any(|command| matches!(command, Command::Path(_)))
        );
    }
}
