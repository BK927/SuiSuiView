use super::{RealtimeSrOutput, TEXTURE_FORMAT};
use crate::core::state::WgpuUpscaleMethod;
use std::borrow::Cow;
use wgpu::util::DeviceExt;

const FEATURE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct AcnetParams {
    source_width: u32,
    source_height: u32,
    output_width: u32,
    output_height: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
    _pad3: u32,
}

pub(super) struct AcnetRenderer {
    bind_group_layout: wgpu::BindGroupLayout,
    variants: Vec<AcnetVariant>,
}

struct AcnetVariant {
    method: WgpuUpscaleMethod,
    name: &'static str,
    entry_points: &'static [&'static str],
    body_blocks: usize,
    pipelines: Vec<wgpu::ComputePipeline>,
}

impl AcnetRenderer {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("suisuiview-realtime-acnet-bind-group-layout"),
            entries: &[
                texture_entry(0),
                texture_entry(1),
                texture_entry(2),
                storage_entry(3, FEATURE_FORMAT),
                storage_entry(4, TEXTURE_FORMAT),
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("suisuiview-realtime-acnet-pipeline-layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });
        let variants = ACNET_VARIANTS
            .iter()
            .map(|variant| {
                let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some(variant.name),
                    source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(variant.shader)),
                });
                let pipelines = variant
                    .entry_points
                    .iter()
                    .map(|entry_point| {
                        device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                            label: Some(entry_point),
                            layout: Some(&pipeline_layout),
                            module: &shader,
                            entry_point: Some(entry_point),
                            compilation_options: Default::default(),
                            cache: None,
                        })
                    })
                    .collect();
                AcnetVariant {
                    method: variant.method,
                    name: variant.name,
                    entry_points: variant.entry_points,
                    body_blocks: variant.body_blocks,
                    pipelines,
                }
            })
            .collect();
        Self {
            bind_group_layout,
            variants,
        }
    }

    pub(super) fn render(
        &self,
        method: WgpuUpscaleMethod,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        source_view: &wgpu::TextureView,
        source_size: [usize; 2],
    ) -> Option<RealtimeSrOutput> {
        let variant = self
            .variants
            .iter()
            .find(|variant| variant.method == method)?;
        let output_size = [
            source_size[0].saturating_mul(2),
            source_size[1].saturating_mul(2),
        ];
        let source_extent = extent_for_size(source_size);
        let output_extent = extent_for_size(output_size);
        let tmp1_0 =
            create_feature_texture(device, source_extent, "suisuiview-realtime-acnet-tmp1-0");
        let tmp1_1 =
            create_feature_texture(device, source_extent, "suisuiview-realtime-acnet-tmp1-1");
        let tmp2_0 =
            create_feature_texture(device, source_extent, "suisuiview-realtime-acnet-tmp2-0");
        let tmp2_1 =
            create_feature_texture(device, source_extent, "suisuiview-realtime-acnet-tmp2-1");
        let tmp1_0_view = tmp1_0.create_view(&wgpu::TextureViewDescriptor::default());
        let tmp1_1_view = tmp1_1.create_view(&wgpu::TextureViewDescriptor::default());
        let tmp2_0_view = tmp2_0.create_view(&wgpu::TextureViewDescriptor::default());
        let tmp2_1_view = tmp2_1.create_view(&wgpu::TextureViewDescriptor::default());
        let output_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(variant.name),
            size: output_extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: TEXTURE_FORMAT,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let output_view = output_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let params = AcnetParams {
            source_width: source_size[0] as u32,
            source_height: source_size[1] as u32,
            output_width: output_size[0] as u32,
            output_height: output_size[1] as u32,
            _pad0: 0,
            _pad1: 0,
            _pad2: 0,
            _pad3: 0,
        };
        let params_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("suisuiview-realtime-acnet-params"),
            contents: bytemuck::bytes_of(&params),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let source_groups = WorkSizes {
            x: source_size[0] as u32,
            y: source_size[1] as u32,
        };
        let output_groups = WorkSizes {
            x: output_size[0] as u32,
            y: output_size[1] as u32,
        };
        let mut pass_resources = PassResources {
            device,
            encoder,
            pipelines: &variant.pipelines,
            entry_points: variant.entry_points,
            params_buffer: &params_buffer,
        };

        self.run_pass(
            &mut pass_resources,
            0,
            Views {
                source: source_view,
                input0: source_view,
                input1: source_view,
                out: &tmp1_0_view,
                final_out: &output_view,
            },
            source_groups,
        );
        self.run_pass(
            &mut pass_resources,
            1,
            Views {
                source: source_view,
                input0: source_view,
                input1: source_view,
                out: &tmp1_1_view,
                final_out: &output_view,
            },
            source_groups,
        );

        let mut input0 = &tmp1_0_view;
        let mut input1 = &tmp1_1_view;
        for block in 0..variant.body_blocks {
            let pipeline_index = 2 + block * 2;
            let (out0, out1) = if block % 2 == 0 {
                (&tmp2_0_view, &tmp2_1_view)
            } else {
                (&tmp1_0_view, &tmp1_1_view)
            };
            self.run_pass(
                &mut pass_resources,
                pipeline_index,
                Views {
                    source: source_view,
                    input0,
                    input1,
                    out: out0,
                    final_out: &output_view,
                },
                source_groups,
            );
            self.run_pass(
                &mut pass_resources,
                pipeline_index + 1,
                Views {
                    source: source_view,
                    input0,
                    input1,
                    out: out1,
                    final_out: &output_view,
                },
                source_groups,
            );
            input0 = out0;
            input1 = out1;
        }

        let upscale_pipeline_index = 2 + variant.body_blocks * 2;
        let upscale_out = if variant.body_blocks % 2 == 0 {
            &tmp2_0_view
        } else {
            &tmp1_0_view
        };
        let pixel_shuffle_scratch = if variant.body_blocks % 2 == 0 {
            &tmp2_1_view
        } else {
            &tmp1_1_view
        };
        self.run_pass(
            &mut pass_resources,
            upscale_pipeline_index,
            Views {
                source: source_view,
                input0,
                input1,
                out: upscale_out,
                final_out: &output_view,
            },
            source_groups,
        );
        self.run_pass(
            &mut pass_resources,
            upscale_pipeline_index + 1,
            Views {
                source: source_view,
                input0: upscale_out,
                input1: upscale_out,
                out: pixel_shuffle_scratch,
                final_out: &output_view,
            },
            output_groups,
        );

        Some(RealtimeSrOutput {
            texture: output_texture,
            view: output_view,
            size: output_size,
            byte_size: output_size[0]
                .saturating_mul(output_size[1])
                .saturating_mul(4),
        })
    }

    fn run_pass(
        &self,
        pass_resources: &mut PassResources<'_>,
        pipeline_index: usize,
        views: Views<'_>,
        groups: WorkSizes,
    ) {
        let bind_group = pass_resources
            .device
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(pass_resources.entry_points[pipeline_index]),
                layout: &self.bind_group_layout,
                entries: &[
                    texture_binding(0, views.source),
                    texture_binding(1, views.input0),
                    texture_binding(2, views.input1),
                    storage_binding(3, views.out),
                    storage_binding(4, views.final_out),
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: pass_resources.params_buffer.as_entire_binding(),
                    },
                ],
            });
        let mut pass = pass_resources
            .encoder
            .begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some(pass_resources.entry_points[pipeline_index]),
                timestamp_writes: None,
            });
        pass.set_pipeline(&pass_resources.pipelines[pipeline_index]);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.dispatch_workgroups(groups.x.div_ceil(16), groups.y.div_ceil(16), 1);
    }
}

