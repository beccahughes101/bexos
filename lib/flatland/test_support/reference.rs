//! Common CPU/Vulkan reference: opacity, fractional transforms, clipping and
//! rounded content. All surfaces use premultiplied RGBA with explicit IDs.
use bexos_flatland::{
    Format, Surface, composition, effects::Effects, resolved::Snapshot, scene::Graph,
};
pub struct Reference {
    pub target: Surface,
    pub snapshot: Snapshot,
    pub images: Vec<(u64, Surface, Vec<u8>)>,
}
impl Reference {
    pub fn new() -> Self {
        let target = Surface {
            width: 128,
            height: 96,
            stride: 512,
            format: Format::Rgba,
        };
        let mut graph = Graph::default();
        graph.create(1).unwrap();
        graph.root = Some(1);
        let mut images = Vec::new();
        for (id, width, height, x, y, color) in [
            (2, 128, 96, 0, 0, [28, 18, 14, 255]),
            (3, 32, 24, 8, 10, [48, 80, 24, 128]),
            (4, 48, 32, 60, 42, [210, 60, 100, 255]),
        ] {
            graph.create(id).unwrap();
            graph.attach(1, id).unwrap();
            let surface = Surface {
                width,
                height,
                stride: width * 4,
                format: Format::Rgba,
            };
            let node = graph.node(id).unwrap();
            node.content = Some((id, surface));
            node.translation = (x, y);
            if id == 3 {
                node.scale = (1.5, 1.5);
                node.clip = Some((28, 24));
                node.opacity = 0.65;
            }
            if id == 4 {
                node.effects = Effects {
                    corner_radius: 8.,
                    backdrop_radius: 0,
                };
            }
            images.push((id, surface, color.repeat((width * height) as usize)));
        }
        let mut snapshot = Snapshot::default();
        snapshot.compile(&graph, target).unwrap();
        Self {
            target,
            snapshot,
            images,
        }
    }
    pub fn cpu(&self) -> Vec<u8> {
        let mut pixels = vec![0; (self.target.stride * self.target.height) as usize];
        composition::composite(
            &self.snapshot,
            self.target,
            &mut pixels,
            self.target.full(),
            |id| {
                self.images
                    .iter()
                    .find(|i| i.0 == id)
                    .map(|i| i.2.as_slice())
            },
        )
        .unwrap();
        pixels
    }
    /// Compare interiors within 3/255 per channel. A one-pixel neighborhood
    /// around CPU discontinuities excludes the hard-edge/area-AA difference.
    /// At least 85% of the complete frame must remain eligible for comparison.
    pub fn compare(&self, gpu: &[u8]) -> Result<usize, String> {
        let cpu = self.cpu();
        if gpu.len() != cpu.len() {
            return Err("reference extent mismatch".into());
        }
        let width = self.target.width as usize;
        let height = self.target.height as usize;
        let mut checked = 0;
        for y in 1..height - 1 {
            for x in 1..width - 1 {
                let i = (y * width + x) * 4;
                if (y - 1..=y + 1).any(|v| {
                    (x - 1..=x + 1)
                        .any(|u| cpu[(v * width + u) * 4..(v * width + u) * 4 + 4] != cpu[i..i + 4])
                }) {
                    continue;
                }
                if gpu[i..i + 4]
                    .iter()
                    .zip(&cpu[i..i + 4])
                    .any(|(a, b)| a.abs_diff(*b) > 3)
                {
                    return Err(format!(
                        "reference pixel ({x},{y}): GPU {:?}, CPU {:?}",
                        &gpu[i..i + 4],
                        &cpu[i..i + 4]
                    ));
                }
                checked += 1;
            }
        }
        if checked * 100 < width * height * 85 {
            return Err("reference coverage below 85%".into());
        }
        Ok(checked)
    }
}
impl Default for Reference {
    fn default() -> Self {
        Self::new()
    }
}
