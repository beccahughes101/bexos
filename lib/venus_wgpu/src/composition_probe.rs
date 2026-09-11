//! Exercise the same scene on CPU and Vulkan, including retained backdrop
//! composition and foreground on an empty backdrop layer.
use bexos_flatland_render::{
    GpuRenderer,
    composition::{Composition, Layer},
    vello,
};
use vello::{
    kurbo::{Affine, Rect},
    peniko::{Blob, Color, Fill, ImageAlphaType, ImageData, ImageFormat},
};
pub(crate) fn verify(gpu: &super::Device, renderer: &mut GpuRenderer) -> Result<(), String> {
    let reference = bexos_flatland_reference::Reference::new();
    let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("CPU/Vulkan common reference"),
        size: wgpu::Extent3d {
            width: 128,
            height: 96,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::STORAGE_BINDING
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let images: Vec<_> = reference
        .images
        .iter()
        .map(|(id, surface, pixels)| {
            (
                *id,
                ImageData {
                    data: Blob::new(std::sync::Arc::new(pixels.clone())),
                    format: ImageFormat::Rgba8,
                    alpha_type: ImageAlphaType::AlphaPremultiplied,
                    width: surface.width,
                    height: surface.height,
                },
            )
        })
        .collect();
    let mut scene = vello::Scene::new();
    bexos_flatland_render::scene::append_surfaces(&mut scene, &reference.snapshot, |id| {
        images.iter().find(|i| i.0 == id).map(|i| &i.1)
    })
    .map_err(|e| format!("reference scene: {e:?}"))?;
    let view = target.create_view(&Default::default());
    renderer
        .render(&gpu.device, &gpu.queue, &scene, &view, 128, 96)
        .map_err(|e| format!("reference render: {e:?}"))?;
    reference.compare(&super::readback::texture(gpu, &target)?)?;
    let mut item = reference.snapshot.items[2];
    // Uniform opaque background makes both bounded box blur and Kawase exact
    // here, while testing their clipping and composition order independently.
    item.visible = bexos_flatland::resolved::Rect {
        x: 16.,
        y: 16.,
        width: 32.,
        height: 32.,
    };
    item.transform = bexos_flatland::resolved::Transform {
        x: 16.,
        y: 16.,
        ..Default::default()
    };
    item.effects.backdrop_radius = 4;
    let mut base = vello::Scene::new();
    base.fill(
        Fill::NonZero,
        Affine::IDENTITY,
        Color::from_rgb8(40, 80, 120),
        None,
        &Rect::new(0., 0., 128., 96.),
    );
    let mut foreground = vello::Scene::new();
    foreground.fill(
        Fill::NonZero,
        Affine::IDENTITY,
        Color::from_rgb8(220, 30, 70),
        None,
        &Rect::new(80., 60., 100., 80.),
    );
    let mut offscreen = item;
    offscreen.visible.x = 500.;
    let layers = [
        Layer {
            scene: base,
            backdrop: None,
        },
        Layer {
            scene: vello::Scene::new(),
            backdrop: Some(item),
        },
        Layer {
            scene: foreground,
            backdrop: Some(offscreen),
        },
    ];
    let mut composition = Composition::new(&gpu.device, renderer, &target);
    let result = composition.render(&gpu.device, &gpu.queue, renderer, &layers, &target, &view);
    composition.release(renderer);
    result?;
    let pixels = super::readback::texture(gpu, &target)?;
    for (x, y, expected) in [
        (32, 32, [40u8, 80, 120, 255]),
        (90, 70, [220, 30, 70, 255]),
        (2, 2, [40, 80, 120, 255]),
    ] {
        let i = (y * 128 + x) * 4;
        if pixels[i..i + 4]
            .iter()
            .zip(expected)
            .any(|(a, b)| a.abs_diff(b) > 2)
        {
            return Err(format!(
                "backdrop/foreground order pixel ({x},{y}): {:?}",
                &pixels[i..i + 4]
            ));
        }
    }
    bexos_userspace::log("input-fixture: CPU/Vulkan scene and backdrop ordering verified\n");
    Ok(())
}
