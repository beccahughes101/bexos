//! A small real Vello compute-render and texture readback check. Samples avoid
//! antialiased edges; this validates execution, not compositor performance.
use bexos_flatland_render::{GpuRenderer, vello};
use vello::{
    kurbo::{Affine, RoundedRect},
    peniko::{Color, Fill},
};

pub async fn verify_rendering() -> Result<(), String> {
    let gpu = super::Device::new().await?;
    let mut renderer =
        GpuRenderer::new(&gpu.device).map_err(|e| format!("Vello initialization: {e:?}"))?;
    bexos_userspace::log("input-fixture: Vello pipelines created\n");
    let mut scene = vello::Scene::new();
    scene.fill(
        Fill::NonZero,
        Affine::IDENTITY,
        Color::from_rgb8(30, 180, 90),
        None,
        &RoundedRect::new(8., 8., 56., 56., 8.),
    );
    let extent = wgpu::Extent3d {
        width: 64,
        height: 64,
        depth_or_array_layers: 1,
    };
    let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Vello reference target"),
        size: extent,
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
    bexos_userspace::log("input-fixture: Vello target texture created\n");
    renderer
        .render(
            &gpu.device,
            &gpu.queue,
            &scene,
            &target.create_view(&Default::default()),
            64,
            64,
        )
        .map_err(|e| format!("Vello rendering: {e:?}"))?;
    bexos_userspace::log("input-fixture: Vello render submitted\n");
    let pixels = super::readback::texture(&gpu, &target)?;
    let result = [
        (0usize, 0usize, [28u8, 18, 14, 255]),
        (32, 32, [30, 180, 90, 255]),
    ]
    .into_iter()
    .try_for_each(|(x, y, expected)| {
        let actual = &pixels[y * 256 + x * 4..y * 256 + x * 4 + 4];
        if actual.iter().zip(expected).all(|(a, e)| a.abs_diff(e) <= 1) {
            Ok(())
        } else {
            Err(format!(
                "Vello pixel ({x},{y}): {actual:?}, expected {expected:?}"
            ))
        }
    });
    result?;
    super::blur_probe::verify(&gpu, &target, &pixels)?;
    super::composition_probe::verify(&gpu, &mut renderer)?;
    super::text_probe::verify(&gpu, &mut renderer)
}
