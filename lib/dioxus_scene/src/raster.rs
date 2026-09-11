use crate::{Command, Paint, Rect, SceneBatch};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Surface {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub format: u32,
}

pub fn rasterize(batch: &SceneBatch) -> Result<(Surface, Vec<u8>), crate::Error> {
    batch.validate()?;
    let stride = batch
        .width
        .checked_mul(4)
        .ok_or(crate::Error::LimitExceeded)?;
    let len = stride
        .checked_mul(batch.height)
        .and_then(|v| usize::try_from(v).ok())
        .ok_or(crate::Error::LimitExceeded)?;
    let mut pixels = vec![0; len];
    fill_rect(
        &mut pixels,
        batch.width,
        batch.height,
        stride,
        Rect {
            x: 0.0,
            y: 0.0,
            width: batch.width as f32,
            height: batch.height as f32,
        },
        batch.clear_rgba,
        1.0,
    );
    let mut clip = Rect {
        x: 0.0,
        y: 0.0,
        width: batch.width as f32,
        height: batch.height as f32,
    };
    let mut stack = Vec::new();
    for command in &batch.commands {
        match command {
            Command::Path(path) => {
                let mut min_x = f32::INFINITY;
                let mut min_y = f32::INFINITY;
                let mut max_x = f32::NEG_INFINITY;
                let mut max_y = f32::NEG_INFINITY;
                for point in &path.points {
                    min_x = min_x.min(point.x);
                    min_y = min_y.min(point.y);
                    max_x = max_x.max(point.x);
                    max_y = max_y.max(point.y);
                }
                let rect = intersect(
                    Rect {
                        x: min_x,
                        y: min_y,
                        width: max_x - min_x,
                        height: max_y - min_y,
                    },
                    clip,
                );
                let rgba = match path.paint {
                    Paint::Solid(rgba) => rgba,
                    Paint::LinearGradient { start_rgba, .. } => start_rgba,
                };
                fill_rect(
                    &mut pixels,
                    batch.width,
                    batch.height,
                    stride,
                    rect,
                    rgba,
                    1.0,
                );
            }
            Command::Image(image) => {
                let gray = ((image.image_asset.wrapping_mul(37)) & 0xff) as u8;
                fill_rect(
                    &mut pixels,
                    batch.width,
                    batch.height,
                    stride,
                    intersect(image.rect, clip),
                    [gray, gray.saturating_add(40), 220, 255],
                    image.opacity,
                );
            }
            Command::Glyphs(run) => {
                let width = run.glyphs.iter().map(|g| g.advance.max(1.0)).sum::<f32>();
                fill_rect(
                    &mut pixels,
                    batch.width,
                    batch.height,
                    stride,
                    intersect(
                        Rect {
                            x: run.x,
                            y: run.y - run.size,
                            width,
                            height: run.size,
                        },
                        clip,
                    ),
                    run.color,
                    1.0,
                );
            }
            Command::PushLayer(layer) => {
                stack.push(clip);
                clip = intersect(clip, layer.clip);
            }
            Command::PopLayer => {
                clip = stack.pop().ok_or(crate::Error::LayerUnderflow)?;
            }
            Command::Clip(rect) => clip = intersect(clip, *rect),
            Command::Shadow(shadow) => fill_rect(
                &mut pixels,
                batch.width,
                batch.height,
                stride,
                intersect(
                    Rect {
                        x: shadow.rect.x + shadow.dx,
                        y: shadow.rect.y + shadow.dy,
                        width: shadow.rect.width,
                        height: shadow.rect.height,
                    },
                    clip,
                ),
                shadow.color,
                0.5,
            ),
            Command::Transform(_) => {}
        }
    }
    Ok((
        Surface {
            width: batch.width,
            height: batch.height,
            stride,
            format: 1,
        },
        pixels,
    ))
}

fn intersect(a: Rect, b: Rect) -> Rect {
    let x0 = a.x.max(b.x);
    let y0 = a.y.max(b.y);
    let x1 = (a.x + a.width).min(b.x + b.width);
    let y1 = (a.y + a.height).min(b.y + b.height);
    Rect {
        x: x0,
        y: y0,
        width: (x1 - x0).max(0.0),
        height: (y1 - y0).max(0.0),
    }
}

fn fill_rect(
    pixels: &mut [u8],
    width: u32,
    height: u32,
    stride: u32,
    rect: Rect,
    rgba: [u8; 4],
    opacity: f32,
) {
    let x0 = rect.x.floor().max(0.0) as u32;
    let y0 = rect.y.floor().max(0.0) as u32;
    let x1 = (rect.x + rect.width).ceil().max(0.0).min(width as f32) as u32;
    let y1 = (rect.y + rect.height).ceil().max(0.0).min(height as f32) as u32;
    let alpha = (f32::from(rgba[3]) * opacity).clamp(0.0, 255.0) as u8;
    for y in y0..y1 {
        for x in x0..x1 {
            let i = (y * stride + x * 4) as usize;
            pixels[i] = rgba[2];
            pixels[i + 1] = rgba[1];
            pixels[i + 2] = rgba[0];
            pixels[i + 3] = alpha;
        }
    }
}
