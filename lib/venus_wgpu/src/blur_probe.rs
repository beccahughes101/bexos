//! Compare full and partial GPU blur with the shared mathematical reference.
use bexos_flatland::Damage;
pub(crate) fn verify(
    gpu: &super::Device,
    source: &wgpu::Texture,
    original: &[u8],
) -> Result<(), String> {
    let (width, height) = (source.width(), source.height());
    let full = Damage {
        x: 0,
        y: 0,
        width,
        height,
    };
    let blur = bexos_flatland_render::blur::DualKawase::new(&gpu.device, source, 2)
        .map_err(|e| format!("Kawase construction: {e:?}"))?;
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    blur.encode(&mut encoder, full)
        .map_err(|e| format!("Kawase encoding: {e:?}"))?;
    gpu.queue.submit([encoder.finish()]);
    let first = super::readback::texture(gpu, blur.output())?;
    let expected = bexos_kawase_reference::render(width, height, 2, original, full, false);
    compare(&first, &expected, width, full)?;

    let mut changed = original.to_vec();
    let patch: Vec<u8> = (0..8 * 8).flat_map(|_| [230, 40, 150, 255]).collect();
    for y in 0..8usize {
        let offset = ((y + 24) * width as usize + 24) * 4;
        changed[offset..offset + 32].copy_from_slice(&patch[y * 32..y * 32 + 32]);
    }
    gpu.queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: source,
            mip_level: 0,
            origin: wgpu::Origin3d { x: 24, y: 24, z: 0 },
            aspect: wgpu::TextureAspect::All,
        },
        &patch,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(32),
            rows_per_image: Some(8),
        },
        wgpu::Extent3d {
            width: 8,
            height: 8,
            depth_or_array_layers: 1,
        },
    );
    let partial = Damage {
        x: 20,
        y: 20,
        width: 24,
        height: 24,
    };
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    blur.encode(&mut encoder, partial)
        .map_err(|e| format!("Kawase repair: {e:?}"))?;
    gpu.queue.submit([encoder.finish()]);
    let repaired = super::readback::texture(gpu, blur.output())?;
    let expected = bexos_kawase_reference::render(width, height, 2, &changed, full, false);
    compare(&repaired, &expected, width, partial)?;
    for y in 0..height {
        for x in 0..width {
            if x < partial.x
                || y < partial.y
                || x >= partial.x + partial.width
                || y >= partial.y + partial.height
            {
                let index = ((y * width + x) * 4) as usize;
                if repaired[index..index + 4] != first[index..index + 4] {
                    return Err(format!("Kawase modified undamaged pixel ({x},{y})"));
                }
            }
        }
    }
    bexos_userspace::log("input-fixture: Dual Kawase full/partial GPU reference verified\n");
    Ok(())
}
fn compare(actual: &[u8], expected: &[u8], width: u32, damage: Damage) -> Result<(), String> {
    for y in damage.y..damage.y + damage.height {
        for x in damage.x..damage.x + damage.width {
            let index = ((y * width + x) * 4) as usize;
            if actual[index..index + 4]
                .iter()
                .zip(&expected[index..index + 4])
                .any(|(a, b)| a.abs_diff(*b) > 3)
            {
                return Err(format!(
                    "Kawase pixel ({x},{y}): {:?}, expected {:?}",
                    &actual[index..index + 4],
                    &expected[index..index + 4]
                ));
            }
        }
    }
    Ok(())
}
