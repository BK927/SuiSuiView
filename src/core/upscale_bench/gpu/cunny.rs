use super::{align_to, GpuUpscaleOutput, TEXTURE_FORMAT};
use crate::core::gpu_effect::color_image_to_rgba;
use crate::core::state::WgpuUpscaleMethod;
use eframe::egui::ColorImage;
use std::borrow::Cow;
use std::sync::mpsc;
use std::time::Instant;
use wgpu::util::DeviceExt;

const CUNNY_INPUT_SLOTS: usize = 8;
const CUNNY_OUTPUT_SLOTS: usize = 3;
const CUNNY_INTERMEDIATE_CAPACITY: usize = 16;
const DUMMY_READ: usize = CUNNY_INTERMEDIATE_CAPACITY;
const DUMMY_OUT0: usize = DUMMY_READ + 1;
const DUMMY_OUT1: usize = DUMMY_READ + 2;
const DUMMY_OUT2: usize = DUMMY_READ + 3;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct CunnyParams {
    source_width: u32,
    source_height: u32,
    output_width: u32,
    output_height: u32,
}

pub(super) struct CunnyBench {
    bind_group_layout: wgpu::BindGroupLayout,
    variants: Vec<CunnyVariantBench>,
}

struct CunnyVariantBench {
    method: WgpuUpscaleMethod,
    name: &'static str,
    entry_points: &'static [&'static str],
    pass_specs: &'static [CunnyPassSpec],
    pipelines: Vec<wgpu::ComputePipeline>,
}

#[derive(Clone, Copy)]
struct CunnyPassSpec {
    inputs: &'static [usize],
    outputs: &'static [usize],
}

impl CunnyBench {
    pub(super) async fn try_new(
        device: &wgpu::Device,
        method_filter: Option<WgpuUpscaleMethod>,
    ) -> Option<Self> {
        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let bench = Self::new(device, method_filter);
        match device.pop_error_scope().await {
            Some(error) => {
                eprintln!("CuNNy NVL bench candidates disabled: {error}");
                None
            }
            None => Some(bench),
        }
    }

    fn new(device: &wgpu::Device, method_filter: Option<WgpuUpscaleMethod>) -> Self {
        let mut layout_entries = Vec::with_capacity(1 + CUNNY_INPUT_SLOTS + CUNNY_OUTPUT_SLOTS + 2);
        layout_entries.push(texture_entry(0));
        for slot in 0..CUNNY_INPUT_SLOTS {
            layout_entries.push(texture_entry(input_binding(slot)));
        }
        for slot in 0..CUNNY_OUTPUT_SLOTS {
            layout_entries.push(storage_entry(output_binding(slot)));
        }
        layout_entries.push(storage_entry(final_binding()));
        layout_entries.push(wgpu::BindGroupLayoutEntry {
            binding: params_binding(),
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        });
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("suisuiview-cunny-nvl-bind-group-layout"),
            entries: &layout_entries,
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("suisuiview-cunny-nvl-pipeline-layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        let variants = cunny_variant_sources(method_filter)
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
                CunnyVariantBench {
                    method: variant.method,
                    name: variant.name,
                    entry_points: variant.entry_points,
                    pass_specs: variant.pass_specs,
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
            label: Some("suisuiview-cunny-source"),
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

        let intermediates: Vec<wgpu::Texture> = (0..intermediate_count(variant.pass_specs))
            .map(|index| create_intermediate_texture(device, source_extent, index))
            .collect();
        let intermediate_views: Vec<wgpu::TextureView> = intermediates
            .iter()
            .map(|texture| texture.create_view(&wgpu::TextureViewDescriptor::default()))
            .collect();
        let dummy_read_texture = create_intermediate_texture(device, source_extent, DUMMY_READ);
        let dummy_read_view =
            dummy_read_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let dummy_output_textures: Vec<wgpu::Texture> = (0..CUNNY_OUTPUT_SLOTS)
            .map(dummy_output_index)
            .map(|index| create_intermediate_texture(device, source_extent, index))
            .collect();
        let dummy_output_views: Vec<wgpu::TextureView> = dummy_output_textures
            .iter()
            .map(|texture| texture.create_view(&wgpu::TextureViewDescriptor::default()))
            .collect();

        let output_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("suisuiview-cunny-output"),
            size: output_extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: TEXTURE_FORMAT,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let source_view = source_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let output_view = output_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let params = CunnyParams {
            source_width: source_width as u32,
            source_height: source_height as u32,
            output_width: output_width as u32,
            output_height: output_height as u32,
        };
        let params_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("suisuiview-cunny-params"),
            contents: bytemuck::bytes_of(&params),
            usage: wgpu::BufferUsages::UNIFORM,
        });

        let padded_bytes_per_row = align_to(
            (output_width * 4) as u32,
            wgpu::COPY_BYTES_PER_ROW_ALIGNMENT,
        );
        let output_buffer_size = padded_bytes_per_row as u64 * output_height as u64;
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("suisuiview-cunny-readback"),
            size: output_buffer_size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("suisuiview-cunny-encoder"),
        });
        for (index, pass_spec) in variant.pass_specs.iter().enumerate() {
            self.run_pass(
                &mut RunPassCtx {
                    device,
                    encoder: &mut encoder,
                    source_view: &source_view,
                    intermediate_views: &intermediate_views,
                    dummy_read_view: &dummy_read_view,
                    dummy_output_views: &dummy_output_views,
                    output_view: &output_view,
                    params_buffer: &params_buffer,
                    variant,
                },
                index,
                *pass_spec,
                [source_width as u32, source_height as u32],
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
        ctx: &mut RunPassCtx<'_>,
        index: usize,
        pass_spec: CunnyPassSpec,
        size: [u32; 2],
    ) {
        let mut bind_entries = Vec::with_capacity(1 + CUNNY_INPUT_SLOTS + CUNNY_OUTPUT_SLOTS + 2);
        bind_entries.push(texture_binding(0, ctx.source_view));
        for slot in 0..CUNNY_INPUT_SLOTS {
            let input_index = pass_spec.inputs.get(slot).copied().unwrap_or(DUMMY_READ);
            bind_entries.push(texture_binding(
                input_binding(slot),
                intermediate_view(
                    ctx.intermediate_views,
                    ctx.dummy_read_view,
                    ctx.dummy_output_views,
                    input_index,
                ),
            ));
        }
        for slot in 0..CUNNY_OUTPUT_SLOTS {
            let output_index = pass_spec
                .outputs
                .get(slot)
                .copied()
                .unwrap_or_else(|| dummy_output_index(slot));
            bind_entries.push(storage_binding(
                output_binding(slot),
                intermediate_view(
                    ctx.intermediate_views,
                    ctx.dummy_read_view,
                    ctx.dummy_output_views,
                    output_index,
                ),
            ));
        }
        bind_entries.push(storage_binding(final_binding(), ctx.output_view));
        bind_entries.push(wgpu::BindGroupEntry {
            binding: params_binding(),
            resource: ctx.params_buffer.as_entire_binding(),
        });
        let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(ctx.variant.entry_points[index]),
            layout: &self.bind_group_layout,
            entries: &bind_entries,
        });
        let mut pass = ctx
            .encoder
            .begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some(ctx.variant.entry_points[index]),
                timestamp_writes: None,
            });
        pass.set_pipeline(&ctx.variant.pipelines[index]);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.dispatch_workgroups(size[0].div_ceil(8), size[1].div_ceil(8), 1);
    }
}

