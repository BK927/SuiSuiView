use super::{align_to, GpuUpscaleOutput, TEXTURE_FORMAT};
use crate::core::gpu_effect::color_image_to_rgba;
use eframe::egui::ColorImage;
use std::borrow::Cow;
use std::sync::mpsc;
use std::time::Instant;
use wgpu::util::DeviceExt;

const FEATURE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const FEATURE_BYTES_PER_PIXEL: u64 = 8;
const OUTPUT_BYTES_PER_PIXEL: u64 = 4;
const TRANSIENT_BYTES_LIMIT: u64 = 768 * 1024 * 1024;
const ENTRY_POINTS: [&str; 8] = [
    "artcnn_c4f16_conv2d",
    "artcnn_c4f16_conv2d_1_relu",
    "artcnn_c4f16_conv2d_2_relu",
    "artcnn_c4f16_conv2d_3_relu",
    "artcnn_c4f16_conv2d_4_relu",
    "artcnn_c4f16_conv2d_5",
    "artcnn_c4f16_conv2d_6",
    "artcnn_c4f16_depth_to_space",
];

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct ArtcnnParams {
    source_width: u32,
    source_height: u32,
    output_width: u32,
    output_height: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
    _pad3: u32,
}

pub(super) struct ArtcnnBench {
    bind_group_layout: wgpu::BindGroupLayout,
    pipelines: Vec<wgpu::ComputePipeline>,
}

impl ArtcnnBench {
    pub(super) async fn try_new(device: &wgpu::Device) -> Option<Self> {
        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let bench = Self::new(device);
        match device.pop_error_scope().await {
            Some(error) => {
                eprintln!("ArtCNN C4F16 bench candidate disabled: {error}");
                None
            }
            None => Some(bench),
        }
    }

    fn new(device: &wgpu::Device) -> Self {
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("suisuiview-artcnn-c4f16-bind-group-layout"),
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
            label: Some("suisuiview-artcnn-c4f16-pipeline-layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("suisuiview-artcnn-c4f16-shader"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(include_str!(
                "../../artcnn_c4f16.wgsl"
            ))),
        });
        let pipelines = ENTRY_POINTS
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
        Self {
            bind_group_layout,
            pipelines,
        }
    }

    pub(super) fn apply(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        image: &ColorImage,
        output_size: [usize; 2],
    ) -> Result<GpuUpscaleOutput, String> {
        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let output = self.apply_scoped(device, queue, image, output_size);
        if let Some(error) = pollster::block_on(device.pop_error_scope()) {
            return Err(format!("ArtCNN C4F16 wgpu validation failed: {error}"));
        }
        output
    }

    fn apply_scoped(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        image: &ColorImage,
        output_size: [usize; 2],
    ) -> Result<GpuUpscaleOutput, String> {
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
                "ArtCNN C4F16 requires 2x output or a one-pixel crop, got {source_width}x{source_height} -> {output_width}x{output_height}"
            ));
        }

        let padded_bytes_per_row = align_to(
            (output_width * 4) as u32,
            wgpu::COPY_BYTES_PER_ROW_ALIGNMENT,
        );
        validate_resource_size(
            device,
            source_width,
            source_height,
            exact_width,
            exact_height,
            output_width,
            output_height,
            padded_bytes_per_row,
        )?;

        let started = Instant::now();
        let source_bytes = color_image_to_rgba(image);
        let source_extent = extent(source_width, source_height);
        let feature_extent = extent(exact_width, exact_height);
        let output_extent = extent(output_width, output_height);

        let source_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("suisuiview-artcnn-source"),
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

        let skip = create_feature_texture(device, feature_extent, "artcnn-skip");
        let tmp_a = create_feature_texture(device, feature_extent, "artcnn-tmp-a");
        let tmp_b = create_feature_texture(device, feature_extent, "artcnn-tmp-b");
        let conv6 = create_feature_texture(device, source_extent, "artcnn-conv6");
        let output_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("suisuiview-artcnn-output"),
            size: output_extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: TEXTURE_FORMAT,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });

        let source_view = source_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let skip_view = skip.create_view(&wgpu::TextureViewDescriptor::default());
        let tmp_a_view = tmp_a.create_view(&wgpu::TextureViewDescriptor::default());
        let tmp_b_view = tmp_b.create_view(&wgpu::TextureViewDescriptor::default());
        let conv6_view = conv6.create_view(&wgpu::TextureViewDescriptor::default());
        let output_view = output_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let params = ArtcnnParams {
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
            label: Some("suisuiview-artcnn-params"),
            contents: bytemuck::bytes_of(&params),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("suisuiview-artcnn-readback"),
            size: padded_bytes_per_row as u64 * output_height as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("suisuiview-artcnn-encoder"),
        });
        let source_groups = WorkSizes {
            x: source_width as u32,
            y: source_height as u32,
        };
        let output_groups = WorkSizes {
            x: output_width as u32,
            y: output_height as u32,
        };
        let mut pass_resources = PassResources {
            device,
            encoder: &mut encoder,
            params_buffer: &params_buffer,
        };

        self.run_pass(
            &mut pass_resources,
            0,
            Views {
                source: &source_view,
                input0: &source_view,
                input1: &source_view,
                out: &skip_view,
                final_out: &output_view,
            },
            source_groups,
        );
        self.run_pass(
            &mut pass_resources,
            1,
            Views {
                source: &source_view,
                input0: &skip_view,
                input1: &source_view,
                out: &tmp_a_view,
                final_out: &output_view,
            },
            source_groups,
        );
        self.run_pass(
            &mut pass_resources,
            2,
            Views {
                source: &source_view,
                input0: &tmp_a_view,
                input1: &source_view,
                out: &tmp_b_view,
                final_out: &output_view,
            },
            source_groups,
        );
        self.run_pass(
            &mut pass_resources,
            3,
            Views {
                source: &source_view,
                input0: &tmp_b_view,
                input1: &source_view,
                out: &tmp_a_view,
                final_out: &output_view,
            },
            source_groups,
        );
        self.run_pass(
            &mut pass_resources,
            4,
            Views {
                source: &source_view,
                input0: &tmp_a_view,
                input1: &source_view,
                out: &tmp_b_view,
                final_out: &output_view,
            },
            source_groups,
        );
        self.run_pass(
            &mut pass_resources,
            5,
            Views {
                source: &source_view,
                input0: &tmp_b_view,
                input1: &source_view,
                out: &tmp_a_view,
                final_out: &output_view,
            },
            source_groups,
        );
        self.run_pass(
            &mut pass_resources,
            6,
            Views {
                source: &source_view,
                input0: &skip_view,
                input1: &tmp_a_view,
                out: &conv6_view,
                final_out: &output_view,
            },
            source_groups,
        );
        self.run_pass(
            &mut pass_resources,
            7,
            Views {
                source: &source_view,
                input0: &conv6_view,
                input1: &source_view,
                out: &tmp_b_view,
                final_out: &output_view,
            },
            output_groups,
        );

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
                label: Some(ENTRY_POINTS[pipeline_index]),
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
                label: Some(ENTRY_POINTS[pipeline_index]),
                timestamp_writes: None,
            });
        pass.set_pipeline(&self.pipelines[pipeline_index]);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.dispatch_workgroups(groups.x.div_ceil(8), groups.y.div_ceil(8), 1);
    }
}

