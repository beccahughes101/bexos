//! Bounded scene batches exchanged between the Dioxus WASM UI library and the
//! native runner. Native GPU objects, VMOs, and renderer caches are deliberately
//! absent from this ABI.

mod raster;
mod wire;

pub use raster::{Surface, rasterize};

pub const VERSION: u32 = 1;
pub const MAX_COMMANDS: usize = 4096;
pub const MAX_PATH_POINTS: usize = 8192;
pub const MAX_GLYPHS: usize = 8192;
pub const MAX_ASSETS: usize = 2048;
pub const MAX_DIMENSION: u32 = 4096;
pub const MAX_LAYER_DEPTH: usize = 64;
pub const MAX_BATCH_BYTES: usize = 4 << 20;

#[derive(Clone, Debug, PartialEq)]
pub struct SceneBatch {
    pub width: u32,
    pub height: u32,
    pub clear_rgba: [u8; 4],
    pub commands: Vec<Command>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    Path(PathCommand),
    Glyphs(GlyphRun),
    Image(ImageCommand),
    PushLayer(Layer),
    PopLayer,
    Clip(Rect),
    Transform(Transform),
    Shadow(Shadow),
}

#[derive(Clone, Debug, PartialEq)]
pub struct PathCommand {
    pub points: Vec<Point>,
    pub closed: bool,
    pub paint: Paint,
    pub stroke_width: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GlyphRun {
    pub font_asset: u32,
    pub size: f32,
    pub x: f32,
    pub y: f32,
    pub color: [u8; 4],
    pub glyphs: Vec<Glyph>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Glyph {
    pub id: u32,
    pub x: f32,
    pub y: f32,
    pub advance: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ImageCommand {
    pub image_asset: u32,
    pub rect: Rect,
    pub opacity: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Layer {
    pub alpha: f32,
    pub clip: Rect,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Shadow {
    pub rect: Rect,
    pub radius: f32,
    pub dx: f32,
    pub dy: f32,
    pub color: [u8; 4],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    pub sx: f32,
    pub kx: f32,
    pub ky: f32,
    pub sy: f32,
    pub tx: f32,
    pub ty: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Paint {
    Solid([u8; 4]),
    LinearGradient {
        start: Point,
        end: Point,
        start_rgba: [u8; 4],
        end_rgba: [u8; 4],
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Error {
    InvalidEncoding,
    InvalidVersion,
    LimitExceeded,
    NonFinite,
    InvalidGeometry,
    InvalidAsset,
    LayerUnderflow,
    LayerOverflow,
    Unsupported,
}

impl SceneBatch {
    pub fn validate(&self) -> Result<(), Error> {
        if self.width == 0
            || self.height == 0
            || self.width > MAX_DIMENSION
            || self.height > MAX_DIMENSION
        {
            return Err(Error::InvalidGeometry);
        }
        if self.commands.len() > MAX_COMMANDS {
            return Err(Error::LimitExceeded);
        }
        let mut depth = 0usize;
        let mut points = 0usize;
        let mut glyphs = 0usize;
        for command in &self.commands {
            match command {
                Command::Path(path) => {
                    if path.points.len() < 2 || path.points.len() > MAX_PATH_POINTS {
                        return Err(Error::InvalidGeometry);
                    }
                    points = points
                        .checked_add(path.points.len())
                        .ok_or(Error::LimitExceeded)?;
                    if points > MAX_PATH_POINTS {
                        return Err(Error::LimitExceeded);
                    }
                    finite_f32(path.stroke_width)?;
                    validate_paint(&path.paint)?;
                    for point in &path.points {
                        validate_point(*point)?;
                    }
                }
                Command::Glyphs(run) => {
                    if run.font_asset == 0 || run.glyphs.is_empty() {
                        return Err(Error::InvalidAsset);
                    }
                    finite_f32(run.size)?;
                    finite_f32(run.x)?;
                    finite_f32(run.y)?;
                    if run.size <= 0.0 || run.size > 512.0 {
                        return Err(Error::InvalidGeometry);
                    }
                    glyphs = glyphs
                        .checked_add(run.glyphs.len())
                        .ok_or(Error::LimitExceeded)?;
                    if glyphs > MAX_GLYPHS {
                        return Err(Error::LimitExceeded);
                    }
                    for glyph in &run.glyphs {
                        finite_f32(glyph.x)?;
                        finite_f32(glyph.y)?;
                        finite_f32(glyph.advance)?;
                    }
                }
                Command::Image(image) => {
                    if image.image_asset == 0 || image.image_asset as usize > MAX_ASSETS {
                        return Err(Error::InvalidAsset);
                    }
                    validate_rect(image.rect)?;
                    finite_f32(image.opacity)?;
                    if !(0.0..=1.0).contains(&image.opacity) {
                        return Err(Error::InvalidGeometry);
                    }
                }
                Command::PushLayer(layer) => {
                    finite_f32(layer.alpha)?;
                    if !(0.0..=1.0).contains(&layer.alpha) {
                        return Err(Error::InvalidGeometry);
                    }
                    validate_rect(layer.clip)?;
                    depth += 1;
                    if depth > MAX_LAYER_DEPTH {
                        return Err(Error::LayerOverflow);
                    }
                }
                Command::PopLayer => {
                    depth = depth.checked_sub(1).ok_or(Error::LayerUnderflow)?;
                }
                Command::Clip(rect) => validate_rect(*rect)?,
                Command::Transform(transform) => {
                    finite_f32(transform.sx)?;
                    finite_f32(transform.kx)?;
                    finite_f32(transform.ky)?;
                    finite_f32(transform.sy)?;
                    finite_f32(transform.tx)?;
                    finite_f32(transform.ty)?;
                }
                Command::Shadow(shadow) => {
                    validate_rect(shadow.rect)?;
                    finite_f32(shadow.radius)?;
                    finite_f32(shadow.dx)?;
                    finite_f32(shadow.dy)?;
                    if shadow.radius < 0.0 || shadow.radius > 256.0 {
                        return Err(Error::InvalidGeometry);
                    }
                }
            }
        }
        if depth != 0 {
            return Err(Error::LayerUnderflow);
        }
        Ok(())
    }

    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        self.validate()?;
        let mut out = Vec::new();
        wire::u32(&mut out, VERSION);
        wire::u32(&mut out, self.width);
        wire::u32(&mut out, self.height);
        out.extend_from_slice(&self.clear_rgba);
        wire::u32(&mut out, self.commands.len() as u32);
        for command in &self.commands {
            encode_command(&mut out, command);
        }
        if out.len() > MAX_BATCH_BYTES {
            return Err(Error::LimitExceeded);
        }
        Ok(out)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() > MAX_BATCH_BYTES {
            return Err(Error::LimitExceeded);
        }
        let mut reader = wire::Reader::new(bytes);
        if reader.u32()? != VERSION {
            return Err(Error::InvalidVersion);
        }
        let width = reader.u32()?;
        let height = reader.u32()?;
        let clear_rgba = reader.bytes4()?;
        let count = reader.count(MAX_COMMANDS)?;
        let mut commands = Vec::with_capacity(count);
        for _ in 0..count {
            commands.push(decode_command(&mut reader)?);
        }
        reader.finish()?;
        let batch = Self {
            width,
            height,
            clear_rgba,
            commands,
        };
        batch.validate()?;
        Ok(batch)
    }
}

fn validate_paint(paint: &Paint) -> Result<(), Error> {
    match paint {
        Paint::Solid(_) => Ok(()),
        Paint::LinearGradient { start, end, .. } => {
            validate_point(*start)?;
            validate_point(*end)
        }
    }
}

fn validate_point(point: Point) -> Result<(), Error> {
    finite_f32(point.x)?;
    finite_f32(point.y)
}

fn validate_rect(rect: Rect) -> Result<(), Error> {
    finite_f32(rect.x)?;
    finite_f32(rect.y)?;
    finite_f32(rect.width)?;
    finite_f32(rect.height)?;
    if rect.width < 0.0 || rect.height < 0.0 {
        return Err(Error::InvalidGeometry);
    }
    Ok(())
}

fn finite_f32(value: f32) -> Result<(), Error> {
    if value.is_finite() {
        Ok(())
    } else {
        Err(Error::NonFinite)
    }
}

fn encode_command(out: &mut Vec<u8>, command: &Command) {
    match command {
        Command::Path(path) => {
            out.push(1);
            wire::u32(out, path.points.len() as u32);
            for point in &path.points {
                wire::f32(out, point.x);
                wire::f32(out, point.y);
            }
            out.push(path.closed as u8);
            encode_paint(out, &path.paint);
            wire::f32(out, path.stroke_width);
        }
        Command::Glyphs(run) => {
            out.push(2);
            wire::u32(out, run.font_asset);
            wire::f32(out, run.size);
            wire::f32(out, run.x);
            wire::f32(out, run.y);
            out.extend_from_slice(&run.color);
            wire::u32(out, run.glyphs.len() as u32);
            for glyph in &run.glyphs {
                wire::u32(out, glyph.id);
                wire::f32(out, glyph.x);
                wire::f32(out, glyph.y);
                wire::f32(out, glyph.advance);
            }
        }
        Command::Image(image) => {
            out.push(3);
            wire::u32(out, image.image_asset);
            encode_rect(out, image.rect);
            wire::f32(out, image.opacity);
        }
        Command::PushLayer(layer) => {
            out.push(4);
            wire::f32(out, layer.alpha);
            encode_rect(out, layer.clip);
        }
        Command::PopLayer => out.push(5),
        Command::Clip(rect) => {
            out.push(6);
            encode_rect(out, *rect);
        }
        Command::Transform(transform) => {
            out.push(7);
            wire::f32(out, transform.sx);
            wire::f32(out, transform.kx);
            wire::f32(out, transform.ky);
            wire::f32(out, transform.sy);
            wire::f32(out, transform.tx);
            wire::f32(out, transform.ty);
        }
        Command::Shadow(shadow) => {
            out.push(8);
            encode_rect(out, shadow.rect);
            wire::f32(out, shadow.radius);
            wire::f32(out, shadow.dx);
            wire::f32(out, shadow.dy);
            out.extend_from_slice(&shadow.color);
        }
    }
}

fn decode_command(reader: &mut wire::Reader<'_>) -> Result<Command, Error> {
    match reader.byte()? {
        1 => {
            let count = reader.count(MAX_PATH_POINTS)?;
            let mut points = Vec::with_capacity(count);
            for _ in 0..count {
                points.push(Point {
                    x: reader.f32()?,
                    y: reader.f32()?,
                });
            }
            Ok(Command::Path(PathCommand {
                points,
                closed: reader.byte()? != 0,
                paint: decode_paint(reader)?,
                stroke_width: reader.f32()?,
            }))
        }
        2 => {
            let font_asset = reader.u32()?;
            let size = reader.f32()?;
            let x = reader.f32()?;
            let y = reader.f32()?;
            let color = reader.bytes4()?;
            let count = reader.count(MAX_GLYPHS)?;
            let mut glyphs = Vec::with_capacity(count);
            for _ in 0..count {
                glyphs.push(Glyph {
                    id: reader.u32()?,
                    x: reader.f32()?,
                    y: reader.f32()?,
                    advance: reader.f32()?,
                });
            }
            Ok(Command::Glyphs(GlyphRun {
                font_asset,
                size,
                x,
                y,
                color,
                glyphs,
            }))
        }
        3 => Ok(Command::Image(ImageCommand {
            image_asset: reader.u32()?,
            rect: decode_rect(reader)?,
            opacity: reader.f32()?,
        })),
        4 => Ok(Command::PushLayer(Layer {
            alpha: reader.f32()?,
            clip: decode_rect(reader)?,
        })),
        5 => Ok(Command::PopLayer),
        6 => Ok(Command::Clip(decode_rect(reader)?)),
        7 => Ok(Command::Transform(Transform {
            sx: reader.f32()?,
            kx: reader.f32()?,
            ky: reader.f32()?,
            sy: reader.f32()?,
            tx: reader.f32()?,
            ty: reader.f32()?,
        })),
        8 => Ok(Command::Shadow(Shadow {
            rect: decode_rect(reader)?,
            radius: reader.f32()?,
            dx: reader.f32()?,
            dy: reader.f32()?,
            color: reader.bytes4()?,
        })),
        _ => Err(Error::Unsupported),
    }
}

fn encode_rect(out: &mut Vec<u8>, rect: Rect) {
    wire::f32(out, rect.x);
    wire::f32(out, rect.y);
    wire::f32(out, rect.width);
    wire::f32(out, rect.height);
}

fn decode_rect(reader: &mut wire::Reader<'_>) -> Result<Rect, Error> {
    Ok(Rect {
        x: reader.f32()?,
        y: reader.f32()?,
        width: reader.f32()?,
        height: reader.f32()?,
    })
}

fn encode_paint(out: &mut Vec<u8>, paint: &Paint) {
    match paint {
        Paint::Solid(rgba) => {
            out.push(1);
            out.extend_from_slice(rgba);
        }
        Paint::LinearGradient {
            start,
            end,
            start_rgba,
            end_rgba,
        } => {
            out.push(2);
            wire::f32(out, start.x);
            wire::f32(out, start.y);
            wire::f32(out, end.x);
            wire::f32(out, end.y);
            out.extend_from_slice(start_rgba);
            out.extend_from_slice(end_rgba);
        }
    }
}

fn decode_paint(reader: &mut wire::Reader<'_>) -> Result<Paint, Error> {
    match reader.byte()? {
        1 => Ok(Paint::Solid(reader.bytes4()?)),
        2 => Ok(Paint::LinearGradient {
            start: Point {
                x: reader.f32()?,
                y: reader.f32()?,
            },
            end: Point {
                x: reader.f32()?,
                y: reader.f32()?,
            },
            start_rgba: reader.bytes4()?,
            end_rgba: reader.bytes4()?,
        }),
        _ => Err(Error::Unsupported),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> SceneBatch {
        SceneBatch {
            width: 64,
            height: 32,
            clear_rgba: [1, 2, 3, 255],
            commands: vec![
                Command::PushLayer(Layer {
                    alpha: 1.0,
                    clip: Rect {
                        x: 0.0,
                        y: 0.0,
                        width: 64.0,
                        height: 32.0,
                    },
                }),
                Command::Path(PathCommand {
                    points: vec![
                        Point { x: 1.0, y: 1.0 },
                        Point { x: 20.0, y: 1.0 },
                        Point { x: 20.0, y: 20.0 },
                    ],
                    closed: true,
                    paint: Paint::Solid([200, 10, 20, 255]),
                    stroke_width: 0.0,
                }),
                Command::PopLayer,
            ],
        }
    }

    #[test]
    fn roundtrip_validates_complete_batch() {
        let batch = sample();
        let encoded = batch.encode().unwrap();
        assert_eq!(SceneBatch::decode(&encoded).unwrap(), batch);
    }

    #[test]
    fn rejects_invalid_layers_before_rendering() {
        let mut batch = sample();
        batch.commands.pop();
        assert_eq!(batch.validate(), Err(Error::LayerUnderflow));
    }

    #[test]
    fn rejects_non_finite_geometry() {
        let mut batch = sample();
        let Command::Path(path) = &mut batch.commands[1] else {
            panic!("path");
        };
        path.points[0].x = f32::NAN;
        assert_eq!(batch.encode(), Err(Error::NonFinite));
    }
}
