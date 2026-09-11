use crate::{Error, Surface};
use vello_cpu::color::palette::css::{BLACK, WHITE};
use vello_cpu::kurbo::{Circle, Rect, Shape};
use vello_cpu::{Level, Pixmap, RenderContext, RenderSettings, Resources};
/// Scratch storage is intentionally rebuilt after transplant.
pub struct Renderer {
    context: RenderContext,
    resources: Resources,
    pixels: Pixmap,
    width: u32,
    height: u32,
}
impl Renderer {
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
            resources: Resources::new(),
            pixels: Pixmap::new(surface.width as u16, surface.height as u16),
            width: surface.width,
            height: surface.height,
        })
    }
    pub fn splash(&mut self, now_us: u64, percent: u8) -> &[u8] {
        self.context.reset();
        self.context.set_paint(BLACK);
        self.context
            .fill_rect(&Rect::new(0.0, 0.0, self.width as f64, self.height as f64));
        let scale = (self.width as f64 / 640.0).min(self.height as f64 / 480.0);
        let cx = self.width as f64 / 2.0;
        let cy = self.height as f64 / 2.0;
        // Embedded vector letter cells; no filesystem or platform fonts.
        let letters: [[u8; 7]; 5] = [
            [30, 17, 17, 30, 17, 17, 30],
            [31, 16, 16, 30, 16, 16, 31],
            [17, 17, 10, 4, 10, 17, 17],
            [14, 17, 17, 17, 17, 17, 14],
            [15, 16, 16, 14, 1, 1, 30],
        ];
        self.context.set_paint(WHITE);
        let cell = 4.0 * scale;
        for (letter, rows) in letters.iter().enumerate() {
            for (row, bits) in rows.iter().enumerate() {
                for col in 0..5 {
                    if bits & (1 << (4 - col)) != 0 {
                        let x = cx - 58.0 * scale + (letter * 6 + col) as f64 * cell;
                        let y = cy - 45.0 * scale + row as f64 * cell;
                        self.context.fill_rect(&Rect::new(x, y, x + cell, y + cell));
                    }
                }
            }
        }
        const DOTS: [(f64, f64); 8] = [
            (0., -1.),
            (0.707, -0.707),
            (1., 0.),
            (0.707, 0.707),
            (0., 1.),
            (-0.707, 0.707),
            (-1., 0.),
            (-0.707, -0.707),
        ];
        let phase = (now_us / 100_000 % 8) as usize;
        for (i, (x, y)) in DOTS.iter().enumerate() {
            self.context
                .set_paint(WHITE.with_alpha(0.2 + ((i + 8 - phase) % 8) as f32 * 0.1));
            self.context.fill_path(
                &Circle::new(
                    (cx + x * 13.0 * scale, cy + 14.0 * scale + y * 13.0 * scale),
                    2.4 * scale,
                )
                .to_path(0.1),
            );
        }
        self.context.set_paint(WHITE.with_alpha(0.2));
        self.context.fill_rect(&Rect::new(
            cx - 80.0 * scale,
            cy + 53.0 * scale,
            cx + 80.0 * scale,
            cy + 56.0 * scale,
        ));
        self.context.set_paint(WHITE);
        self.context.fill_rect(&Rect::new(
            cx - 80.0 * scale,
            cy + 53.0 * scale,
            cx - 80.0 * scale + 160.0 * scale * percent.min(100) as f64 / 100.0,
            cy + 56.0 * scale,
        ));
        self.context.flush();
        self.context.render(&mut self.pixels, &mut self.resources);
        self.pixels.data_as_u8_slice()
    }
}
/// First frame (elapsed=0) is byte-identical to the frozen splash.
pub fn cross_fade(source: &[u8], target: &mut [u8], elapsed_us: u64) -> Result<(), Error> {
    if source.len() != target.len() || source.len() % 4 != 0 {
        return Err(Error::Invalid);
    }
    let weight = elapsed_us.min(250_000);
    for (s, d) in source.chunks_exact(4).zip(target.chunks_exact_mut(4)) {
        for i in 0..4 {
            let end = [14u64, 18, 28, 255][i];
            d[i] = ((s[i] as u64 * (250_000 - weight) + end * weight) / 250_000) as u8;
        }
    }
    Ok(())
}
