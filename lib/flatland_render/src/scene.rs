use bexos_flatland::{Error, resolved::Snapshot};
use bexos_flatland_text::{Layout, PositionedLayoutItem};
use vello::{
    Scene,
    kurbo::{Affine, Rect, RoundedRect},
    peniko::{BlendMode, Color, Fill, ImageData},
};

pub fn append_surfaces<'a>(
    scene: &mut Scene,
    snapshot: &Snapshot,
    mut image: impl FnMut(u64) -> Option<&'a ImageData>,
) -> Result<(), Error> {
    for item in &snapshot.items {
        let image = image(item.buffer).ok_or(Error::Invalid)?;
        if image.width != item.surface.width || image.height != item.surface.height {
            return Err(Error::Bounds);
        }
        let r = item.visible;
        scene.push_layer(
            Fill::NonZero,
            BlendMode::default(),
            item.opacity,
            Affine::IDENTITY,
            &Rect::new(r.x, r.y, r.x + r.width, r.y + r.height),
        );
        let t = item.transform;
        let transform = Affine::new([t.sx, 0., 0., t.sy, t.x, t.y]);
        if item.effects.corner_radius != 0. {
            scene.push_layer(
                Fill::NonZero,
                BlendMode::default(),
                1.,
                transform,
                &RoundedRect::new(
                    0.,
                    0.,
                    item.surface.width as f64,
                    item.surface.height as f64,
                    item.effects.corner_radius as f64,
                ),
            );
        }
        scene.draw_image(image, transform);
        if item.effects.corner_radius != 0. {
            scene.pop_layer();
        }
        scene.pop_layer();
    }
    Ok(())
}
pub fn append_text(scene: &mut Scene, layout: &Layout<[u8; 4]>, x: f64, y: f64) {
    for line in layout.lines() {
        for item in line.items() {
            if let PositionedLayoutItem::GlyphRun(glyph_run) = item {
                let run = glyph_run.run();
                let color = glyph_run.style().brush;
                let mut cursor = glyph_run.offset();
                let baseline = glyph_run.baseline();
                scene
                    .draw_glyphs(run.font())
                    .font_size(run.font_size())
                    .normalized_coords(run.normalized_coords())
                    .transform(Affine::translate((x, y)))
                    .brush(Color::from_rgba8(color[0], color[1], color[2], color[3]))
                    .draw(
                        Fill::NonZero,
                        glyph_run.glyphs().map(|g| {
                            let glyph = vello::Glyph {
                                id: g.id,
                                x: cursor + g.x,
                                y: baseline - g.y,
                            };
                            cursor += g.advance;
                            glyph
                        }),
                    );
            }
        }
    }
}