struct RunPassCtx<'a> {
    device: &'a wgpu::Device,
    encoder: &'a mut wgpu::CommandEncoder,
    source_view: &'a wgpu::TextureView,
    intermediate_views: &'a [wgpu::TextureView],
    dummy_read_view: &'a wgpu::TextureView,
    dummy_output_views: &'a [wgpu::TextureView],
    output_view: &'a wgpu::TextureView,
    params_buffer: &'a wgpu::Buffer,
    variant: &'a CunnyVariantBench,
}

fn intermediate_count(pass_specs: &[CunnyPassSpec]) -> usize {
    pass_specs
        .iter()
        .flat_map(|pass| pass.inputs.iter().chain(pass.outputs).copied())
        .filter(|&index| index < DUMMY_READ)
        .max()
        .map_or(0, |index| index + 1)
}

fn intermediate_view<'a>(
    intermediate_views: &'a [wgpu::TextureView],
    dummy_read_view: &'a wgpu::TextureView,
    dummy_output_views: &'a [wgpu::TextureView],
    index: usize,
) -> &'a wgpu::TextureView {
    if index == DUMMY_READ {
        dummy_read_view
    } else if (DUMMY_OUT0..DUMMY_OUT0 + CUNNY_OUTPUT_SLOTS).contains(&index) {
        &dummy_output_views[index - DUMMY_OUT0]
    } else {
        &intermediate_views[index]
    }
}

struct CunnyVariantSource {
    method: WgpuUpscaleMethod,
    name: &'static str,
    shader: &'static str,
    entry_points: &'static [&'static str],
    pass_specs: &'static [CunnyPassSpec],
}

