use bexos_flatland_layout::*;
#[test]
fn flex_resize_and_cycle_rejection() {
    let mut t = LayoutTree::default();
    t.create(
        1,
        Style {
            display: Display::Flex,
            size: Size {
                width: Dimension::percent(1.),
                height: Dimension::percent(1.),
            },
            ..Default::default()
        },
    )
    .unwrap();
    for id in [2, 3] {
        t.create(
            id,
            Style {
                flex_grow: 1.,
                ..Default::default()
            },
        )
        .unwrap();
    }
    t.set_children(1, &[2, 3]).unwrap();
    assert!(t.set_children(2, &[1]).is_err());
    t.compute(1, 800., 600.).unwrap();
    assert_eq!(t.bounds(2).unwrap().width, 400.);
    t.compute(1, 1920., 1080.).unwrap();
    assert_eq!(t.bounds(3).unwrap().x, 960.);
}
