//! Persistent Dual Kawase targets and bindings. Construction belongs outside
//! the frame loop. Encoding repairs only the requested output and its halos.
use bexos_flatland::{Damage, Error, kawase::Pyramid};
use wgpu::*;
struct Target {
    texture: Texture,
    view: TextureView,
}
pub struct DualKawase {
    pyramid: Pyramid,
    targets: Vec<Target>,
    bindings: Vec<BindGroup>,
    down: RenderPipeline,
    up: RenderPipeline,
}
impl DualKawase {
    /// Source storage must remain the same between frames. Recreate on resize
    /// or source replacement. Sampling uses premultiplied unorm channel values.
    pub fn new(device: &Device, source: &Texture, levels: usize) -> Result<Self, Error> {
        if !matches!(
            source.format(),
            TextureFormat::Rgba8Unorm | TextureFormat::Bgra8Unorm
        ) || source.dimension() != TextureDimension::D2
            || source.depth_or_array_layers() != 1
            || source.sample_count() != 1
            || !source.usage().contains(TextureUsages::TEXTURE_BINDING)
        {
            return Err(Error::Invalid);
        }
        let pyramid = Pyramid::new(source.width(), source.height(), levels)?;
        let shader = device.create_shader_module(ShaderModuleDescriptor {
            label: Some("Flatland Dual Kawase"),
            source: ShaderSource::Wgsl(include_str!("kawase.wgsl").into()),
        });
        let layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("Kawase bindings"),
            entries: &[
                BindGroupLayoutEntry {
                    binding: 0,
                    visibility: ShaderStages::FRAGMENT,
                    ty: BindingType::Texture {
                        sample_type: TextureSampleType::Float { filterable: true },
                        view_dimension: TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                BindGroupLayoutEntry {
                    binding: 1,
                    visibility: ShaderStages::FRAGMENT,
                    ty: BindingType::Sampler(SamplerBindingType::Filtering),
                    count: None,
                },
                BindGroupLayoutEntry {
                    binding: 2,
                    visibility: ShaderStages::FRAGMENT,
                    ty: BindingType::Buffer {
                        ty: BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: BufferSize::new(16),
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("Kawase pipeline layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = |entry| {
            device.create_render_pipeline(&RenderPipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipeline_layout),
                vertex: VertexState {
                    module: &shader,
                    entry_point: Some("vertex"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: PrimitiveState::default(),
                depth_stencil: None,
                multisample: MultisampleState::default(),
                fragment: Some(FragmentState {
                    module: &shader,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    targets: &[Some(ColorTargetState {
                        format: TextureFormat::Rgba8Unorm,
                        blend: None,
                        write_mask: ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let down = pipeline("downsample");
        let up = pipeline("upsample");
        let targets: Vec<_> = pyramid
            .sizes()
            .iter()
            .map(|&(width, height)| {
                let texture = device.create_texture(&TextureDescriptor {
                    label: Some("Kawase reusable scratch"),
                    size: Extent3d {
                        width,
                        height,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: TextureDimension::D2,
                    format: TextureFormat::Rgba8Unorm,
                    usage: TextureUsages::RENDER_ATTACHMENT
                        | TextureUsages::TEXTURE_BINDING
                        | TextureUsages::COPY_SRC,
                    view_formats: &[],
                });
                let view = texture.create_view(&Default::default());
                Target { texture, view }
            })
            .collect();
        let source_view = source.create_view(&TextureViewDescriptor {
            mip_level_count: Some(1),
            ..Default::default()
        });
        let sampler = device.create_sampler(&SamplerDescriptor {
            label: Some("Kawase linear clamp"),
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            ..Default::default()
        });
        let full = Damage {
            x: 0,
            y: 0,
            width: source.width(),
            height: source.height(),
        };
        let bindings = pyramid
            .plan(full)?
            .passes()
            .iter()
            .enumerate()
            .map(|(index, pass)| {
                let (width, height) = pyramid.sizes()[pass.target];
                let dimensions = device.create_buffer(&BufferDescriptor {
                    label: Some("Kawase target dimensions"),
                    size: 16,
                    usage: BufferUsages::UNIFORM,
                    mapped_at_creation: true,
                });
                {
                    let mut bytes = [0u8; 16];
                    for (chunk, value) in
                        bytes
                            .chunks_exact_mut(4)
                            .zip([width as f32, height as f32, 0., 0.])
                    {
                        chunk.copy_from_slice(&value.to_le_bytes());
                    }
                    dimensions
                        .slice(..)
                        .get_mapped_range_mut()
                        .copy_from_slice(&bytes);
                }
                dimensions.unmap();
                device.create_bind_group(&BindGroupDescriptor {
                    label: Some("Kawase cached pass"),
                    layout: &layout,
                    entries: &[
                        BindGroupEntry {
                            binding: 0,
                            resource: BindingResource::TextureView(if index == 0 {
                                &source_view
                            } else {
                                &targets[pass.source].view
                            }),
                        },
                        BindGroupEntry {
                            binding: 1,
                            resource: BindingResource::Sampler(&sampler),
                        },
                        BindGroupEntry {
                            binding: 2,
                            resource: dimensions.as_entire_binding(),
                        },
                    ],
                })
            })
            .collect();
        Ok(Self {
            pyramid,
            targets,
            bindings,
            down,
            up,
        })
    }
    pub fn output(&self) -> &Texture {
        &self.targets[0].texture
    }
    pub fn source_damage(&self, output: Damage) -> Result<Damage, Error> {
        Ok(self.pyramid.plan(output)?.source_damage)
    }
    /// Only `output` is valid after encoding. The caller propagates backdrop
    /// damage and composites that region from the returned reusable texture.
    pub fn encode(&self, encoder: &mut CommandEncoder, output: Damage) -> Result<(), Error> {
        let plan = self.pyramid.plan(output)?;
        for (index, pass) in plan.passes().iter().enumerate() {
            let attachments = [Some(RenderPassColorAttachment {
                view: &self.targets[pass.target].view,
                depth_slice: None,
                resolve_target: None,
                ops: Operations {
                    load: LoadOp::Load,
                    store: StoreOp::Store,
                },
            })];
            let mut render = encoder.begin_render_pass(&RenderPassDescriptor {
                label: Some("Kawase damaged region"),
                color_attachments: &attachments,
                ..Default::default()
            });
            render.set_pipeline(if pass.downsample {
                &self.down
            } else {
                &self.up
            });
            render.set_bind_group(0, &self.bindings[index], &[]);
            let r = pass.damage;
            render.set_scissor_rect(r.x, r.y, r.width, r.height);
            render.draw(0..3, 0..1);
        }
        Ok(())
    }
}