const CUNNY_VARIANTS: [CunnyVariantSource; 27] = [
    CunnyVariantSource {
        method: WgpuUpscaleMethod::CunnyVeryfastNvl,
        name: "CuNNy veryfast NVL",
        shader: include_str!("../../cunny_veryfast_nvl.wgsl"),
        entry_points: &CUNNY_VERYFAST_NVL_ENTRY_POINTS,
        pass_specs: &CUNNY_VERYFAST_NVL_PASSES,
    },
    CunnyVariantSource {
        method: WgpuUpscaleMethod::CunnyVeryfastSoft,
        name: "CuNNy veryfast SOFT",
        shader: include_str!("../../cunny_veryfast_soft.wgsl"),
        entry_points: &CUNNY_VERYFAST_SOFT_ENTRY_POINTS,
        pass_specs: &CUNNY_VERYFAST_SOFT_PASSES,
    },
    CunnyVariantSource {
        method: WgpuUpscaleMethod::CunnyFasterNvl,
        name: "CuNNy faster NVL",
        shader: include_str!("../../cunny_faster_nvl.wgsl"),
        entry_points: &CUNNY_FASTER_NVL_ENTRY_POINTS,
        pass_specs: &CUNNY_FASTER_NVL_PASSES,
    },
    CunnyVariantSource {
        method: WgpuUpscaleMethod::CunnyFasterSoft,
        name: "CuNNy faster SOFT",
        shader: include_str!("../../cunny_faster_soft.wgsl"),
        entry_points: &CUNNY_FASTER_SOFT_ENTRY_POINTS,
        pass_specs: &CUNNY_FASTER_SOFT_PASSES,
    },
    CunnyVariantSource {
        method: WgpuUpscaleMethod::CunnyFasterDs,
        name: "CuNNy faster DS",
        shader: include_str!("../../cunny_faster_ds.wgsl"),
        entry_points: &CUNNY_FASTER_DS_ENTRY_POINTS,
        pass_specs: &CUNNY_FASTER_DS_PASSES,
    },
    CunnyVariantSource {
        method: WgpuUpscaleMethod::CunnyFastNvl,
        name: "CuNNy fast NVL",
        shader: include_str!("../../cunny_fast_nvl.wgsl"),
        entry_points: &CUNNY_FAST_NVL_ENTRY_POINTS,
        pass_specs: &CUNNY_FAST_NVL_PASSES,
    },
    CunnyVariantSource {
        method: WgpuUpscaleMethod::CunnyFastSoft,
        name: "CuNNy fast SOFT",
        shader: include_str!("../../cunny_fast_soft.wgsl"),
        entry_points: &CUNNY_FAST_SOFT_ENTRY_POINTS,
        pass_specs: &CUNNY_FAST_SOFT_PASSES,
    },
    CunnyVariantSource {
        method: WgpuUpscaleMethod::CunnyFastDs,
        name: "CuNNy fast DS",
        shader: include_str!("../../cunny_fast_ds.wgsl"),
        entry_points: &CUNNY_FAST_DS_ENTRY_POINTS,
        pass_specs: &CUNNY_FAST_DS_PASSES,
    },
    CunnyVariantSource {
        method: WgpuUpscaleMethod::Cunny2x12Soft,
        name: "CuNNy 2x12 SOFT",
        shader: include_str!("../../cunny_2x12_soft.wgsl"),
        entry_points: &CUNNY_2X12_SOFT_ENTRY_POINTS,
        pass_specs: &CUNNY_2X12_MPV_PASSES,
    },
    CunnyVariantSource {
        method: WgpuUpscaleMethod::Cunny2x12Ds,
        name: "CuNNy 2x12 DS",
        shader: include_str!("../../cunny_2x12_ds.wgsl"),
        entry_points: &CUNNY_2X12_DS_ENTRY_POINTS,
        pass_specs: &CUNNY_2X12_MPV_PASSES,
    },
    CunnyVariantSource {
        method: WgpuUpscaleMethod::Cunny3x12Nvl,
        name: "CuNNy 3x12 NVL",
        shader: include_str!("../../cunny_3x12_nvl.wgsl"),
        entry_points: &CUNNY_3X12_NVL_ENTRY_POINTS,
        pass_specs: &CUNNY_3X12_NVL_PASSES,
    },
    CunnyVariantSource {
        method: WgpuUpscaleMethod::Cunny3x12Soft,
        name: "CuNNy 3x12 SOFT",
        shader: include_str!("../../cunny_3x12_soft.wgsl"),
        entry_points: &CUNNY_3X12_SOFT_ENTRY_POINTS,
        pass_specs: &CUNNY_3X12_NVL_PASSES,
    },
    CunnyVariantSource {
        method: WgpuUpscaleMethod::Cunny3x12Ds,
        name: "CuNNy 3x12 DS",
        shader: include_str!("../../cunny_3x12_ds.wgsl"),
        entry_points: &CUNNY_3X12_DS_ENTRY_POINTS,
        pass_specs: &CUNNY_3X12_NVL_PASSES,
    },
    CunnyVariantSource {
        method: WgpuUpscaleMethod::Cunny4x12Nvl,
        name: "CuNNy 4x12 NVL",
        shader: include_str!("../../cunny_4x12_nvl.wgsl"),
        entry_points: &CUNNY_4X12_NVL_ENTRY_POINTS,
        pass_specs: &CUNNY_4X12_NVL_PASSES,
    },
    CunnyVariantSource {
        method: WgpuUpscaleMethod::Cunny4x12Soft,
        name: "CuNNy 4x12 SOFT",
        shader: include_str!("../../cunny_4x12_soft.wgsl"),
        entry_points: &CUNNY_4X12_SOFT_ENTRY_POINTS,
        pass_specs: &CUNNY_4X12_NVL_PASSES,
    },
    CunnyVariantSource {
        method: WgpuUpscaleMethod::Cunny4x12Ds,
        name: "CuNNy 4x12 DS",
        shader: include_str!("../../cunny_4x12_ds.wgsl"),
        entry_points: &CUNNY_4X12_DS_ENTRY_POINTS,
        pass_specs: &CUNNY_4X12_NVL_PASSES,
    },
    CunnyVariantSource {
        method: WgpuUpscaleMethod::Cunny4x16Nvl,
        name: "CuNNy 4x16 NVL",
        shader: include_str!("../../cunny_4x16_nvl.wgsl"),
        entry_points: &CUNNY_4X16_NVL_ENTRY_POINTS,
        pass_specs: &CUNNY_4X16_NVL_PASSES,
    },
    CunnyVariantSource {
        method: WgpuUpscaleMethod::Cunny4x16Soft,
        name: "CuNNy 4x16 SOFT",
        shader: include_str!("../../cunny_4x16_soft.wgsl"),
        entry_points: &CUNNY_4X16_SOFT_ENTRY_POINTS,
        pass_specs: &CUNNY_4X16_NVL_PASSES,
    },
    CunnyVariantSource {
        method: WgpuUpscaleMethod::Cunny4x16Ds,
        name: "CuNNy 4x16 DS",
        shader: include_str!("../../cunny_4x16_ds.wgsl"),
        entry_points: &CUNNY_4X16_DS_ENTRY_POINTS,
        pass_specs: &CUNNY_4X16_NVL_PASSES,
    },
    CunnyVariantSource {
        method: WgpuUpscaleMethod::Cunny4x24Nvl,
        name: "CuNNy 4x24 NVL",
        shader: include_str!("../../cunny_4x24_nvl.wgsl"),
        entry_points: &CUNNY_4X24_NVL_ENTRY_POINTS,
        pass_specs: &CUNNY_4X24_NVL_PASSES,
    },
    CunnyVariantSource {
        method: WgpuUpscaleMethod::Cunny4x24Soft,
        name: "CuNNy 4x24 SOFT",
        shader: include_str!("../../cunny_4x24_soft.wgsl"),
        entry_points: &CUNNY_4X24_SOFT_ENTRY_POINTS,
        pass_specs: &CUNNY_4X24_NVL_PASSES,
    },
    CunnyVariantSource {
        method: WgpuUpscaleMethod::Cunny4x24Ds,
        name: "CuNNy 4x24 DS",
        shader: include_str!("../../cunny_4x24_ds.wgsl"),
        entry_points: &CUNNY_4X24_DS_ENTRY_POINTS,
        pass_specs: &CUNNY_4X24_NVL_PASSES,
    },
    CunnyVariantSource {
        method: WgpuUpscaleMethod::Cunny4x32Nvl,
        name: "CuNNy 4x32 NVL",
        shader: include_str!("../../cunny_4x32_nvl.wgsl"),
        entry_points: &CUNNY_4X32_NVL_ENTRY_POINTS,
        pass_specs: &CUNNY_4X32_NVL_PASSES,
    },
    CunnyVariantSource {
        method: WgpuUpscaleMethod::Cunny4x32Soft,
        name: "CuNNy 4x32 SOFT",
        shader: include_str!("../../cunny_4x32_soft.wgsl"),
        entry_points: &CUNNY_4X32_SOFT_ENTRY_POINTS,
        pass_specs: &CUNNY_4X32_NVL_PASSES,
    },
    CunnyVariantSource {
        method: WgpuUpscaleMethod::Cunny4x32Ds,
        name: "CuNNy 4x32 DS",
        shader: include_str!("../../cunny_4x32_ds.wgsl"),
        entry_points: &CUNNY_4X32_DS_ENTRY_POINTS,
        pass_specs: &CUNNY_4X32_NVL_PASSES,
    },
    CunnyVariantSource {
        method: WgpuUpscaleMethod::Cunny8x32Nvl,
        name: "CuNNy 8x32 NVL",
        shader: include_str!("../../cunny_8x32_nvl.wgsl"),
        entry_points: &CUNNY_8X32_NVL_ENTRY_POINTS,
        pass_specs: &CUNNY_8X32_NVL_PASSES,
    },
    CunnyVariantSource {
        method: WgpuUpscaleMethod::Cunny8x32Ds,
        name: "CuNNy 8x32 DS",
        shader: include_str!("../../cunny_8x32_ds.wgsl"),
        entry_points: &CUNNY_8X32_DS_ENTRY_POINTS,
        pass_specs: &CUNNY_8X32_NVL_PASSES,
    },
];

fn cunny_variant_sources(
    method_filter: Option<WgpuUpscaleMethod>,
) -> impl Iterator<Item = &'static CunnyVariantSource> {
    CUNNY_VARIANTS
        .iter()
        .filter(move |variant| method_filter.map_or(true, |method| variant.method == method))
}

