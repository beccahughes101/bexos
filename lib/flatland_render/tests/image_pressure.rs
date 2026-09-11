use std::sync::Arc;
use vello::{
    Scene,
    kurbo::Affine,
    peniko::{Blob, ImageAlphaType, ImageData, ImageFormat},
};
use vello_encoding::Resolver;

fn image(value: u8) -> ImageData {
    ImageData {
        data: Blob::new(Arc::new(vec![value; 800 * 600 * 4])),
        format: ImageFormat::Rgba8,
        alpha_type: ImageAlphaType::AlphaPremultiplied,
        width: 800,
        height: 600,
    }
}

#[test]
fn replacing_fullscreen_images_reclaims_previous_scene_before_growing_atlas() {
    let mut resolver = Resolver::default();
    let mut packed = Vec::new();
    for n in 0..16 {
        let mut scene = Scene::new();
        let image = image(n);
        scene.draw_image(&image, Affine::IDENTITY);
        let (_, _, atlas) = resolver.resolve(scene.encoding(), &mut packed);
        assert_eq!((atlas.width, atlas.height), (1024, 1024));
        assert_eq!(atlas.images.len(), 1);
        assert_eq!(atlas.images[0].0.data.id(), image.data.id());
    }
}

#[test]
fn pressure_does_not_evict_images_already_resolved_in_the_current_scene() {
    let mut resolver = Resolver::default();
    let mut packed = Vec::new();
    let a = image(1);
    let b = image(2);
    let mut scene = Scene::new();
    scene.draw_image(&a, Affine::IDENTITY);
    scene.draw_image(&b, Affine::translate((10., 10.)));
    let (_, _, atlas) = resolver.resolve(scene.encoding(), &mut packed);
    assert_eq!((atlas.width, atlas.height), (2048, 2048));
    let ids: Vec<_> = atlas.images.iter().map(|image| image.0.data.id()).collect();
    assert!(ids.contains(&a.data.id()) && ids.contains(&b.data.id()));
    let (_, _, atlas) = resolver.resolve(scene.encoding(), &mut packed);
    assert_eq!((atlas.width, atlas.height), (2048, 2048));
    assert!(atlas.images.is_empty());
}
