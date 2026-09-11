//! Native rendering support for validated Dioxus scene batches.
//!
//! The crate deliberately depends on the public scene ABI rather than on guest
//! Dioxus or Blitz state. Guests and shared WASM UI libraries submit bounded
//! commands; the native runner owns Vello/Venus resources and may fall back to
//! the CPU replay path without exposing renderer objects across the boundary.

use bexos_dioxus_scene::{Command, Paint, PathCommand, Point, Rect, SceneBatch, Transform};
use bexos_flatland::{Damage, Format, Surface};
use bexos_flatland_render::{composition::Layer, vello};
use vello::{
    kurbo::{Affine, BezPath, Rect as VRect, RoundedRect, Stroke},
    peniko::{BlendMode, Color, Fill},
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Error {
    InvalidScene(bexos_dioxus_scene::Error),
    Unsupported,
}

pub fn surface_for(batch: &SceneBatch) -> Result<Surface, Error> {
    batch.validate().map_err(Error::InvalidScene)?;
    let stride = batch.width.checked_mul(4).ok_or(Error::InvalidScene(
        bexos_dioxus_scene::Error::LimitExceeded,
    ))?;
    Ok(Surface {
        width: batch.width,
        height: batch.height,
        stride,
        format: Format::Bgra,
    })
}

pub fn damage_for(batch: &SceneBatch) -> Result<Damage, Error> {
    let surface = surface_for(batch)?;
    Ok(surface.full())
}

pub fn vello_layers(batch: &SceneBatch) -> Result<Vec<Layer>, Error> {
    batch.validate().map_err(Error::InvalidScene)?;
    let mut scene = vello::Scene::new();
    scene.fill(
        Fill::NonZero,
        Affine::IDENTITY,
        color(batch.clear_rgba),
        None,
        &VRect::new(0., 0., batch.width as f64, batch.height as f64),
    );
    let mut state = RenderState::default();
    for command in &batch.commands {
        match command {
            Command::Path(path) => append_path(&mut scene, path, state.transform)?,
            Command::Glyphs(run) => {
                let mut cursor = run.x;
                for glyph in &run.glyphs {
                    let w = glyph.advance.max(1.0);
                    let rect = VRect::new(
                        f64::from(cursor + glyph.x),
                        f64::from(run.y + glyph.y - run.size),
                        f64::from(cursor + glyph.x + w),
                        f64::from(run.y + glyph.y),
                    );
                    scene.fill(
                        Fill::NonZero,
                        state.transform,
                        color(run.color),
                        None,
                        &rect,
                    );
                    cursor += glyph.advance;
                }
            }
            Command::Image(image) => {
                let shade = image.image_asset.wrapping_mul(37) as u8;
                let brush = Color::from_rgba8(
                    shade,
                    shade.saturating_add(40),
                    220,
                    (255. * image.opacity) as u8,
                );
                scene.fill(
                    Fill::NonZero,
                    state.transform,
                    brush,
                    None,
                    &rect(image.rect),
                );
            }
            Command::PushLayer(layer) => {
                scene.push_layer(
                    Fill::NonZero,
                    BlendMode::default(),
                    layer.alpha,
                    state.transform,
                    &rect(layer.clip),
                );
                state.depth += 1;
            }
            Command::PopLayer => {
                if state.depth == 0 {
                    return Err(Error::InvalidScene(
                        bexos_dioxus_scene::Error::LayerUnderflow,
                    ));
                }
                scene.pop_layer();
                state.depth -= 1;
            }
            Command::Clip(clip) => {
                scene.push_layer(
                    Fill::NonZero,
                    BlendMode::default(),
                    1.,
                    state.transform,
                    &rect(*clip),
                );
                state.depth += 1;
            }
            Command::Transform(transform) => {
                state.transform = affine(*transform);
            }
            Command::Shadow(shadow) => {
                let r = shadow.rect;
                let rounded = RoundedRect::new(
                    f64::from(r.x + shadow.dx),
                    f64::from(r.y + shadow.dy),
                    f64::from(r.x + shadow.dx + r.width),
                    f64::from(r.y + shadow.dy + r.height),
                    f64::from(shadow.radius),
                );
                scene.fill(
                    Fill::NonZero,
                    state.transform,
                    color(shadow.color),
                    None,
                    &rounded,
                );
            }
        }
    }
    while state.depth > 0 {
        scene.pop_layer();
        state.depth -= 1;
    }
    Ok(vec![Layer {
        scene,
        backdrop: None,
    }])
}

#[derive(Clone, Copy)]
struct RenderState {
    transform: Affine,
    depth: usize,
}

impl Default for RenderState {
    fn default() -> Self {
        Self {
            transform: Affine::IDENTITY,
            depth: 0,
        }
    }
}

fn append_path(
    scene: &mut vello::Scene,
    path: &PathCommand,
    transform: Affine,
) -> Result<(), Error> {
    let Some(first) = path.points.first().copied() else {
        return Err(Error::InvalidScene(
            bexos_dioxus_scene::Error::InvalidGeometry,
        ));
    };
    let mut p = BezPath::new();
    p.move_to(point(first));
    for point_value in path.points.iter().copied().skip(1) {
        p.line_to(point(point_value));
    }
    if path.closed {
        p.close_path();
    }
    let brush = paint(&path.paint)?;
    if path.stroke_width > 0.0 {
        scene.stroke(
            &Stroke::new(f64::from(path.stroke_width)),
            transform,
            brush,
            None,
            &p,
        );
    } else {
        scene.fill(Fill::NonZero, transform, brush, None, &p);
    }
    Ok(())
}

fn paint(paint: &Paint) -> Result<Color, Error> {
    match paint {
        Paint::Solid(rgba) => Ok(color(*rgba)),
        Paint::LinearGradient { start_rgba, .. } => Ok(color(*start_rgba)),
    }
}

fn color(rgba: [u8; 4]) -> Color {
    Color::from_rgba8(rgba[0], rgba[1], rgba[2], rgba[3])
}

fn rect(r: Rect) -> VRect {
    VRect::new(
        f64::from(r.x),
        f64::from(r.y),
        f64::from(r.x + r.width),
        f64::from(r.y + r.height),
    )
}

fn point(p: Point) -> (f64, f64) {
    (f64::from(p.x), f64::from(p.y))
}

fn affine(t: Transform) -> Affine {
    Affine::new([
        f64::from(t.sx),
        f64::from(t.ky),
        f64::from(t.kx),
        f64::from(t.sy),
        f64::from(t.tx),
        f64::from(t.ty),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use bexos_dioxus_scene::{Glyph, GlyphRun, ImageCommand, Layer as SceneLayer, PathCommand};

    #[test]
    fn converts_mixed_scene_into_one_vello_layer() {
        let batch = SceneBatch {
            width: 64,
            height: 48,
            clear_rgba: [1, 2, 3, 255],
            commands: vec![
                Command::PushLayer(SceneLayer {
                    alpha: 0.8,
                    clip: Rect {
                        x: 0.,
                        y: 0.,
                        width: 64.,
                        height: 48.,
                    },
                }),
                Command::Path(PathCommand {
                    points: vec![
                        Point { x: 4., y: 4. },
                        Point { x: 32., y: 4. },
                        Point { x: 32., y: 24. },
                    ],
                    closed: true,
                    paint: Paint::Solid([240, 10, 20, 255]),
                    stroke_width: 0.,
                }),
                Command::Image(ImageCommand {
                    image_asset: 1,
                    rect: Rect {
                        x: 8.,
                        y: 10.,
                        width: 16.,
                        height: 12.,
                    },
                    opacity: 0.75,
                }),
                Command::Glyphs(GlyphRun {
                    font_asset: 2,
                    size: 12.,
                    x: 3.,
                    y: 30.,
                    color: [255, 255, 255, 255],
                    glyphs: vec![Glyph {
                        id: 1,
                        x: 0.,
                        y: 0.,
                        advance: 7.,
                    }],
                }),
                Command::PopLayer,
            ],
        };
        let layers = vello_layers(&batch).unwrap();
        assert_eq!(layers.len(), 1);
        assert_eq!(surface_for(&batch).unwrap().format, Format::Bgra);
        assert_eq!(damage_for(&batch).unwrap().width, 64);
    }
}