const CUNNY_VERYFAST_NVL_ENTRY_POINTS: [&str; 4] = [
    "cunny_veryfast_nvl_pass_0",
    "cunny_veryfast_nvl_pass_1",
    "cunny_veryfast_nvl_pass_2",
    "cunny_veryfast_nvl_pass_3",
];

const CUNNY_VERYFAST_SOFT_ENTRY_POINTS: [&str; 4] = [
    "cunny_veryfast_soft_pass_0",
    "cunny_veryfast_soft_pass_1",
    "cunny_veryfast_soft_pass_2",
    "cunny_veryfast_soft_pass_3",
];

const CUNNY_FASTER_NVL_ENTRY_POINTS: [&str; 4] = [
    "cunny_faster_nvl_pass_0",
    "cunny_faster_nvl_pass_1",
    "cunny_faster_nvl_pass_2",
    "cunny_faster_nvl_pass_3",
];

const CUNNY_FASTER_SOFT_ENTRY_POINTS: [&str; 4] = [
    "cunny_faster_soft_pass_0",
    "cunny_faster_soft_pass_1",
    "cunny_faster_soft_pass_2",
    "cunny_faster_soft_pass_3",
];

const CUNNY_FASTER_DS_ENTRY_POINTS: [&str; 4] = [
    "cunny_faster_ds_pass_0",
    "cunny_faster_ds_pass_1",
    "cunny_faster_ds_pass_2",
    "cunny_faster_ds_pass_3",
];

const CUNNY_FAST_NVL_ENTRY_POINTS: [&str; 4] = [
    "cunny_fast_nvl_pass_0",
    "cunny_fast_nvl_pass_1",
    "cunny_fast_nvl_pass_2",
    "cunny_fast_nvl_pass_3",
];

const CUNNY_FAST_SOFT_ENTRY_POINTS: [&str; 4] = [
    "cunny_fast_soft_pass_0",
    "cunny_fast_soft_pass_1",
    "cunny_fast_soft_pass_2",
    "cunny_fast_soft_pass_3",
];

const CUNNY_FAST_DS_ENTRY_POINTS: [&str; 4] = [
    "cunny_fast_ds_pass_0",
    "cunny_fast_ds_pass_1",
    "cunny_fast_ds_pass_2",
    "cunny_fast_ds_pass_3",
];

const CUNNY_2X12_SOFT_ENTRY_POINTS: [&str; 4] = [
    "cunny_2x12_soft_pass_0",
    "cunny_2x12_soft_pass_1",
    "cunny_2x12_soft_pass_2",
    "cunny_2x12_soft_pass_3",
];

const CUNNY_2X12_DS_ENTRY_POINTS: [&str; 4] = [
    "cunny_2x12_ds_pass_0",
    "cunny_2x12_ds_pass_1",
    "cunny_2x12_ds_pass_2",
    "cunny_2x12_ds_pass_3",
];

const CUNNY_3X12_NVL_ENTRY_POINTS: [&str; 5] = [
    "cunny_3x12_nvl_pass_0",
    "cunny_3x12_nvl_pass_1",
    "cunny_3x12_nvl_pass_2",
    "cunny_3x12_nvl_pass_3",
    "cunny_3x12_nvl_pass_4",
];

const CUNNY_3X12_SOFT_ENTRY_POINTS: [&str; 5] = [
    "cunny_3x12_soft_pass_0",
    "cunny_3x12_soft_pass_1",
    "cunny_3x12_soft_pass_2",
    "cunny_3x12_soft_pass_3",
    "cunny_3x12_soft_pass_4",
];

const CUNNY_3X12_DS_ENTRY_POINTS: [&str; 5] = [
    "cunny_3x12_ds_pass_0",
    "cunny_3x12_ds_pass_1",
    "cunny_3x12_ds_pass_2",
    "cunny_3x12_ds_pass_3",
    "cunny_3x12_ds_pass_4",
];

const CUNNY_4X12_NVL_ENTRY_POINTS: [&str; 6] = [
    "cunny_4x12_nvl_pass_0",
    "cunny_4x12_nvl_pass_1",
    "cunny_4x12_nvl_pass_2",
    "cunny_4x12_nvl_pass_3",
    "cunny_4x12_nvl_pass_4",
    "cunny_4x12_nvl_pass_5",
];

const CUNNY_4X12_SOFT_ENTRY_POINTS: [&str; 6] = [
    "cunny_4x12_soft_pass_0",
    "cunny_4x12_soft_pass_1",
    "cunny_4x12_soft_pass_2",
    "cunny_4x12_soft_pass_3",
    "cunny_4x12_soft_pass_4",
    "cunny_4x12_soft_pass_5",
];

const CUNNY_4X12_DS_ENTRY_POINTS: [&str; 6] = [
    "cunny_4x12_ds_pass_0",
    "cunny_4x12_ds_pass_1",
    "cunny_4x12_ds_pass_2",
    "cunny_4x12_ds_pass_3",
    "cunny_4x12_ds_pass_4",
    "cunny_4x12_ds_pass_5",
];

const CUNNY_4X16_NVL_ENTRY_POINTS: [&str; 11] = [
    "cunny_4x16_nvl_pass_0_chunk_0",
    "cunny_4x16_nvl_pass_0_chunk_1",
    "cunny_4x16_nvl_pass_1_chunk_0",
    "cunny_4x16_nvl_pass_1_chunk_1",
    "cunny_4x16_nvl_pass_2_chunk_0",
    "cunny_4x16_nvl_pass_2_chunk_1",
    "cunny_4x16_nvl_pass_3_chunk_0",
    "cunny_4x16_nvl_pass_3_chunk_1",
    "cunny_4x16_nvl_pass_4_chunk_0",
    "cunny_4x16_nvl_pass_4_chunk_1",
    "cunny_4x16_nvl_pass_5",
];

const CUNNY_4X16_SOFT_ENTRY_POINTS: [&str; 11] = [
    "cunny_4x16_soft_pass_0_chunk_0",
    "cunny_4x16_soft_pass_0_chunk_1",
    "cunny_4x16_soft_pass_1_chunk_0",
    "cunny_4x16_soft_pass_1_chunk_1",
    "cunny_4x16_soft_pass_2_chunk_0",
    "cunny_4x16_soft_pass_2_chunk_1",
    "cunny_4x16_soft_pass_3_chunk_0",
    "cunny_4x16_soft_pass_3_chunk_1",
    "cunny_4x16_soft_pass_4_chunk_0",
    "cunny_4x16_soft_pass_4_chunk_1",
    "cunny_4x16_soft_pass_5",
];

