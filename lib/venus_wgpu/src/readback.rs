//! Fixture-only texture readback. Allocation and CPU copying here serve pixel
//! validation; this is not a compositor presentation path.
use std::sync::{
    Arc,
    atomic::{AtomicU8, Ordering},
};
pub(crate) fn texture(gpu: &super::Device, texture: &wgpu::Texture) -> Result<Vec<u8>, String> {
    let width = texture.width();
    let height = texture.height();
    let stride = (width * 4).div_ceil(256) * 256;
    let output = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Venus fixture readback"),
        size: u64::from(stride) * u64::from(height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &output,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(stride),
                rows_per_image: Some(height),
            },
        },
        texture.size(),
    );
    let submission = gpu.queue.submit([encoder.finish()]);
    let completion = Arc::new(AtomicU8::new(0));
    let callback = completion.clone();
    output
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            callback.store(if result.is_ok() { 1 } else { 2 }, Ordering::Release);
        });
    gpu.device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(submission),
            timeout: Some(std::time::Duration::from_secs(5)),
        })
        .map_err(|e| format!("Venus readback completion: {e:?}"))?;
    if completion.load(Ordering::Acquire) != 1 {
        return Err("Venus readback mapping failed".into());
    }
    let mut pixels = vec![0; (width * height * 4) as usize];
    {
        let mapped = output.slice(..).get_mapped_range();
        for (row, destination) in mapped
            .chunks_exact(stride as usize)
            .zip(pixels.chunks_exact_mut(width as usize * 4))
        {
            destination.copy_from_slice(&row[..destination.len()]);
        }
    }
    output.unmap();
    Ok(pixels)
}