struct PassResources<'a> {
    device: &'a wgpu::Device,
    encoder: &'a mut wgpu::CommandEncoder,
    pipelines: &'a [wgpu::ComputePipeline],
    entry_points: &'static [&'static str],
    params_buffer: &'a wgpu::Buffer,
}

struct Views<'a> {
    source: &'a wgpu::TextureView,
    input0: &'a wgpu::TextureView,
    input1: &'a wgpu::TextureView,
    out: &'a wgpu::TextureView,
    final_out: &'a wgpu::TextureView,
}

#[derive(Clone, Copy)]
struct WorkSizes {
    x: u32,
    y: u32,
}

struct AcnetVariantSource {
    method: WgpuUpscaleMethod,
    name: &'static str,
    shader: &'static str,
    entry_points: &'static [&'static str],
    body_blocks: usize,
}

const ACNET_VARIANTS: [AcnetVariantSource; 4] = [
    AcnetVariantSource {
        method: WgpuUpscaleMethod::WgslAcnetF8B4Luma,
        name: "ACNet F8B4 Luma",
        shader: include_str!("../../core/acnet_f8b4_luma.wgsl"),
        entry_points: &F8B4_ENTRY_POINTS,
        body_blocks: 4,
    },
    AcnetVariantSource {
        method: WgpuUpscaleMethod::WgslAcnetF8B4BoxLuma,
        name: "ACNet F8B4 Box Luma",
        shader: include_str!("../../core/acnet_f8b4_box_luma.wgsl"),
        entry_points: &F8B4_ENTRY_POINTS,
        body_blocks: 4,
    },
    AcnetVariantSource {
        method: WgpuUpscaleMethod::WgslAcnetF8B4HdnLuma,
        name: "ACNet F8B4 HDN Luma",
        shader: include_str!("../../core/acnet_f8b4_hdn_luma.wgsl"),
        entry_points: &F8B4_ENTRY_POINTS,
        body_blocks: 4,
    },
    AcnetVariantSource {
        method: WgpuUpscaleMethod::WgslAcnetF8B4BoxHdnLuma,
        name: "ACNet F8B4 Box HDN Luma",
        shader: include_str!("../../core/acnet_f8b4_box_hdn_luma.wgsl"),
        entry_points: &F8B4_ENTRY_POINTS,
        body_blocks: 4,
    },
];

