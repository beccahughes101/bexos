mod allocations;
mod measurement;
use bexos_flatland::{Damage, Format, Surface, blur::BoxBlur};
fn main() {
    let surface = Surface {
        width: 1920,
        height: 1080,
        stride: 7680,
        format: Format::Bgra,
    };
    let mut pixels = vec![0u8; surface.validate(u64::MAX).unwrap()];
    let mut blur = BoxBlur::new(surface.width).unwrap();
    for (name, output, radius) in [
        (
            "backdrop_640x480",
            Damage {
                x: 300,
                y: 200,
                width: 640,
                height: 480,
            },
            16,
        ),
        (
            "backdrop_partial_64x64",
            Damage {
                x: 400,
                y: 300,
                width: 64,
                height: 64,
            },
            16,
        ),
        ("backdrop_fullscreen", surface.full(), 32),
    ] {
        for (index, pixel) in pixels.chunks_exact_mut(4).enumerate() {
            pixel.copy_from_slice(&[(index % 256) as u8, ((index / 1920) % 256) as u8, 100, 255]);
        }
        measurement::measure(name, output.width as u64 * output.height as u64 * 4, |_| {
            blur.apply(surface, &mut pixels, output, radius).unwrap();
            std::hint::black_box(&pixels);
        });
    }
}
