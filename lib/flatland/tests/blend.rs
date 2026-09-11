#[path = "../src/blend.rs"]
mod blend;

#[test]
fn vector_rows_match_integer_source_over_for_every_alpha_and_opacity() {
    // Include malformed (non-premultiplied) pixels: saturation still must match.
    for opacity in 0..=255u8 {
        let mut source = Vec::new();
        let mut destination = Vec::new();
        for alpha in 0..=255u8 {
            source.extend_from_slice(&[alpha.wrapping_mul(37), 255 - alpha, alpha / 2, alpha]);
            destination.extend_from_slice(&[255 - alpha, alpha.wrapping_mul(13), 255, alpha]);
        }
        let mut expected = destination.clone();
        for (src, dst) in source.chunks_exact(4).zip(expected.chunks_exact_mut(4)) {
            let alpha = (u32::from(src[3]) * u32::from(opacity) + 127) / 255;
            for c in 0..4 {
                dst[c] = ((u32::from(src[c]) * u32::from(opacity) + 127) / 255
                    + (u32::from(dst[c]) * (255 - alpha) + 127) / 255)
                    .min(255) as u8;
            }
        }
        blend::row(&source, &mut destination, opacity);
        assert_eq!(destination, expected, "opacity {opacity}");
    }
}

#[test]
fn unaligned_rows_and_short_tails_do_not_touch_neighboring_bytes() {
    for offset in 0..16 {
        for pixels in 0..20 {
            let mut destination = [213u8; 128];
            let source = [128u8; 128];
            let end = offset + pixels * 4;
            blend::row(&source[offset..end], &mut destination[offset..end], 137);
            assert!(destination[..offset].iter().all(|v| *v == 213));
            assert!(destination[end..].iter().all(|v| *v == 213));
            assert!(destination[offset..end].iter().all(|v| *v == 224));
        }
    }
}