const F8B4_ENTRY_POINTS: [&str; 12] = [
    "acnet_head_conv_1x8x3x3_part_0",
    "acnet_head_conv_1x8x3x3_part_1",
    "acnet_body_block_1_conv_8x8x3x3_part_0",
    "acnet_body_block_1_conv_8x8x3x3_part_1",
    "acnet_body_block_2_conv_8x8x3x3_part_0",
    "acnet_body_block_2_conv_8x8x3x3_part_1",
    "acnet_body_block_3_conv_8x8x3x3_part_0",
    "acnet_body_block_3_conv_8x8x3x3_part_1",
    "acnet_body_block_4_conv_8x8x3x3_part_0",
    "acnet_body_block_4_conv_8x8x3x3_part_1",
    "acnet_upscale_conv_8x4x3x3_part_0",
    "acnet_pixel_shuffle",
];

fn texture_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: false },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn storage_entry(binding: u32, format: wgpu::TextureFormat) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::StorageTexture {
            access: wgpu::StorageTextureAccess::WriteOnly,
            format,
            view_dimension: wgpu::TextureViewDimension::D2,
        },
        count: None,
    }
}

fn texture_binding(binding: u32, view: &wgpu::TextureView) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: wgpu::BindingResource::TextureView(view),
    }
}

fn storage_binding(binding: u32, view: &wgpu::TextureView) -> wgpu::BindGroupEntry<'_> {
    texture_binding(binding, view)
}

fn create_feature_texture(
    device: &wgpu::Device,
    size: wgpu::Extent3d,
    label: &'static str,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FEATURE_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING,
        view_formats: &[],
    })
}

fn extent_for_size(size: [usize; 2]) -> wgpu::Extent3d {
    wgpu::Extent3d {
        width: size[0] as u32,
        height: size[1] as u32,
        depth_or_array_layers: 1,
    }
}