const CUNNY_4X16_DS_ENTRY_POINTS: [&str; 11] = [
    "cunny_4x16_ds_pass_0_chunk_0",
    "cunny_4x16_ds_pass_0_chunk_1",
    "cunny_4x16_ds_pass_1_chunk_0",
    "cunny_4x16_ds_pass_1_chunk_1",
    "cunny_4x16_ds_pass_2_chunk_0",
    "cunny_4x16_ds_pass_2_chunk_1",
    "cunny_4x16_ds_pass_3_chunk_0",
    "cunny_4x16_ds_pass_3_chunk_1",
    "cunny_4x16_ds_pass_4_chunk_0",
    "cunny_4x16_ds_pass_4_chunk_1",
    "cunny_4x16_ds_pass_5",
];

const CUNNY_4X24_NVL_ENTRY_POINTS: [&str; 11] = [
    "cunny_4x24_nvl_pass_0_chunk_0",
    "cunny_4x24_nvl_pass_0_chunk_1",
    "cunny_4x24_nvl_pass_1_chunk_0",
    "cunny_4x24_nvl_pass_1_chunk_1",
    "cunny_4x24_nvl_pass_2_chunk_0",
    "cunny_4x24_nvl_pass_2_chunk_1",
    "cunny_4x24_nvl_pass_3_chunk_0",
    "cunny_4x24_nvl_pass_3_chunk_1",
    "cunny_4x24_nvl_pass_4_chunk_0",
    "cunny_4x24_nvl_pass_4_chunk_1",
    "cunny_4x24_nvl_pass_5",
];

const CUNNY_4X24_SOFT_ENTRY_POINTS: [&str; 11] = [
    "cunny_4x24_soft_pass_0_chunk_0",
    "cunny_4x24_soft_pass_0_chunk_1",
    "cunny_4x24_soft_pass_1_chunk_0",
    "cunny_4x24_soft_pass_1_chunk_1",
    "cunny_4x24_soft_pass_2_chunk_0",
    "cunny_4x24_soft_pass_2_chunk_1",
    "cunny_4x24_soft_pass_3_chunk_0",
    "cunny_4x24_soft_pass_3_chunk_1",
    "cunny_4x24_soft_pass_4_chunk_0",
    "cunny_4x24_soft_pass_4_chunk_1",
    "cunny_4x24_soft_pass_5",
];

const CUNNY_4X24_DS_ENTRY_POINTS: [&str; 11] = [
    "cunny_4x24_ds_pass_0_chunk_0",
    "cunny_4x24_ds_pass_0_chunk_1",
    "cunny_4x24_ds_pass_1_chunk_0",
    "cunny_4x24_ds_pass_1_chunk_1",
    "cunny_4x24_ds_pass_2_chunk_0",
    "cunny_4x24_ds_pass_2_chunk_1",
    "cunny_4x24_ds_pass_3_chunk_0",
    "cunny_4x24_ds_pass_3_chunk_1",
    "cunny_4x24_ds_pass_4_chunk_0",
    "cunny_4x24_ds_pass_4_chunk_1",
    "cunny_4x24_ds_pass_5",
];

const CUNNY_4X32_NVL_ENTRY_POINTS: [&str; 16] = [
    "cunny_4x32_nvl_pass_0_chunk_0",
    "cunny_4x32_nvl_pass_0_chunk_1",
    "cunny_4x32_nvl_pass_0_chunk_2",
    "cunny_4x32_nvl_pass_1_chunk_0",
    "cunny_4x32_nvl_pass_1_chunk_1",
    "cunny_4x32_nvl_pass_1_chunk_2",
    "cunny_4x32_nvl_pass_2_chunk_0",
    "cunny_4x32_nvl_pass_2_chunk_1",
    "cunny_4x32_nvl_pass_2_chunk_2",
    "cunny_4x32_nvl_pass_3_chunk_0",
    "cunny_4x32_nvl_pass_3_chunk_1",
    "cunny_4x32_nvl_pass_3_chunk_2",
    "cunny_4x32_nvl_pass_4_chunk_0",
    "cunny_4x32_nvl_pass_4_chunk_1",
    "cunny_4x32_nvl_pass_4_chunk_2",
    "cunny_4x32_nvl_pass_5",
];

const CUNNY_4X32_SOFT_ENTRY_POINTS: [&str; 16] = [
    "cunny_4x32_soft_pass_0_chunk_0",
    "cunny_4x32_soft_pass_0_chunk_1",
    "cunny_4x32_soft_pass_0_chunk_2",
    "cunny_4x32_soft_pass_1_chunk_0",
    "cunny_4x32_soft_pass_1_chunk_1",
    "cunny_4x32_soft_pass_1_chunk_2",
    "cunny_4x32_soft_pass_2_chunk_0",
    "cunny_4x32_soft_pass_2_chunk_1",
    "cunny_4x32_soft_pass_2_chunk_2",
    "cunny_4x32_soft_pass_3_chunk_0",
    "cunny_4x32_soft_pass_3_chunk_1",
    "cunny_4x32_soft_pass_3_chunk_2",
    "cunny_4x32_soft_pass_4_chunk_0",
    "cunny_4x32_soft_pass_4_chunk_1",
    "cunny_4x32_soft_pass_4_chunk_2",
    "cunny_4x32_soft_pass_5",
];

const CUNNY_4X32_DS_ENTRY_POINTS: [&str; 16] = [
    "cunny_4x32_ds_pass_0_chunk_0",
    "cunny_4x32_ds_pass_0_chunk_1",
    "cunny_4x32_ds_pass_0_chunk_2",
    "cunny_4x32_ds_pass_1_chunk_0",
    "cunny_4x32_ds_pass_1_chunk_1",
    "cunny_4x32_ds_pass_1_chunk_2",
    "cunny_4x32_ds_pass_2_chunk_0",
    "cunny_4x32_ds_pass_2_chunk_1",
    "cunny_4x32_ds_pass_2_chunk_2",
    "cunny_4x32_ds_pass_3_chunk_0",
    "cunny_4x32_ds_pass_3_chunk_1",
    "cunny_4x32_ds_pass_3_chunk_2",
    "cunny_4x32_ds_pass_4_chunk_0",
    "cunny_4x32_ds_pass_4_chunk_1",
    "cunny_4x32_ds_pass_4_chunk_2",
    "cunny_4x32_ds_pass_5",
];

