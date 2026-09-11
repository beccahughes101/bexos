use bexos_flatland_text::*;
#[test]
fn multilingual_layout_and_cache() {
    let mut e = TextEngine::default();
    for data in [
        include_bytes!(env!("NOTO_SANS")).as_slice(),
        include_bytes!(env!("NOTO_ARABIC")).as_slice(),
        include_bytes!(env!("NOTO_DEVANAGARI")).as_slice(),
    ] {
        e.register_font(data.to_vec()).unwrap();
    }
    for text in [
        "BexOS ready",
        "مرحبا بالعالم",
        "नमस्ते दुनिया",
        "abc مرحبا 123",
    ] {
        let a = e.shape(text, TextStyle::default()).unwrap();
        let b = e.shape(text, TextStyle::default()).unwrap();
        assert!(std::sync::Arc::ptr_eq(&a, &b));
        assert!(a.width() > 0.);
        assert!(a.height() > 0.);
        let mut glyphs = 0;
        for line in a.lines() {
            for item in line.items() {
                if let PositionedLayoutItem::GlyphRun(run) = item {
                    for glyph in run.glyphs() {
                        assert_ne!(glyph.id, 0, "missing glyph in {text}");
                        glyphs += 1;
                    }
                }
            }
        }
        assert!(glyphs > 0);
    }
}
#[test]
fn rejects_invalid_inputs() {
    let mut e = TextEngine::default();
    assert_eq!(e.register_font(vec![0; 16]), Err(Error::InvalidFont));
    assert!(
        e.shape(
            "x",
            TextStyle {
                size: f32::NAN,
                ..Default::default()
            }
        )
        .is_err()
    );
}
