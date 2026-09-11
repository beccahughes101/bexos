//! Retained Vello CPU resources for compositor-owned vector and text content.
//! Rasterize only when logical content changes, then reuse the resulting pixels.
use bexos_flatland::{Error, Surface};
use bexos_flatland_text::{Layout, PositionedLayoutItem};
use vello_cpu::{Level, Pixmap, RenderContext, RenderSettings, Resources};
pub struct Rasterizer {
    context: RenderContext,
    resources: Resources,
    pixels: Pixmap,
    surface: Surface,
}
impl Rasterizer {
    pub fn new(surface: Surface) -> Result<Self, Error> {
        surface.validate(u64::MAX)?;
        Ok(Self {
            context: RenderContext::new_with(
                surface.width as u16,
                surface.height as u16,
                RenderSettings {
                    level: Level::baseline(),
                    num_threads: 0,
                },
            ),
            resources: Resources::with_glyph_atlas_size(512, 512, Level::baseline()),
            pixels: Pixmap::new(surface.width as u16, surface.height as u16),
            surface,
        })
    }
    pub fn text(&mut self, layout: &Layout<[u8; 4]>, x: f64, y: f64) -> Result<&[u8], Error> {
        if !x.is_finite() || !y.is_finite() {
            return Err(Error::Bounds);
        }
        self.context.reset();
        self.context
            .set_transform(vello_cpu::kurbo::Affine::translate((x, y)));
        for line in layout.lines() {
            for item in line.items() {
                if let PositionedLayoutItem::GlyphRun(glyph_run) = item {
                    let run = glyph_run.run();
                    let color = glyph_run.style().brush;
                    self.context.set_paint(vello_cpu::peniko::Color::from_rgba8(
                        color[0], color[1], color[2], color[3],
                    ));
                    let mut cursor = glyph_run.offset();
                    let baseline = glyph_run.baseline();
                    self.context
                        .glyph_run(&mut self.resources, run.font())
                        .font_size(run.font_size())
                        .normalized_coords(run.normalized_coords())
                        .atlas_cache(true)
                        .fill_glyphs(glyph_run.glyphs().map(move |g| {
                            let glyph = vello_cpu::Glyph {
                                id: g.id,
                                x: cursor + g.x,
                                y: baseline - g.y,
                            };
                            cursor += g.advance;
                            glyph
                        }));
                }
            }
        }
        self.context.flush();
        self.context.render(&mut self.pixels, &mut self.resources);
        Ok(self.pixels.data_as_u8_slice())
    }
    /// Vello CPU pixels are tightly packed, premultiplied RGBA. Convert while
    /// copying into a destination surface whose channel order may be BGRA.
    pub fn copy_to(&self, output: &mut [u8]) -> Result<(), Error> {
        self.surface.validate(output.len() as u64)?;
        let packed = self.surface.width as usize * 4;
        for (src, dst) in self
            .pixels
            .data_as_u8_slice()
            .chunks_exact(packed)
            .zip(output.chunks_exact_mut(self.surface.stride as usize))
        {
            dst[..packed].copy_from_slice(src);
            if self.surface.format.is_bgra() {
                for pixel in dst[..packed].chunks_exact_mut(4) {
                    pixel.swap(0, 2);
                }
            }
            if self.surface.format.is_opaque() {
                for pixel in dst[..packed].chunks_exact_mut(4) {
                    pixel[3] = 255;
                }
            }
        }
        Ok(())
    }
}
