use bexos_flatland::{
    accessibility::*,
    resolved::{Rect, Transform},
    *,
};
fn surface() -> Surface {
    Surface {
        width: 8,
        height: 6,
        stride: 36,
        format: Format::Rgba,
    }
}
#[test]
fn magnification_preserves_inverse_geometry_and_padding() {
    let s = surface();
    let mut src = vec![0; s.validate(u64::MAX).unwrap()];
    for y in 0..s.height {
        for x in 0..s.width {
            let i = (y * s.stride + x * 4) as usize;
            src[i..i + 4].copy_from_slice(&[x as u8, y as u8, 30, 255]);
        }
    }
    let mut dst = vec![77; src.len()];
    let t = DisplayTransform {
        scale: 2.,
        origin_x: 2.,
        origin_y: 1.,
        filter: 0,
    };
    t.apply(s, &src, &mut dst, s.full()).unwrap();
    assert_eq!(&dst[..4], &[2, 1, 30, 255]);
    assert_eq!(&dst[28..32], &[5, 1, 30, 255]);
    assert!(
        dst.chunks_exact(s.stride as usize)
            .all(|row| row[32..].iter().all(|b| *b == 77))
    );
    let world = t.world(Transform {
        x: 3.,
        y: 2.,
        sx: 1.5,
        sy: 2.,
    });
    assert_eq!(
        world,
        Transform {
            x: 2.,
            y: 2.,
            sx: 3.,
            sy: 4.
        }
    );
    assert_eq!(t.local(world.x, world.y), (3., 2.));
    let old = dst.clone();
    let invalid = DisplayTransform {
        scale: f64::NAN,
        ..t
    };
    assert!(invalid.apply(s, &src, &mut dst, s.full()).is_err());
    assert_eq!(dst, old);
}
#[test]
fn filters_preserve_premultiplied_alpha_and_channel_order() {
    for format in [Format::Rgba, Format::Bgra] {
        let s = Surface {
            width: 1,
            height: 1,
            stride: 4,
            format,
        };
        let mut src = [80, 40, 20, 100];
        if format == Format::Bgra {
            src.swap(0, 2);
        }
        for filter in 1..=5 {
            let mut dst = [0; 4];
            DisplayTransform {
                filter,
                ..Default::default()
            }
            .apply(s, &src, &mut dst, s.full())
            .unwrap();
            assert_eq!(dst[3], 100);
            assert!(dst[..3].iter().all(|v| *v <= 100));
            if format == Format::Bgra {
                dst.swap(0, 2);
            }
            if filter == 2 {
                assert_eq!(dst, [20, 60, 80, 100]);
            }
            if filter == 1 || filter == 5 {
                assert_eq!(dst[0], dst[1]);
                assert_eq!(dst[1], dst[2]);
            }
        }
    }
}
#[test]
fn focus_ring_has_a_hollow_center_and_respects_committed_clipping() {
    let s = surface();
    let mut pixels = vec![0; s.validate(u64::MAX).unwrap()];
    let bounds = Rect {
        x: 1.,
        y: 1.,
        width: 6.,
        height: 4.,
    };
    let clip = Rect {
        x: 2.,
        y: 0.,
        width: 6.,
        height: 6.,
    };
    focus_ring(s, &mut pixels, bounds, clip, 1, 0xff0000ff).unwrap();
    for y in 0..s.height {
        for x in 0..s.width {
            let alpha = pixels[(y * s.stride + x * 4 + 3) as usize];
            let border = x >= 2 && x < 7 && y >= 1 && y < 5 && (x == 6 || y == 1 || y == 4);
            assert_eq!(alpha, if border { 255 } else { 0 }, "{x},{y}");
        }
    }
}
