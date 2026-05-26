use super::{RealtimeSrOutput, TEXTURE_FORMAT};
use std::borrow::Cow;
use wgpu::util::DeviceExt;

const FEATURE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct ConvParams {
    pass_id: [u32; 4],
}

pub(super) struct Anime4kSRenderer {
    conv_pipeline: wgpu::RenderPipeline,
    final_pipeline: wgpu::RenderPipeline,
    conv_bind_group_layout: wgpu::BindGroupLayout,
    final_bind_group_layout: wgpu::BindGroupLayout,
}

impl Anime4kSRenderer {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("suisuiview-realtime-anime4k-v32-cnn-x2-s-shader"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(include_str!(
                "../../core/anime4k_v32_cnn_x2_s.wgsl"
            ))),
        });
        let conv_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("suisuiview-realtime-anime4k-conv-bind-group-layout"),
                entries: &[
                    texture_entry(0),
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                ],
            });
        let final_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("suisuiview-realtime-anime4k-final-bind-group-layout"),
                entries: &[texture_entry(0), texture_entry(1)],
            });
        let conv_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("suisuiview-realtime-anime4k-conv-pipeline-layout"),
            bind_group_layouts: &[&conv_bind_group_layout],
            push_constant_ranges: &[],
        });
        let final_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("suisuiview-realtime-anime4k-final-pipeline-layout"),
                bind_group_layouts: &[&final_bind_group_layout],
                push_constant_ranges: &[],
            });
        let conv_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("suisuiview-realtime-anime4k-conv-pipeline"),
            layout: Some(&conv_pipeline_layout),
            vertex: vertex_state(&shader),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_conv_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: FEATURE_FORMAT,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });
        let final_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("suisuiview-realtime-anime4k-final-pipeline"),
            layout: Some(&final_pipeline_layout),
            vertex: vertex_state(&shader),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_final_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: TEXTURE_FORMAT,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        Self {
            conv_pipeline,
            final_pipeline,
            conv_bind_group_layout,
            final_bind_group_layout,
        }
    }

    pub(super) fn render(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        source_view: &wgpu::TextureView,
        source_size: [usize; 2],
    ) -> RealtimeSrOutput {
        let source_extent = extent_for_size(source_size);
        let output_size = [
            source_size[0].saturating_mul(2),
            source_size[1].saturating_mul(2),
        ];
        let output_extent = extent_for_size(output_size);
        let feature_a = self.feature_texture(device, source_extent, "feature-a");
        let feature_b = self.feature_texture(device, source_extent, "feature-b");
        let feature_a_view = feature_a.create_view(&wgpu::TextureViewDescriptor::default());
        let feature_b_view = feature_b.create_view(&wgpu::TextureViewDescriptor::default());
        let output_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("suisuiview-realtime-anime4k-output"),
            size: output_extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: TEXTURE_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let output_view = output_texture.create_view(&wgpu::TextureViewDescriptor::default());

        self.render_conv_pass(device, encoder, source_view, &feature_a_view, 0);
        self.render_conv_pass(device, encoder, &feature_a_view, &feature_b_view, 1);
        self.render_conv_pass(device, encoder, &feature_b_view, &feature_a_view, 2);
        self.render_conv_pass(device, encoder, &feature_a_view, &feature_b_view, 3);
        self.render_final_pass(device, encoder, source_view, &feature_b_view, &output_view);

        RealtimeSrOutput {
            texture: output_texture,
            view: output_view,
            size: output_size,
            byte_size: output_size[0]
                .saturating_mul(output_size[1])
                .saturating_mul(4),
        }
    }

    fn feature_texture(
        &self,
        device: &wgpu::Device,
        size: wgpu::Extent3d,
        label_suffix: &str,
    ) -> wgpu::Texture {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some(&format!("suisuiview-realtime-anime4k-{label_suffix}")),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FEATURE_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
    }

    fn render_conv_pass(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        input_view: &wgpu::TextureView,
        output_view: &wgpu::TextureView,
        pass_id: u32,
    ) {
        let params = ConvParams {
            pass_id: [pass_id, 0, 0, 0],
        };
        let params_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("suisuiview-realtime-anime4k-conv-params"),
            contents: bytemuck::bytes_of(&params),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("suisuiview-realtime-anime4k-conv-bind-group"),
            layout: &self.conv_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(input_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: params_buffer.as_entire_binding(),
                },
            ],
        });

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("suisuiview-realtime-anime4k-conv-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: output_view,
                resolve_target: None,
                ops: clear_store_ops(),
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&self.conv_pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.draw(0..3, 0..1);
    }

    fn render_final_pass(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        source_view: &wgpu::TextureView,
        feature_view: &wgpu::TextureView,
        output_view: &wgpu::TextureView,
    ) {
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("suisuiview-realtime-anime4k-final-bind-group"),
            layout: &self.final_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(source_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(feature_view),
                },
            ],
        });

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("suisuiview-realtime-anime4k-final-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: output_view,
                resolve_target: None,
                ops: clear_store_ops(),
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&self.final_pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

fn vertex_state(shader: &wgpu::ShaderModule) -> wgpu::VertexState<'_> {
    wgpu::VertexState {
        module: shader,
        entry_point: Some("vs_main"),
        buffers: &[],
        compilation_options: Default::default(),
    }
}

fn texture_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: false },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn clear_store_ops() -> wgpu::Operations<wgpu::Color> {
    wgpu::Operations {
        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
        store: wgpu::StoreOp::Store,
    }
}

fn extent_for_size(size: [usize; 2]) -> wgpu::Extent3d {
    wgpu::Extent3d {
        width: size[0] as u32,
        height: size[1] as u32,
        depth_or_array_layers: 1,
    }
}
