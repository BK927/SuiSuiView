use super::acnet_manifest::VARIANTS;
use super::{align_to, GpuUpscaleOutput, TEXTURE_FORMAT};
use crate::core::gpu_effect::color_image_to_rgba;
use crate::core::state::WgpuUpscaleMethod;
use egui::ColorImage;
use std::borrow::Cow;
use std::sync::mpsc;
use std::time::Instant;
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

pub(super) struct AcnetBench {
    bind_group_layout: wgpu::BindGroupLayout,
    variants: Vec<AcnetVariantBench>,
}

struct AcnetVariantBench {
    method: WgpuUpscaleMethod,
    name: &'static str,
    entry_points: &'static [&'static str],
    body_blocks: usize,
    pipelines: Vec<wgpu::ComputePipeline>,
}

impl AcnetBench {
    pub(super) async fn try_new(device: &wgpu::Device) -> Option<Self> {
        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let bench = Self::new(device);
        match device.pop_error_scope().await {
            Some(error) => {
                eprintln!("ACNet luma bench candidates disabled: {error}");
                None
            }
            None => Some(bench),
        }
    }

    fn new(device: &wgpu::Device) -> Self {
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("suisuiview-acnet-f8b4-luma-bind-group-layout"),
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
            label: Some("suisuiview-acnet-f8b4-luma-pipeline-layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        let variants = VARIANTS
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
                AcnetVariantBench {
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

    pub(super) fn apply(
        &self,
        method: WgpuUpscaleMethod,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        image: &ColorImage,
        output_size: [usize; 2],
    ) -> Result<GpuUpscaleOutput, String> {
        let variant = self
            .variants
            .iter()
            .find(|variant| variant.method == method)
            .ok_or_else(|| format!("{} GPU pipelines unavailable", method.label()))?;
        let [source_width, source_height] = image.size;
        let [output_width, output_height] = output_size;
        let exact_width = source_width.saturating_mul(2);
        let exact_height = source_height.saturating_mul(2);
        if output_width > exact_width
            || output_height > exact_height
            || exact_width - output_width > 1
            || exact_height - output_height > 1
        {
            return Err(format!(
                "{} requires 2x output or a one-pixel crop, got {source_width}x{source_height} -> {output_width}x{output_height}",
                variant.name
            ));
        }

        let started = Instant::now();
        let source_bytes = color_image_to_rgba(image);
        let source_extent = wgpu::Extent3d {
            width: source_width as u32,
            height: source_height as u32,
            depth_or_array_layers: 1,
        };
        let output_extent = wgpu::Extent3d {
            width: output_width as u32,
            height: output_height as u32,
            depth_or_array_layers: 1,
        };

        let source_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("suisuiview-acnet-source"),
            size: source_extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: TEXTURE_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &source_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &source_bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some((source_width * 4) as u32),
                rows_per_image: Some(source_height as u32),
            },
            source_extent,
        );

        let tmp1_0 = create_feature_texture(device, source_extent, "acnet-tmp1-0");
        let tmp1_1 = create_feature_texture(device, source_extent, "acnet-tmp1-1");
        let tmp2_0 = create_feature_texture(device, source_extent, "acnet-tmp2-0");
        let tmp2_1 = create_feature_texture(device, source_extent, "acnet-tmp2-1");
        let output_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("suisuiview-acnet-output"),
            size: output_extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: TEXTURE_FORMAT,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });

        let source_view = source_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let tmp1_0_view = tmp1_0.create_view(&wgpu::TextureViewDescriptor::default());
        let tmp1_1_view = tmp1_1.create_view(&wgpu::TextureViewDescriptor::default());
        let tmp2_0_view = tmp2_0.create_view(&wgpu::TextureViewDescriptor::default());
        let tmp2_1_view = tmp2_1.create_view(&wgpu::TextureViewDescriptor::default());
        let output_view = output_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let params = AcnetParams {
            source_width: source_width as u32,
            source_height: source_height as u32,
            output_width: output_width as u32,
            output_height: output_height as u32,
            _pad0: 0,
            _pad1: 0,
            _pad2: 0,
            _pad3: 0,
        };
        let params_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("suisuiview-acnet-params"),
            contents: bytemuck::bytes_of(&params),
            usage: wgpu::BufferUsages::UNIFORM,
        });

        let padded_bytes_per_row = align_to(
            (output_width * 4) as u32,
            wgpu::COPY_BYTES_PER_ROW_ALIGNMENT,
        );
        let output_buffer_size = padded_bytes_per_row as u64 * output_height as u64;
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("suisuiview-acnet-readback"),
            size: output_buffer_size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("suisuiview-acnet-encoder"),
        });
        let source_groups = WorkSizes {
            x: source_width as u32,
            y: source_height as u32,
        };
        let output_groups = WorkSizes {
            x: output_width as u32,
            y: output_height as u32,
        };

        {
            let mut pass_resources = PassResources {
                device,
                encoder: &mut encoder,
                pipelines: &variant.pipelines,
                entry_points: variant.entry_points,
                params_buffer: &params_buffer,
            };

            self.run_pass(
                &mut pass_resources,
                0,
                Views {
                    source: &source_view,
                    input0: &source_view,
                    input1: &source_view,
                    out: &tmp1_0_view,
                    final_out: &output_view,
                },
                source_groups,
            );
            self.run_pass(
                &mut pass_resources,
                1,
                Views {
                    source: &source_view,
                    input0: &source_view,
                    input1: &source_view,
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
                        source: &source_view,
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
                        source: &source_view,
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
                    source: &source_view,
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
                    source: &source_view,
                    input0: upscale_out,
                    input1: upscale_out,
                    out: pixel_shuffle_scratch,
                    final_out: &output_view,
                },
                output_groups,
            );
        }

        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &output_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_bytes_per_row),
                    rows_per_image: Some(output_height as u32),
                },
            },
            output_extent,
        );

        queue.submit(Some(encoder.finish()));

        let slice = readback.slice(..);
        let (sender, receiver) = mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result.map_err(|error| error.to_string()));
        });
        device
            .poll(wgpu::PollType::Wait)
            .map_err(|error| format!("wgpu poll failed: {error}"))?;
        receiver
            .recv()
            .map_err(|error| format!("wgpu readback channel failed: {error}"))?
            .map_err(|error| format!("wgpu readback failed: {error}"))?;
        let elapsed = started.elapsed();

        let mapped = slice.get_mapped_range();
        let mut pixels = vec![0_u8; output_width * output_height * 4];
        for y in 0..output_height {
            let src_offset = y * padded_bytes_per_row as usize;
            let dst_offset = y * output_width * 4;
            pixels[dst_offset..dst_offset + output_width * 4]
                .copy_from_slice(&mapped[src_offset..src_offset + output_width * 4]);
        }
        drop(mapped);
        readback.unmap();

        Ok(GpuUpscaleOutput {
            image: ColorImage::from_rgba_unmultiplied([output_width, output_height], &pixels),
            elapsed,
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
