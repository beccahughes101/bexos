//! Font-table metrics in em units for layout/style clients. These use the same
//! pinned font bytes as Parley, and contain no font-engine pointers.
use skrifa::{
    FontRef, MetadataProvider,
    instance::{LocationRef, Size},
};
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Metrics {
    pub ascent: f32,
    pub x_height: Option<f32>,
    pub cap_height: Option<f32>,
    pub zero_advance: Option<f32>,
    pub ideographic_advance: Option<f32>,
}
impl Metrics {
    pub fn from_font(bytes: &[u8]) -> Result<Self, crate::Error> {
        let font = FontRef::new(bytes).map_err(|_| crate::Error::InvalidFont)?;
        let metrics = font.metrics(Size::new(1.), LocationRef::default());
        if metrics.units_per_em == 0 {
            return Err(crate::Error::InvalidFont);
        }
        let glyphs = font.glyph_metrics(Size::new(1.), LocationRef::default());
        let charmap = font.charmap();
        let advance = |c| charmap.map(c).and_then(|id| glyphs.advance_width(id));
        Ok(Self {
            ascent: metrics.ascent,
            x_height: metrics.x_height,
            cap_height: metrics.cap_height,
            zero_advance: advance('0'),
            ideographic_advance: advance('水'),
        })
    }
}