const CUNNY_8X32_NVL_ENTRY_POINTS: [&str; 28] = [
    "cunny_8x32_nvl_pass_0_chunk_0",
    "cunny_8x32_nvl_pass_0_chunk_1",
    "cunny_8x32_nvl_pass_0_chunk_2",
    "cunny_8x32_nvl_pass_1_chunk_0",
    "cunny_8x32_nvl_pass_1_chunk_1",
    "cunny_8x32_nvl_pass_1_chunk_2",
    "cunny_8x32_nvl_pass_2_chunk_0",
    "cunny_8x32_nvl_pass_2_chunk_1",
    "cunny_8x32_nvl_pass_2_chunk_2",
    "cunny_8x32_nvl_pass_3_chunk_0",
    "cunny_8x32_nvl_pass_3_chunk_1",
    "cunny_8x32_nvl_pass_3_chunk_2",
    "cunny_8x32_nvl_pass_4_chunk_0",
    "cunny_8x32_nvl_pass_4_chunk_1",
    "cunny_8x32_nvl_pass_4_chunk_2",
    "cunny_8x32_nvl_pass_5_chunk_0",
    "cunny_8x32_nvl_pass_5_chunk_1",
    "cunny_8x32_nvl_pass_5_chunk_2",
    "cunny_8x32_nvl_pass_6_chunk_0",
    "cunny_8x32_nvl_pass_6_chunk_1",
    "cunny_8x32_nvl_pass_6_chunk_2",
    "cunny_8x32_nvl_pass_7_chunk_0",
    "cunny_8x32_nvl_pass_7_chunk_1",
    "cunny_8x32_nvl_pass_7_chunk_2",
    "cunny_8x32_nvl_pass_8_chunk_0",
    "cunny_8x32_nvl_pass_8_chunk_1",
    "cunny_8x32_nvl_pass_8_chunk_2",
    "cunny_8x32_nvl_pass_9",
];

const CUNNY_8X32_DS_ENTRY_POINTS: [&str; 28] = [
    "cunny_8x32_ds_pass_0_chunk_0",
    "cunny_8x32_ds_pass_0_chunk_1",
    "cunny_8x32_ds_pass_0_chunk_2",
    "cunny_8x32_ds_pass_1_chunk_0",
    "cunny_8x32_ds_pass_1_chunk_1",
    "cunny_8x32_ds_pass_1_chunk_2",
    "cunny_8x32_ds_pass_2_chunk_0",
    "cunny_8x32_ds_pass_2_chunk_1",
    "cunny_8x32_ds_pass_2_chunk_2",
    "cunny_8x32_ds_pass_3_chunk_0",
    "cunny_8x32_ds_pass_3_chunk_1",
    "cunny_8x32_ds_pass_3_chunk_2",
    "cunny_8x32_ds_pass_4_chunk_0",
    "cunny_8x32_ds_pass_4_chunk_1",
    "cunny_8x32_ds_pass_4_chunk_2",
    "cunny_8x32_ds_pass_5_chunk_0",
    "cunny_8x32_ds_pass_5_chunk_1",
    "cunny_8x32_ds_pass_5_chunk_2",
    "cunny_8x32_ds_pass_6_chunk_0",
    "cunny_8x32_ds_pass_6_chunk_1",
    "cunny_8x32_ds_pass_6_chunk_2",
    "cunny_8x32_ds_pass_7_chunk_0",
    "cunny_8x32_ds_pass_7_chunk_1",
    "cunny_8x32_ds_pass_7_chunk_2",
    "cunny_8x32_ds_pass_8_chunk_0",
    "cunny_8x32_ds_pass_8_chunk_1",
    "cunny_8x32_ds_pass_8_chunk_2",
    "cunny_8x32_ds_pass_9",
];

const CUNNY_VERYFAST_NVL_PASSES: [CunnyPassSpec; 4] = [
    CunnyPassSpec {
        inputs: &[DUMMY_READ, DUMMY_READ, DUMMY_READ],
        outputs: &[0, 1, DUMMY_OUT0],
    },
    CunnyPassSpec {
        inputs: &[0, 1, DUMMY_READ],
        outputs: &[2, 3, DUMMY_OUT0],
    },
    CunnyPassSpec {
        inputs: &[2, 3, DUMMY_READ],
        outputs: &[0, DUMMY_OUT0, DUMMY_OUT1],
    },
    CunnyPassSpec {
        inputs: &[0, DUMMY_READ, DUMMY_READ],
        outputs: &[DUMMY_OUT0, DUMMY_OUT1, DUMMY_OUT2],
    },
];

const CUNNY_VERYFAST_SOFT_PASSES: [CunnyPassSpec; 4] = [
    CunnyPassSpec {
        inputs: &[DUMMY_READ, DUMMY_READ, DUMMY_READ],
        outputs: &[0, 1, DUMMY_OUT0],
    },
    CunnyPassSpec {
        inputs: &[0, 1, DUMMY_READ],
        outputs: &[2, 3, DUMMY_OUT0],
    },
    CunnyPassSpec {
        inputs: &[2, 3, DUMMY_READ],
        outputs: &[0, DUMMY_OUT0, DUMMY_OUT1],
    },
    CunnyPassSpec {
        inputs: &[0, DUMMY_READ, DUMMY_READ],
        outputs: &[DUMMY_OUT0, DUMMY_OUT1, DUMMY_OUT2],
    },
];

const CUNNY_FASTER_NVL_PASSES: [CunnyPassSpec; 4] = [
    CunnyPassSpec {
        inputs: &[DUMMY_READ, DUMMY_READ, DUMMY_READ],
        outputs: &[0, 1, DUMMY_OUT0],
    },
    CunnyPassSpec {
        inputs: &[0, 1, DUMMY_READ],
        outputs: &[2, 3, DUMMY_OUT0],
    },
    CunnyPassSpec {
        inputs: &[2, 3, DUMMY_READ],
        outputs: &[0, 1, DUMMY_OUT0],
    },
    CunnyPassSpec {
        inputs: &[0, 1, DUMMY_READ],
        outputs: &[DUMMY_OUT0, DUMMY_OUT1, DUMMY_OUT2],
    },
];

const CUNNY_FASTER_SOFT_PASSES: [CunnyPassSpec; 4] = [
    CunnyPassSpec {
        inputs: &[DUMMY_READ, DUMMY_READ, DUMMY_READ],
        outputs: &[0, 1, DUMMY_OUT0],
    },
    CunnyPassSpec {
        inputs: &[0, 1, DUMMY_READ],
        outputs: &[2, 3, DUMMY_OUT0],
    },
    CunnyPassSpec {
        inputs: &[2, 3, DUMMY_READ],
        outputs: &[0, 1, DUMMY_OUT0],
    },
    CunnyPassSpec {
        inputs: &[0, 1, DUMMY_READ],
        outputs: &[DUMMY_OUT0, DUMMY_OUT1, DUMMY_OUT2],
    },
];

