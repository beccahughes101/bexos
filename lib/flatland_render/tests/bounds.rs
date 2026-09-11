use bexos_flatland::{
    Format, Surface,
    resolved::{Item, Rect, Transform},
};
use bexos_flatland_render::bounds::backdrop;
fn item() -> Item {
    let surface = Surface {
        width: 16,
        height: 16,
        stride: 64,
        format: Format::Rgba,
    };
    Item {
        effects: Default::default(),
        node: 1,
        buffer: 1,
        surface,
        transform: Transform::default(),
        inverse_scale: (1., 1.),
        opacity: 1.,
        visible: Rect {
            x: 0.,
            y: 0.,
            width: 16.,
            height: 16.,
        },
        pixels: surface.full(),
    }
}
#[test]
fn offscreen_backdrop_is_empty_and_nonfinite_geometry_is_rejected() {
    let mut i = item();
    i.visible.x = 1e100;
    assert_eq!(backdrop(&i, 128, 96).unwrap().width, 0);
    i.visible.x = -100.;
    assert_eq!(backdrop(&i, 128, 96).unwrap().width, 0);
    i.visible.x = -0.5;
    assert_eq!(backdrop(&i, 128, 96).unwrap().width, 16);
    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        i.visible.x = value;
        assert!(backdrop(&i, 128, 96).is_err());
    }
    let mut i = item();
    i.transform.sx = 0.;
    assert!(backdrop(&i, 128, 96).is_err());
}
