//! Deliberately simple software reference for host/guest rendering comparisons.
//! This allocates between passes and is not a compositor rendering backend.
use bexos_flatland::{Damage, kawase::Pyramid};
pub fn render(
    width: u32,
    height: u32,
    levels: usize,
    input: &[u8],
    output: Damage,
    partial: bool,
) -> Vec<u8> {
    assert_eq!(input.len(), (width * height * 4) as usize);
    let pyramid = Pyramid::new(width, height, levels).unwrap();
    let plan = pyramid.plan(output).unwrap();
    let mut targets: Vec<Vec<u8>> = pyramid
        .sizes()
        .iter()
        .map(|&(w, h)| vec![213; (w * h * 4) as usize])
        .collect();
    for (index, pass) in plan.passes().iter().enumerate() {
        let source = if index == 0 {
            input.to_vec()
        } else {
            targets[pass.source].clone()
        };
        let (sw, sh) = pyramid.sizes()[pass.source];
        let (dw, dh) = pyramid.sizes()[pass.target];
        let rect = if partial {
            pass.damage
        } else {
            Damage {
                x: 0,
                y: 0,
                width: dw,
                height: dh,
            }
        };
        let taps: &[(f64, f64, f64)] = if pass.downsample {
            &[
                (0., 0., 4.),
                (-1., -1., 1.),
                (1., -1., 1.),
                (-1., 1., 1.),
                (1., 1., 1.),
            ]
        } else {
            &[
                (-2., 0., 1.),
                (2., 0., 1.),
                (0., -2., 1.),
                (0., 2., 1.),
                (-1., -1., 2.),
                (1., -1., 2.),
                (-1., 1., 2.),
                (1., 1., 2.),
            ]
        };
        let weight = if pass.downsample { 8. } else { 12. };
        for y in rect.y..rect.y + rect.height {
            for x in rect.x..rect.x + rect.width {
                for channel in 0..4 {
                    let sample = |dx: f64, dy: f64| {
                        let sx = (x as f64 + 0.5) * sw as f64 / dw as f64 - 0.5 + dx;
                        let sy = (y as f64 + 0.5) * sh as f64 / dh as f64 - 0.5 + dy;
                        let x0 = sx.floor();
                        let y0 = sy.floor();
                        let fx = sx - x0;
                        let fy = sy - y0;
                        let pixel = |px: f64, py: f64| {
                            let px = px.clamp(0., (sw - 1) as f64) as u32;
                            let py = py.clamp(0., (sh - 1) as f64) as u32;
                            source[((py * sw + px) * 4) as usize + channel] as f64
                        };
                        (pixel(x0, y0) * (1. - fx) + pixel(x0 + 1., y0) * fx) * (1. - fy)
                            + (pixel(x0, y0 + 1.) * (1. - fx) + pixel(x0 + 1., y0 + 1.) * fx) * fy
                    };
                    let value: f64 = taps.iter().map(|&(dx, dy, w)| sample(dx, dy) * w).sum();
                    targets[pass.target][((y * dw + x) * 4) as usize + channel] =
                        (value / weight).round() as u8;
                }
            }
        }
    }
    targets.remove(0)
}
