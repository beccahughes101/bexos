//! Compare actual CPU and Vulkan glyph rasterization from one Parley layout.
use bexos_flatland::{Format, Surface};
use bexos_flatland_render::{
    GpuRenderer,
    vello::{
        self,
        kurbo::{Affine, Rect},
        peniko::{Color, Fill},
    },
};
pub(crate) fn verify(gpu: &super::Device, renderer: &mut GpuRenderer) -> Result<(), String> {
    let mut text = bexos_flatland_text::TextEngine::default();
    for font in [
        include_bytes!(env!("NOTO_SANS")).as_slice(),
        include_bytes!(env!("NOTO_ARABIC")).as_slice(),
        include_bytes!(env!("NOTO_DEVANAGARI")).as_slice(),
    ] {
        text.register_font(font.to_vec())
            .map_err(|e| format!("reference font: {e:?}"))?;
    }
    let surface = Surface {
        width: 320,
        height: 128,
        stride: 1280,
        format: Format::Rgba,
    };
    let layout = text
        .shape(
            "BexOS\nمرحبا بالعالم\nनमस्ते दुनिया",
            bexos_flatland_text::TextStyle {
                width: 300.,
                size: 24.,
                color: [255; 4],
                ..Default::default()
            },
        )
        .map_err(|e| format!("reference shaping: {e:?}"))?;
    let mut raster = bexos_flatland_cpu::Rasterizer::new(surface)
        .map_err(|e| format!("reference raster: {e:?}"))?;
    let cpu = raster
        .text(&layout, 8., 4.)
        .map_err(|e| format!("reference glyphs: {e:?}"))?;
    let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("CPU/Vulkan multilingual reference"),
        size: wgpu::Extent3d {
            width: surface.width,
            height: surface.height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let mut scene = vello::Scene::new();
    scene.fill(
        Fill::NonZero,
        Affine::IDENTITY,
        Color::BLACK,
        None,
        &Rect::new(0., 0., 320., 128.),
    );
    bexos_flatland_render::scene::append_text(&mut scene, &layout, 8., 4.);
    renderer
        .render(
            &gpu.device,
            &gpu.queue,
            &scene,
            &target.create_view(&Default::default()),
            320,
            128,
        )
        .map_err(|e| format!("reference Vulkan text: {e:?}"))?;
    let pixels = super::readback::texture(gpu, &target)?;
    // Compare only the union of ink pixels; a mostly empty target cannot hide
    // missing glyphs. Different antialiasers may disagree at glyph boundaries.
    let mut ink = 0u64;
    let mut error = 0u64;
    let mut misplaced = 0u64;
    for y in 1..127usize {
        for x in 1..319usize {
            let index = (y * 320 + x) * 4;
            if cpu[index] <= 8 && pixels[index] <= 8 {
                continue;
            }
            ink += 1;
            error += cpu[index].abs_diff(pixels[index]) as u64;
            let near = |bytes: &[u8]| {
                (y - 1..=y + 1).any(|ny| (x - 1..=x + 1).any(|nx| bytes[(ny * 320 + nx) * 4] > 8))
            };
            if !near(cpu) || !near(&pixels) {
                misplaced += 1;
            }
        }
    }
    if ink < 500 || error > ink * 24 || misplaced * 100 > ink * 2 {
        return Err(format!(
            "CPU/Vulkan text mismatch: ink={ink} absolute_error={error} misplaced={misplaced}"
        ));
    }
    bexos_userspace::log(&format!(
        "input-fixture: CPU/Vulkan multilingual glyphs verified ink={ink} absolute_error={error} misplaced={misplaced}\n"
    ));
    Ok(())
}
