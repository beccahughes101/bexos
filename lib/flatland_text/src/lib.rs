//! Parley layout with explicitly supplied font data and bounded retained caches.
//! System font discovery is disabled in Bazel: all fonts come from packages.
use parley::{Alignment, AlignmentOptions, FontContext, LayoutContext, StyleProperty};
pub use parley::{Layout, PositionedLayoutItem};
pub mod metrics;
use std::{collections::VecDeque, sync::Arc};
#[derive(Clone, Debug, PartialEq)]
pub struct TextStyle {
    pub size: f32,
    pub width: f32,
    pub scale: f32,
    pub color: [u8; 4],
    pub language: Option<String>,
    pub rtl: bool,
}
impl Default for TextStyle {
    fn default() -> Self {
        Self {
            size: 16.,
            width: 1024.,
            scale: 1.,
            color: [255; 4],
            language: None,
            rtl: false,
        }
    }
}
struct Cached {
    text: String,
    style: TextStyle,
    layout: Arc<Layout<[u8; 4]>>,
}
#[derive(Default)]
pub struct TextEngine {
    fonts: FontContext,
    context: LayoutContext<[u8; 4]>,
    cache: VecDeque<Cached>,
}
#[derive(Debug, PartialEq)]
pub enum Error {
    InvalidFont,
    InvalidStyle,
    TextTooLong,
}
impl TextEngine {
    pub fn register_font(&mut self, data: Vec<u8>) -> Result<(), Error> {
        if data.is_empty() || data.len() > 32 * 1024 * 1024 {
            return Err(Error::InvalidFont);
        }
        let families = self.fonts.collection.register_fonts(data.into(), None);
        if families.is_empty() {
            return Err(Error::InvalidFont);
        }
        self.fonts.collection.append_generic_families(
            parley::fontique::GenericFamily::SansSerif,
            families.iter().map(|(id, _)| *id),
        );
        self.cache.clear();
        Ok(())
    }
    pub fn register_shared_font<T>(&mut self, data: Arc<T>) -> Result<(), Error>
    where
        T: AsRef<[u8]> + Send + Sync + 'static,
    {
        if data.as_ref().as_ref().is_empty() || data.as_ref().as_ref().len() > 32 * 1024 * 1024 {
            return Err(Error::InvalidFont);
        }
        let blob = parley::fontique::Blob::new(data);
        let families = self.fonts.collection.register_fonts(blob, None);
        if families.is_empty() {
            return Err(Error::InvalidFont);
        }
        self.fonts.collection.append_generic_families(
            parley::fontique::GenericFamily::SansSerif,
            families.iter().map(|(id, _)| *id),
        );
        self.cache.clear();
        Ok(())
    }
    pub fn shape(&mut self, text: &str, style: TextStyle) -> Result<Arc<Layout<[u8; 4]>>, Error> {
        if text.len() > 64 * 1024 {
            return Err(Error::TextTooLong);
        }
        if !style.size.is_finite()
            || !(1. ..=512.).contains(&style.size)
            || !style.width.is_finite()
            || style.width <= 0.
            || !style.scale.is_finite()
            || !(0.1..=16.).contains(&style.scale)
        {
            return Err(Error::InvalidStyle);
        }
        if let Some(index) = self
            .cache
            .iter()
            .position(|c| c.text == text && c.style == style)
        {
            let cached = self.cache.remove(index).unwrap();
            let out = cached.layout.clone();
            self.cache.push_back(cached);
            return Ok(out);
        }
        // Parley infers paragraph direction from the first strong character.
        // A non-rendering direction mark carries the computed CSS base direction
        // even for neutral text or a paragraph beginning with an embedded name.
        let directed = format!("{}{text}", if style.rtl { '\u{200f}' } else { '\u{200e}' });
        let mut builder =
            self.context
                .ranged_builder(&mut self.fonts, &directed, style.scale, true);
        builder.push_default(StyleProperty::FontSize(style.size));
        builder.push_default(StyleProperty::Brush(style.color));
        builder.push_default(StyleProperty::Locale(
            style.language.as_deref().and_then(|v| v.parse().ok()),
        ));
        let mut layout = builder.build(&directed);
        layout.break_all_lines(Some(style.width));
        layout.align(Alignment::Start, AlignmentOptions::default());
        let layout = Arc::new(layout);
        if self.cache.len() == 128 {
            self.cache.pop_front();
        }
        self.cache.push_back(Cached {
            text: text.into(),
            style,
            layout: layout.clone(),
        });
        Ok(layout)
    }
    pub fn clear_cache(&mut self) {
        self.cache.clear();
    }
}