const CUNNY_FASTER_DS_PASSES: [CunnyPassSpec; 4] = CUNNY_FASTER_SOFT_PASSES;

const CUNNY_FAST_NVL_PASSES: [CunnyPassSpec; 4] = [
    CunnyPassSpec {
        inputs: &[DUMMY_READ, DUMMY_READ, DUMMY_READ],
        outputs: &[0, 1, 2],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2],
        outputs: &[3, 4, 5],
    },
    CunnyPassSpec {
        inputs: &[3, 4, 5],
        outputs: &[0, 1, DUMMY_OUT0],
    },
    CunnyPassSpec {
        inputs: &[0, 1, DUMMY_READ],
        outputs: &[DUMMY_OUT0, DUMMY_OUT1, DUMMY_OUT2],
    },
];

const CUNNY_FAST_SOFT_PASSES: [CunnyPassSpec; 4] = [
    CunnyPassSpec {
        inputs: &[DUMMY_READ, DUMMY_READ, DUMMY_READ],
        outputs: &[0, 1, 2],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2],
        outputs: &[3, 4, 5],
    },
    CunnyPassSpec {
        inputs: &[3, 4, 5],
        outputs: &[0, 1, DUMMY_OUT0],
    },
    CunnyPassSpec {
        inputs: &[0, 1, DUMMY_READ],
        outputs: &[DUMMY_OUT0, DUMMY_OUT1, DUMMY_OUT2],
    },
];

const CUNNY_FAST_DS_PASSES: [CunnyPassSpec; 4] = CUNNY_FAST_SOFT_PASSES;

const CUNNY_2X12_MPV_PASSES: [CunnyPassSpec; 4] = [
    CunnyPassSpec {
        inputs: &[DUMMY_READ, DUMMY_READ, DUMMY_READ],
        outputs: &[0, 1, 2],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2],
        outputs: &[3, 4, 5],
    },
    CunnyPassSpec {
        inputs: &[3, 4, 5],
        outputs: &[0, 1, 2],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2],
        outputs: &[DUMMY_OUT0, DUMMY_OUT1, DUMMY_OUT2],
    },
];

const CUNNY_3X12_NVL_PASSES: [CunnyPassSpec; 5] = [
    CunnyPassSpec {
        inputs: &[DUMMY_READ, DUMMY_READ, DUMMY_READ],
        outputs: &[0, 1, 2],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2],
        outputs: &[3, 4, 5],
    },
    CunnyPassSpec {
        inputs: &[3, 4, 5],
        outputs: &[0, 1, 2],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2],
        outputs: &[3, 4, 5],
    },
    CunnyPassSpec {
        inputs: &[3, 4, 5],
        outputs: &[DUMMY_OUT0, DUMMY_OUT1, DUMMY_OUT2],
    },
];

const CUNNY_4X12_NVL_PASSES: [CunnyPassSpec; 6] = [
    CunnyPassSpec {
        inputs: &[DUMMY_READ, DUMMY_READ, DUMMY_READ],
        outputs: &[0, 1, 2],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2],
        outputs: &[3, 4, 5],
    },
    CunnyPassSpec {
        inputs: &[3, 4, 5],
        outputs: &[0, 1, 2],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2],
        outputs: &[3, 4, 5],
    },
    CunnyPassSpec {
        inputs: &[3, 4, 5],
        outputs: &[0, 1, 2],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2],
        outputs: &[DUMMY_OUT0, DUMMY_OUT1, DUMMY_OUT2],
    },
];

const CUNNY_4X16_NVL_PASSES: [CunnyPassSpec; 11] = [
    CunnyPassSpec {
        inputs: &[],
        outputs: &[0, 1, 2],
    },
    CunnyPassSpec {
        inputs: &[],
        outputs: &[3],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2, 3],
        outputs: &[4, 5, 6],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2, 3],
        outputs: &[7],
    },
    CunnyPassSpec {
        inputs: &[4, 5, 6, 7],
        outputs: &[0, 1, 2],
    },
    CunnyPassSpec {
        inputs: &[4, 5, 6, 7],
        outputs: &[3],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2, 3],
        outputs: &[4, 5, 6],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2, 3],
        outputs: &[7],
    },
    CunnyPassSpec {
        inputs: &[4, 5, 6, 7],
        outputs: &[0, 1, 2],
    },
    CunnyPassSpec {
        inputs: &[4, 5, 6, 7],
        outputs: &[3],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2, 3],
        outputs: &[],
    },
];

const CUNNY_4X24_NVL_PASSES: [CunnyPassSpec; 11] = [
    CunnyPassSpec {
        inputs: &[],
        outputs: &[0, 1, 2],
    },
    CunnyPassSpec {
        inputs: &[],
        outputs: &[3, 4, 5],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2, 3, 4, 5],
        outputs: &[6, 7, 8],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2, 3, 4, 5],
        outputs: &[9, 10, 11],
    },
    CunnyPassSpec {
        inputs: &[6, 7, 8, 9, 10, 11],
        outputs: &[0, 1, 2],
    },
    CunnyPassSpec {
        inputs: &[6, 7, 8, 9, 10, 11],
        outputs: &[3, 4, 5],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2, 3, 4, 5],
        outputs: &[6, 7, 8],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2, 3, 4, 5],
        outputs: &[9, 10, 11],
    },
    CunnyPassSpec {
        inputs: &[6, 7, 8, 9, 10, 11],
        outputs: &[0, 1, 2],
    },
    CunnyPassSpec {
        inputs: &[6, 7, 8, 9, 10, 11],
        outputs: &[3, 4, 5],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2, 3, 4, 5],
        outputs: &[],
    },
];

