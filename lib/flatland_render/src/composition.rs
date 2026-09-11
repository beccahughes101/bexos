//! Ordered Vello content and backdrop passes. The source and blur pyramids are
//! retained; only the dependency halo of a visible backdrop is filtered.
use crate::{GpuRenderer, blur::DualKawase};
use bexos_flatland::{Error, resolved::Item};
use std::collections::BTreeMap;
use vello::{
    Scene,
    kurbo::{Affine, Rect, RoundedRect},
    peniko::{BlendMode, Fill, ImageData},
};
pub const MAX_BACKDROPS: usize = 32;
pub struct Layer {
    pub scene: Scene,
    pub backdrop: Option<Item>,
}
struct Blur {
    kernel: DualKawase,
    image: ImageData,
}
pub struct Composition {
    source: wgpu::Texture,
    previous: ImageData,
    blur: BTreeMap<usize, Blur>,
    scene: Scene,
}
impl Composition {
    pub fn new(device: &wgpu::Device, renderer: &mut GpuRenderer, target: &wgpu::Texture) -> Self {
        let source = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("retained backdrop source"),
            size: target.size(),
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        Self {
            source,
            previous: renderer.register_texture(target.clone()),
            blur: BTreeMap::new(),
            scene: Scene::new(),
        }
    }
    pub fn release(self, renderer: &mut GpuRenderer) {
        renderer.unregister_texture(self.previous);
        for (_, blur) in self.blur {
            renderer.unregister_texture(blur.image);
        }
    }
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        renderer: &mut GpuRenderer,
        layers: &[Layer],
        target: &wgpu::Texture,
        view: &wgpu::TextureView,
    ) -> Result<(), String> {
        if layers.is_empty() || layers.len() > MAX_BACKDROPS + 1 || layers[0].backdrop.is_some() {
            return Err("invalid GPU composition layers".into());
        }
        // Validate all public geometry before submitting the base pass. A bad
        // later layer must not leave a partially rendered frame in the target.
        for layer in &layers[1..] {
            crate::bounds::backdrop(
                layer.backdrop.as_ref().ok_or("missing backdrop geometry")?,
                target.width(),
                target.height(),
            )
            .map_err(|_| "invalid backdrop geometry")?;
        }
        renderer
            .render(
                device,
                queue,
                &layers[0].scene,
                view,
                target.width(),
                target.height(),
            )
            .map_err(|e| format!("Vello base: {e:?}"))?;
        for layer in &layers[1..] {
            let item = layer.backdrop.as_ref().ok_or("missing backdrop geometry")?;
            let visible = item.visible;
            let damage = crate::bounds::backdrop(item, target.width(), target.height())
                .map_err(|_| "invalid backdrop geometry")?;
            renderer.image_changed(&self.previous);
            self.scene.reset();
            self.scene.draw_image(&self.previous, Affine::IDENTITY);
            if damage.width != 0 && damage.height != 0 && item.effects.backdrop_radius != 0 {
                // Public radius selects a bounded downsampling depth (1..=5).
                let levels = (u32::BITS - item.effects.backdrop_radius.max(1).leading_zeros())
                    .min(5) as usize;
                if !self.blur.contains_key(&levels) {
                    let kernel = DualKawase::new(device, &self.source, levels)
                        .map_err(|e| format!("backdrop allocation: {e:?}"))?;
                    let image = renderer.register_texture(kernel.output().clone());
                    self.blur.insert(levels, Blur { kernel, image });
                }
                let blur = &self.blur[&levels];
                let region = blur
                    .kernel
                    .source_damage(damage)
                    .map_err(|_| "invalid backdrop damage")?;
                let mut encoder = device.create_command_encoder(&Default::default());
                let origin = wgpu::Origin3d {
                    x: region.x,
                    y: region.y,
                    z: 0,
                };
                encoder.copy_texture_to_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: target,
                        mip_level: 0,
                        origin,
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::TexelCopyTextureInfo {
                        texture: &self.source,
                        mip_level: 0,
                        origin,
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::Extent3d {
                        width: region.width,
                        height: region.height,
                        depth_or_array_layers: 1,
                    },
                );
                blur.kernel
                    .encode(&mut encoder, damage)
                    .map_err(|e: Error| format!("backdrop encoding: {e:?}"))?;
                queue.submit([encoder.finish()]);
                // Vello copies overrides into its atlas before writing the target.
                // The previous composition can therefore be reused without a CPU
                // readback or a second full-size compositor target.
                renderer.image_changed(&blur.image);
                self.scene.push_layer(
                    Fill::NonZero,
                    BlendMode::default(),
                    1.,
                    Affine::IDENTITY,
                    &Rect::new(
                        visible.x,
                        visible.y,
                        visible.x + visible.width,
                        visible.y + visible.height,
                    ),
                );
                let t = item.transform;
                self.scene.push_layer(
                    Fill::NonZero,
                    BlendMode::default(),
                    1.,
                    Affine::new([t.sx, 0., 0., t.sy, t.x, t.y]),
                    &RoundedRect::new(
                        0.,
                        0.,
                        item.surface.width as f64,
                        item.surface.height as f64,
                        item.effects.corner_radius as f64,
                    ),
                );
                self.scene.draw_image(&blur.image, Affine::IDENTITY);
                self.scene.pop_layer();
                self.scene.pop_layer();
            }
            // Empty/offscreen backdrops still carry foreground content.
            self.scene.append(&layer.scene, None);
            renderer
                .render(
                    device,
                    queue,
                    &self.scene,
                    view,
                    target.width(),
                    target.height(),
                )
                .map_err(|e| format!("Vello backdrop composition: {e:?}"))?;
        }
        Ok(())
    }
}
