//! Rebuildable desktop text/layout cache; no process-local pointers migrate.
use crate::desktop_config::DesktopConfig;
use bexos_flatland_cpu::Rasterizer;
use bexos_flatland_layout::{
    AlignItems, Dimension, Display, JustifyContent, LayoutTree, Size, Style,
};
use bexos_flatland_text::{TextEngine, TextStyle};
use bexos_graphics::{Damage, Error, Surface, resolved::Snapshot, scene::Graph};
use bexos_userspace::Channel;
pub struct Desktop {
    pub profile_gpu: bool,
    pub software_vulkan_timeout_ms: u32,
    pub surface: Surface,
    pub presented: bool,
    pixels: Vec<u8>,
    snapshot: Snapshot,
    #[cfg(bexos_guest)]
    text: std::sync::Arc<bexos_flatland_text::Layout<[u8; 4]>>,
}
impl Desktop {
    pub fn new(target: Surface, font_provider: Option<Channel>) -> Result<Self, Error> {
        let config = DesktopConfig::decode(include_bytes!(env!("DESKTOP_CONFIG")))?;
        let [left, top, right, bottom] = config.insets;
        let width = target.width.saturating_sub(left + right).min(480).max(1);
        let available_height = target.height.saturating_sub(top + bottom).max(1);
        let (mut engine, metrics) = text_engine(font_provider)?;
        let mut styles = crate::styling::resolver(target, &config, metrics)?;
        styles
            .resolve(&[
                crate::styling::element(1, None, "desktop", "", ""),
                crate::styling::element(2, Some(1), "label", "", ""),
            ])
            .map_err(|_| Error::Invalid)?;
        let computed = styles.get(2).ok_or(Error::Invalid)?;
        let font_size = computed
            .get_font()
            .clone_font_size()
            .computed_size()
            .px()
            .clamp(8., 96.);
        let color = computed
            .get_inherited_text()
            .clone_color()
            .to_nscolor()
            .to_le_bytes();
        bexos_userspace::log("scened: fontd desktop fonts registered\n");
        let text = engine
            .shape(
                config.text,
                TextStyle {
                    size: font_size,
                    width: width as f32,
                    color,
                    ..Default::default()
                },
            )
            .map_err(|_| Error::Invalid)?;
        bexos_userspace::log("scened: desktop text shaped\n");
        let height = (text.height().ceil() as u32 + 4).min(available_height);
        let surface = Surface {
            width,
            height,
            stride: width * 4,
            format: target.format,
        };
        let mut raster = Rasterizer::new(surface)?;
        raster.text(&text, 0., 0.)?;
        bexos_userspace::log("scened: desktop text rasterized\n");
        let mut pixels = vec![0; surface.validate(u64::MAX)?];
        raster.copy_to(&mut pixels)?;
        let mut layout = LayoutTree::default();
        layout.create(
            1,
            Style {
                display: Display::Flex,
                size: Size {
                    width: Dimension::length(
                        target.width.saturating_sub(left + right).max(1) as f32
                    ),
                    height: Dimension::length(available_height as f32),
                },
                align_items: Some(AlignItems::CENTER),
                justify_content: Some(JustifyContent::CENTER),
                ..Default::default()
            },
        )?;
        layout.create(
            2,
            Style {
                size: Size {
                    width: Dimension::length(width as f32),
                    height: Dimension::length(height as f32),
                },
                ..Default::default()
            },
        )?;
        layout.set_children(1, &[2])?;
        layout.compute(1, target.width as f32, target.height as f32)?;
        let bounds = layout.bounds(2)?;
        let mut graph = Graph::default();
        graph.create(1)?;
        graph.root = Some(1);
        let node = graph.node(1)?;
        node.translation = (
            (left as f32 + bounds.x) as i32,
            (top as f32 + bounds.y) as i32,
        );
        node.content = Some((1, surface));
        let mut snapshot = Snapshot::default();
        snapshot.compile(&graph, target)?;
        Ok(Self {
            profile_gpu: config.profile_gpu,
            software_vulkan_timeout_ms: config.software_vulkan_timeout_ms,
            surface: target,
            presented: false,
            pixels,
            snapshot,
            #[cfg(bexos_guest)]
            text,
        })
    }
    #[cfg(bexos_guest)]
    pub fn append_gpu(&self, scene: &mut bexos_flatland_render::vello::Scene) {
        if let Some(item) = self.snapshot.items.first() {
            bexos_flatland_render::scene::append_text(
                scene,
                &self.text,
                item.transform.x,
                item.transform.y,
            );
        }
    }
    pub fn compose(
        &self,
        surface: Surface,
        output: &mut [u8],
        damage: Damage,
    ) -> Result<(), Error> {
        bexos_graphics::composition::composite(&self.snapshot, surface, output, damage, |_| {
            Some(self.pixels.as_slice())
        })
    }
}

fn text_engine(
    font_provider: Option<Channel>,
) -> Result<(TextEngine, bexos_flatland_text::metrics::Metrics), Error> {
    let mut engine = TextEngine::default();
    #[cfg(bexos_guest)]
    {
        let provider = font_provider.ok_or(Error::Invalid)?;
        let mut client = bexos_font_client::Client::new(provider.0);
        let latin = client
            .resolve(bexos_font_client::Request::sans("Inter"))
            .map_err(|_| Error::Invalid)?;
        let metrics = bexos_flatland_text::metrics::Metrics::from_font(latin.as_ref().as_ref())
            .map_err(|_| Error::Invalid)?;
        engine
            .register_shared_font(latin)
            .map_err(|_| Error::Invalid)?;
        for script in ["Arab", "Deva"] {
            for font in client.fallback(script).map_err(|_| Error::Invalid)? {
                engine
                    .register_shared_font(font)
                    .map_err(|_| Error::Invalid)?;
            }
        }
        Ok((engine, metrics))
    }
    #[cfg(not(bexos_guest))]
    {
        let _ = font_provider;
        let latin = include_bytes!(env!("INTER")).as_slice();
        let metrics =
            bexos_flatland_text::metrics::Metrics::from_font(latin).map_err(|_| Error::Invalid)?;
        for font in [
            latin,
            include_bytes!(env!("NOTO_ARABIC")).as_slice(),
            include_bytes!(env!("NOTO_DEVANAGARI")).as_slice(),
        ] {
            engine
                .register_font(font.to_vec())
                .map_err(|_| Error::Invalid)?;
        }
        Ok((engine, metrics))
    }
}