const CUNNY_4X32_NVL_PASSES: [CunnyPassSpec; 16] = [
    CunnyPassSpec {
        inputs: &[],
        outputs: &[0, 1, 2],
    },
    CunnyPassSpec {
        inputs: &[],
        outputs: &[3, 4, 5],
    },
    CunnyPassSpec {
        inputs: &[],
        outputs: &[6, 7],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2, 3, 4, 5, 6, 7],
        outputs: &[8, 9, 10],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2, 3, 4, 5, 6, 7],
        outputs: &[11, 12, 13],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2, 3, 4, 5, 6, 7],
        outputs: &[14, 15],
    },
    CunnyPassSpec {
        inputs: &[8, 9, 10, 11, 12, 13, 14, 15],
        outputs: &[0, 1, 2],
    },
    CunnyPassSpec {
        inputs: &[8, 9, 10, 11, 12, 13, 14, 15],
        outputs: &[3, 4, 5],
    },
    CunnyPassSpec {
        inputs: &[8, 9, 10, 11, 12, 13, 14, 15],
        outputs: &[6, 7],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2, 3, 4, 5, 6, 7],
        outputs: &[8, 9, 10],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2, 3, 4, 5, 6, 7],
        outputs: &[11, 12, 13],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2, 3, 4, 5, 6, 7],
        outputs: &[14, 15],
    },
    CunnyPassSpec {
        inputs: &[8, 9, 10, 11, 12, 13, 14, 15],
        outputs: &[0, 1, 2],
    },
    CunnyPassSpec {
        inputs: &[8, 9, 10, 11, 12, 13, 14, 15],
        outputs: &[3, 4, 5],
    },
    CunnyPassSpec {
        inputs: &[8, 9, 10, 11, 12, 13, 14, 15],
        outputs: &[6, 7],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2, 3, 4, 5, 6, 7],
        outputs: &[],
    },
];

const CUNNY_8X32_NVL_PASSES: [CunnyPassSpec; 28] = [
    CunnyPassSpec {
        inputs: &[],
        outputs: &[0, 1, 2],
    },
    CunnyPassSpec {
        inputs: &[],
        outputs: &[3, 4, 5],
    },
    CunnyPassSpec {
        inputs: &[],
        outputs: &[6, 7],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2, 3, 4, 5, 6, 7],
        outputs: &[8, 9, 10],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2, 3, 4, 5, 6, 7],
        outputs: &[11, 12, 13],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2, 3, 4, 5, 6, 7],
        outputs: &[14, 15],
    },
    CunnyPassSpec {
        inputs: &[8, 9, 10, 11, 12, 13, 14, 15],
        outputs: &[0, 1, 2],
    },
    CunnyPassSpec {
        inputs: &[8, 9, 10, 11, 12, 13, 14, 15],
        outputs: &[3, 4, 5],
    },
    CunnyPassSpec {
        inputs: &[8, 9, 10, 11, 12, 13, 14, 15],
        outputs: &[6, 7],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2, 3, 4, 5, 6, 7],
        outputs: &[8, 9, 10],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2, 3, 4, 5, 6, 7],
        outputs: &[11, 12, 13],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2, 3, 4, 5, 6, 7],
        outputs: &[14, 15],
    },
    CunnyPassSpec {
        inputs: &[8, 9, 10, 11, 12, 13, 14, 15],
        outputs: &[0, 1, 2],
    },
    CunnyPassSpec {
        inputs: &[8, 9, 10, 11, 12, 13, 14, 15],
        outputs: &[3, 4, 5],
    },
    CunnyPassSpec {
        inputs: &[8, 9, 10, 11, 12, 13, 14, 15],
        outputs: &[6, 7],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2, 3, 4, 5, 6, 7],
        outputs: &[8, 9, 10],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2, 3, 4, 5, 6, 7],
        outputs: &[11, 12, 13],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2, 3, 4, 5, 6, 7],
        outputs: &[14, 15],
    },
    CunnyPassSpec {
        inputs: &[8, 9, 10, 11, 12, 13, 14, 15],
        outputs: &[0, 1, 2],
    },
    CunnyPassSpec {
        inputs: &[8, 9, 10, 11, 12, 13, 14, 15],
        outputs: &[3, 4, 5],
    },
    CunnyPassSpec {
        inputs: &[8, 9, 10, 11, 12, 13, 14, 15],
        outputs: &[6, 7],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2, 3, 4, 5, 6, 7],
        outputs: &[8, 9, 10],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2, 3, 4, 5, 6, 7],
        outputs: &[11, 12, 13],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2, 3, 4, 5, 6, 7],
        outputs: &[14, 15],
    },
    CunnyPassSpec {
        inputs: &[8, 9, 10, 11, 12, 13, 14, 15],
        outputs: &[0, 1, 2],
    },
    CunnyPassSpec {
        inputs: &[8, 9, 10, 11, 12, 13, 14, 15],
        outputs: &[3, 4, 5],
    },
    CunnyPassSpec {
        inputs: &[8, 9, 10, 11, 12, 13, 14, 15],
        outputs: &[6, 7],
    },
    CunnyPassSpec {
        inputs: &[0, 1, 2, 3, 4, 5, 6, 7],
        outputs: &[],
    },
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

fn storage_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::StorageTexture {
            access: wgpu::StorageTextureAccess::WriteOnly,
            format: TEXTURE_FORMAT,
            view_dimension: wgpu::TextureViewDimension::D2,
        },
        count: None,
    }
}

fn input_binding(slot: usize) -> u32 {
    1 + slot as u32
}

fn output_binding(slot: usize) -> u32 {
    1 + CUNNY_INPUT_SLOTS as u32 + slot as u32
}

fn final_binding() -> u32 {
    1 + CUNNY_INPUT_SLOTS as u32 + CUNNY_OUTPUT_SLOTS as u32
}

fn params_binding() -> u32 {
    final_binding() + 1
}

fn dummy_output_index(slot: usize) -> usize {
    DUMMY_OUT0 + slot
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

fn create_intermediate_texture(
    device: &wgpu::Device,
    size: wgpu::Extent3d,
    index: usize,
) -> wgpu::Texture {
    let label = format!("suisuiview-cunny-intermediate-{index}");
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(&label),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: TEXTURE_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING,
        view_formats: &[],
    })
}

#[cfg(test)]
mod tests {
    use super::{cunny_variant_sources, CUNNY_VARIANTS};
    use crate::core::state::WgpuUpscaleMethod;

    #[test]
    fn filtered_cunny_sources_select_only_requested_variant() {
        let variants =
            cunny_variant_sources(Some(WgpuUpscaleMethod::CunnyVeryfastSoft)).collect::<Vec<_>>();

        assert_eq!(variants.len(), 1);
        assert_eq!(variants[0].method, WgpuUpscaleMethod::CunnyVeryfastSoft);
        assert_eq!(variants[0].entry_points.len(), 4);
    }

    #[test]
    fn unfiltered_cunny_sources_keep_full_matrix_available() {
        let variants = cunny_variant_sources(None).collect::<Vec<_>>();

        assert_eq!(variants.len(), CUNNY_VARIANTS.len());
        assert!(variants.iter().all(|variant| variant.method.is_cunny()));
    }
}
