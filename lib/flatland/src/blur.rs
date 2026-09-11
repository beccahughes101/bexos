//! Bounded, separable box blur for the CPU backdrop fallback. Only output bounds
//! and their sampling halo are visited. A sliding row ring permits in-place
//! operation without cloning the surface or allocating during a frame.
use crate::{Damage, Error, Surface};
use alloc::{vec, vec::Vec};
pub const MAX_RADIUS: u32 = 32;
pub struct BoxBlur {
    rows: Vec<u8>,
    sums: Vec<u32>,
    max_width: u32,
}
impl BoxBlur {
    pub fn new(max_width: u32) -> Result<Self, Error> {
        if max_width == 0 || max_width > 4096 {
            return Err(Error::Bounds);
        }
        Ok(Self {
            rows: vec![0; max_width as usize * 4 * (MAX_RADIUS as usize * 2 + 1)],
            sums: vec![0; max_width as usize * 4],
            max_width,
        })
    }
    /// RGBA or BGRA premultiplied channels are filtered independently, with
    /// replicated surface edges. Each one-dimensional pass rounds to nearest;
    /// this differs by at most one channel value from an exact square average.
    pub fn apply(
        &mut self,
        surface: Surface,
        pixels: &mut [u8],
        output: Damage,
        radius: u32,
    ) -> Result<(), Error> {
        self.apply_masked(surface, pixels, output, radius, |_, _| true)
    }
    pub fn apply_masked(
        &mut self,
        surface: Surface,
        pixels: &mut [u8],
        output: Damage,
        radius: u32,
        mut mask: impl FnMut(u32, u32) -> bool,
    ) -> Result<(), Error> {
        surface.validate(pixels.len() as u64)?;
        output.validate(surface)?;
        if radius > MAX_RADIUS || output.width > self.max_width {
            return Err(Error::Bounds);
        }
        if radius == 0 {
            return Ok(());
        }
        let diameter = radius as usize * 2 + 1;
        let average = Average::new(diameter as u32);
        let width = output.width as usize * 4;
        let rows = &mut self.rows[..width * diameter];
        let sums = &mut self.sums[..width];
        sums.fill(0);
        for index in 0..diameter {
            let y = (output.y as i64 + index as i64 - radius as i64)
                .clamp(0, surface.height as i64 - 1) as u32;
            let row = &mut rows[index * width..(index + 1) * width];
            horizontal(surface, pixels, output, y, radius, row);
            for (sum, byte) in sums.iter_mut().zip(row) {
                *sum += *byte as u32;
            }
        }
        for index in 0..output.height as usize {
            let y = output.y as usize + index;
            let offset = y * surface.stride as usize + output.x as usize * 4;
            for (x, (pixel, sum)) in pixels[offset..offset + width]
                .chunks_exact_mut(4)
                .zip(sums.chunks_exact(4))
                .enumerate()
            {
                if mask(output.x + x as u32, y as u32) {
                    for (byte, sum) in pixel.iter_mut().zip(sum) {
                        *byte = average.channel(*sum);
                    }
                }
            }
            if index + 1 == output.height as usize {
                break;
            }
            let slot = index % diameter;
            let row = &mut rows[slot * width..(slot + 1) * width];
            for (sum, byte) in sums.iter_mut().zip(row.iter()) {
                *sum -= *byte as u32;
            }
            // This source row is strictly below the just-written output row,
            // including bottom-edge replication. Its original pixels survive.
            let incoming = (y + radius as usize + 1).min(surface.height as usize - 1);
            horizontal(surface, pixels, output, incoming as u32, radius, row);
            for (sum, byte) in sums.iter_mut().zip(row) {
                *sum += *byte as u32;
            }
        }
        Ok(())
    }
}
fn horizontal(
    surface: Surface,
    pixels: &[u8],
    output: Damage,
    y: u32,
    radius: u32,
    row: &mut [u8],
) {
    let source = &pixels[y as usize * surface.stride as usize..][..surface.width as usize * 4];
    let diameter = radius * 2 + 1;
    let average = Average::new(diameter);
    let sample = |x: i64| -> &[u8] {
        let x = x.clamp(0, surface.width as i64 - 1) as usize;
        &source[x * 4..x * 4 + 4]
    };
    let mut sum = [0u32; 4];
    for offset in -(radius as i64)..=radius as i64 {
        for (sum, byte) in sum.iter_mut().zip(sample(output.x as i64 + offset)) {
            *sum += *byte as u32;
        }
    }
    for (index, pixel) in row.chunks_exact_mut(4).enumerate() {
        for (byte, sum) in pixel.iter_mut().zip(sum) {
            *byte = average.channel(sum);
        }
        let x = output.x as i64 + index as i64;
        for (sum, byte) in sum.iter_mut().zip(sample(x - radius as i64)) {
            *sum -= *byte as u32;
        }
        for (sum, byte) in sum.iter_mut().zip(sample(x + radius as i64 + 1)) {
            *sum += *byte as u32;
        }
    }
}
/// Exact division for sums of at most 65 eight-bit values. A fixed-point
/// reciprocal and one remainder correction avoid eight variable divisions per
/// output pixel. The product is below 2^24 throughout the admitted range.
struct Average {
    divisor: u32,
    reciprocal: u32,
}
impl Average {
    fn new(divisor: u32) -> Self {
        Self {
            divisor,
            reciprocal: 65536 / divisor,
        }
    }
    #[inline(always)]
    fn channel(&self, sum: u32) -> u8 {
        let numerator = sum + self.divisor / 2;
        let quotient = (numerator * self.reciprocal) >> 16;
        (quotient + (numerator - quotient * self.divisor >= self.divisor) as u32) as u8
    }
}
