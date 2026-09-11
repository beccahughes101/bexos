//! Metrics for the compositor's packaged default sans-serif face. Embedders
//! supporting additional CSS font families can supply their own provider.
use style::{
    device::servo::FontMetricsProvider,
    font_metrics::FontMetrics,
    properties::style_structs::Font,
    values::{
        computed::{CSSPixelLength, Length, font::GenericFontFamily},
        specified::font::QueryFontMetricsFlags,
    },
};
#[derive(Debug)]
pub struct DefaultFontMetrics(pub bexos_flatland_text::metrics::Metrics);
impl FontMetricsProvider for DefaultFontMetrics {
    fn query_font_metrics(
        &self,
        vertical: bool,
        _: &Font,
        base: CSSPixelLength,
        _: QueryFontMetricsFlags,
    ) -> FontMetrics {
        let scale = |v| Length::new(v * base.px());
        FontMetrics {
            ascent: scale(self.0.ascent),
            x_height: self.0.x_height.map(scale),
            cap_height: self.0.cap_height.map(scale),
            zero_advance_measure: if vertical {
                None
            } else {
                self.0.zero_advance.map(scale)
            },
            ic_width: if vertical {
                None
            } else {
                self.0.ideographic_advance.map(scale)
            },
            ..Default::default()
        }
    }
    fn base_size_for_generic(&self, _: GenericFontFamily) -> Length {
        Length::new(16.)
    }
}
