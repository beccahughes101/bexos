//! Vello GPU rendering independent of display discovery and presentation policy.
//! The caller owns the device/queue and supplies an imported render target.
pub use vello;
pub use wgpu;
pub mod blur;
pub mod bounds;
pub mod composition;
pub mod errors;
pub mod recovery;
pub mod scene;
pub mod timing;
pub struct GpuRenderer {
    renderer: vello::Renderer,
}
impl GpuRenderer {
    pub fn register_texture(&mut self, texture: wgpu::Texture) -> vello::peniko::ImageData {
        let mut image = self.renderer.register_texture(texture);
        image.alpha_type = vello::peniko::ImageAlphaType::AlphaPremultiplied;
        image
    }
    pub fn image_changed(&mut self, image: &vello::peniko::ImageData) {
        self.renderer.mark_override_image_dirty(image);
    }
    pub fn unregister_texture(&mut self, image: vello::peniko::ImageData) {
        self.renderer.unregister_texture(image);
    }
    pub fn new(device: &wgpu::Device) -> Result<Self, vello::Error> {
        Ok(Self {
            renderer: vello::Renderer::new(
                device,
                vello::RendererOptions {
                    use_cpu: false,
                    antialiasing_support: vello::AaSupport::area_only(),
                    num_init_threads: std::num::NonZeroUsize::new(1),
                    pipeline_cache: None,
                },
            )?,
        })
    }
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scene: &vello::Scene,
        target: &wgpu::TextureView,
        width: u32,
        height: u32,
    ) -> Result<(), vello::Error> {
        self.renderer.render_to_texture(
            device,
            queue,
            scene,
            target,
            &vello::RenderParams {
                base_color: vello::peniko::Color::from_rgb8(28, 18, 14),
                width,
                height,
                antialiasing_method: vello::AaConfig::Area,
            },
        )
    }
}