struct PassResources<'a> {
    device: &'a wgpu::Device,
    encoder: &'a mut wgpu::CommandEncoder,
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

fn validate_resource_size(
    device: &wgpu::Device,
    source_width: usize,
    source_height: usize,
    feature_width: usize,
    feature_height: usize,
    output_width: usize,
    output_height: usize,
    padded_bytes_per_row: u32,
) -> Result<(), String> {
    let max_texture_dimension = device.limits().max_texture_dimension_2d as usize;
    validate_texture_size(max_texture_dimension, source_width, source_height, "source")?;
    validate_texture_size(
        max_texture_dimension,
        feature_width,
        feature_height,
        "feature",
    )?;
    validate_texture_size(max_texture_dimension, output_width, output_height, "output")?;

    let feature_bytes = texture_bytes(feature_width, feature_height, FEATURE_BYTES_PER_PIXEL)?;
    let conv6_bytes = texture_bytes(source_width, source_height, FEATURE_BYTES_PER_PIXEL)?;
    let output_bytes = texture_bytes(output_width, output_height, OUTPUT_BYTES_PER_PIXEL)?;
    let readback_bytes = (padded_bytes_per_row as u64)
        .checked_mul(output_height as u64)
        .ok_or_else(|| "ArtCNN C4F16 readback buffer size overflowed".to_owned())?;
    let transient_bytes = feature_bytes
        .checked_mul(3)
        .and_then(|bytes| bytes.checked_add(conv6_bytes))
        .and_then(|bytes| bytes.checked_add(output_bytes))
        .and_then(|bytes| bytes.checked_add(readback_bytes))
        .ok_or_else(|| "ArtCNN C4F16 transient resource size overflowed".to_owned())?;
    if transient_bytes > TRANSIENT_BYTES_LIMIT {
        return Err(format!(
            "ArtCNN C4F16 transient resources would use about {} MiB, above the {} MiB safety limit",
            bytes_to_mib(transient_bytes),
            bytes_to_mib(TRANSIENT_BYTES_LIMIT)
        ));
    }

    Ok(())
}

fn validate_texture_size(
    max_texture_dimension: usize,
    width: usize,
    height: usize,
    label: &str,
) -> Result<(), String> {
    if width > max_texture_dimension || height > max_texture_dimension {
        return Err(format!(
            "ArtCNN C4F16 {label} texture {width}x{height} exceeds adapter 2D texture limit {max_texture_dimension}"
        ));
    }
    Ok(())
}

fn texture_bytes(width: usize, height: usize, bytes_per_pixel: u64) -> Result<u64, String> {
    let pixels = width
        .checked_mul(height)
        .ok_or_else(|| "ArtCNN C4F16 texture pixel count overflowed".to_owned())?;
    (pixels as u64)
        .checked_mul(bytes_per_pixel)
        .ok_or_else(|| "ArtCNN C4F16 texture byte size overflowed".to_owned())
}

fn bytes_to_mib(bytes: u64) -> u64 {
    bytes.div_ceil(1024 * 1024)
}

fn extent(width: usize, height: usize) -> wgpu::Extent3d {
    wgpu::Extent3d {
        width: width as u32,
        height: height as u32,
        depth_or_array_layers: 1,
    }
}
