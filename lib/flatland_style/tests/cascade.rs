use bexos_flatland_style::{Resolver, Theme, dom::Node, metrics::DefaultFontMetrics};
use std::collections::BTreeMap;
fn node(id: u64, parent: Option<u64>, classes: &str, inline: &str) -> Node {
    Node {
        id,
        parent,
        tag: "node".into(),
        classes: classes.into(),
        style: inline.into(),
        attributes: BTreeMap::new(),
        state: 0,
    }
}
fn resolver(css: &str) -> Resolver {
    let metrics =
        bexos_flatland_text::metrics::Metrics::from_font(include_bytes!(env!("NOTO_SANS")))
            .unwrap();
    Resolver::new(
        Theme::new(css).unwrap(),
        1920.,
        1080.,
        1.,
        true,
        Box::new(DefaultFontMetrics(metrics)),
    )
    .unwrap()
}
#[test]
fn specificity_inheritance_inline_and_important_use_the_cascade() {
    let mut r = resolver(
        "node { display: flex; color: #123456; font-size: 20px; } .panel > .title { font-size: 2em; opacity: .25 !important; } #chosen { opacity: .5; }",
    );
    let mut child = node(2, Some(1), "title", "opacity: .9");
    child.attributes.insert("id".into(), "chosen".into());
    let nodes = [node(1, None, "panel", ""), child];
    assert!(r.resolve(&nodes).unwrap());
    let style = r.get(2).unwrap();
    assert_eq!(style.get_effects().clone_opacity(), 0.25);
    assert_eq!(style.get_font().clone_font_size().computed_size().px(), 40.);
    assert_eq!(
        style
            .get_inherited_text()
            .clone_color()
            .to_nscolor()
            .to_le_bytes(),
        [0x12, 0x34, 0x56, 255]
    );
    let generation = r.generation();
    assert!(!r.resolve(&nodes).unwrap());
    assert_eq!(r.generation(), generation);
    assert!(r.update_theme("node { opacity: .7; }").unwrap());
    assert!(r.resolve(&nodes).unwrap());
    assert_eq!(r.get(2).unwrap().get_effects().clone_opacity(), 0.9);
}
#[test]
fn sibling_reorder_and_class_changes_invalidate_structural_selectors() {
    let mut r =
        resolver(".group > node:nth-child(2) { opacity: .5; } .group > .selected { opacity: .2; }");
    let mut nodes = vec![
        node(1, None, "group", ""),
        node(2, Some(1), "", ""),
        node(3, Some(1), "", ""),
    ];
    r.resolve(&nodes).unwrap();
    assert_eq!(r.get(3).unwrap().get_effects().clone_opacity(), 0.5);
    nodes.swap(1, 2);
    r.resolve(&nodes).unwrap();
    assert_eq!(r.get(2).unwrap().get_effects().clone_opacity(), 0.5);
    nodes[1].classes = "selected".into();
    r.resolve(&nodes).unwrap();
    assert_eq!(r.get(3).unwrap().get_effects().clone_opacity(), 0.2);
}
#[test]
fn invalid_trees_never_replace_computed_results() {
    let mut r = resolver("node { opacity: .3; }");
    let good = [node(1, None, "", "")];
    r.resolve(&good).unwrap();
    let generation = r.generation();
    for invalid in [
        vec![node(1, Some(2), "", ""), node(2, Some(1), "", "")],
        vec![node(1, Some(99), "", "")],
        vec![node(1, None, "", ""), node(1, None, "", "")],
    ] {
        assert!(r.resolve(&invalid).is_err());
        assert_eq!(r.generation(), generation);
        assert_eq!(r.get(1).unwrap().get_effects().clone_opacity(), 0.3);
    }
}
#[test]
fn pixel_layout_and_render_properties_come_from_computed_values() {
    let mut r = resolver(
        "node { display:flex; flex-direction:column; font-size:20px; width:4em; height:60px; gap:4px; padding:8px; opacity:.5; border-radius:6px; }",
    );
    r.resolve(&[node(1, None, "", "")]).unwrap();
    let mut layout = Default::default();
    let mut effects = Default::default();
    let mut opacity = 0.8;
    bexos_flatland_style::properties::apply(
        r.get(1).unwrap(),
        &mut layout,
        &mut opacity,
        &mut effects,
    )
    .unwrap();
    assert_eq!(
        (
            layout.mode,
            layout.width,
            layout.height,
            layout.padding,
            layout.gap
        ),
        (2, 80., 60., 8., 4.)
    );
    assert_eq!(effects.corner_radius, 6.);
    assert!((opacity - 0.4).abs() < 0.00001);
}

#[test]
fn equal_css_grid_tracks_project_and_unsupported_tracks_reject_atomically() {
    for (tracks, expected) in [
        ("1fr 1fr", Some(2)),
        ("repeat(3, 2fr 2fr)", Some(6)),
        ("repeat(65, 1fr)", None),
        ("1fr 2fr", None),
        ("20px 20px", None),
    ] {
        let mut r = resolver(&format!(
            "node {{ display:grid; grid-template-columns:{tracks}; opacity:.5; }}"
        ));
        r.resolve(&[node(1, None, "", "")]).unwrap();
        let mut layout = bexos_flatland::layout::Properties::default();
        let original = layout;
        let mut effects = Default::default();
        let mut opacity = 1.;
        let result = bexos_flatland_style::properties::apply(
            r.get(1).unwrap(),
            &mut layout,
            &mut opacity,
            &mut effects,
        );
        if let Some(columns) = expected {
            result.unwrap_or_else(|error| {
                panic!(
                    "{tracks}: {error:?}, display={:?}, columns={:?}",
                    r.get(1).unwrap().get_box().clone_display(),
                    r.get(1)
                        .unwrap()
                        .get_position()
                        .clone_grid_template_columns()
                )
            });
            assert_eq!(layout.columns, columns);
            assert_eq!(layout.mode, 3);
        } else {
            assert!(result.is_err(), "{tracks}");
            assert_eq!(layout, original);
            assert_eq!(opacity, 1.);
        }
    }
}
