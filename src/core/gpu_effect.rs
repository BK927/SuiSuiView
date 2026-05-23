use crate::core::effects::{ImageFilter, ViewEffects};
use crate::core::state::DisplayUpscaler;
use eframe::egui::ColorImage;
use std::borrow::Cow;
use std::sync::mpsc;
use std::time::{Duration, Instant};
use wgpu::util::DeviceExt;

const TEXTURE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct EffectParams {
    source_output: [u32; 4],
    transform_filter: [u32; 4],
    color_origin: [u32; 4],
    upscale: [u32; 4],
    opacity: [f32; 4],
}

pub struct GpuEffectBench {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
}

pub struct GpuEffectOutput {
    pub image: ColorImage,
    pub elapsed: Duration,
}

impl GpuEffectBench {
    pub fn new() -> Result<Self, String> {
        pollster::block_on(Self::new_async())
    }

    async fn new_async() -> Result<Self, String> {
        let instance = wgpu::Instance::default();
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: None,
            })
            .await
            .map_err(|error| format!("wgpu adapter unavailable: {error}"))?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("suisuiview-effect-bench-device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::downlevel_webgl2_defaults()
                    .using_resolution(adapter.limits()),
                memory_hints: wgpu::MemoryHints::Performance,
                trace: wgpu::Trace::Off,
            })
            .await
            .map_err(|error| format!("wgpu device unavailable: {error}"))?;

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("suisuiview-effect-bench-shader"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(include_str!("gpu_effect.wgsl"))),
        });
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("suisuiview-effect-bench-bind-group-layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
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
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("suisuiview-effect-bench-pipeline-layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("suisuiview-effect-bench-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
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

        Ok(Self {
            device,
            queue,
            pipeline,
            bind_group_layout,
        })
    }

    pub fn apply(
        &self,
        image: &ColorImage,
        effects: ViewEffects,
    ) -> Result<GpuEffectOutput, String> {
        let [source_width, source_height] = image.size;
        if source_width == 0 || source_height == 0 {
            return Err("cannot apply GPU effects to an empty image".to_owned());
        }
        let output_size = output_size_for_effects(image.size, effects);
        let [output_width, output_height] = output_size;
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

        let source_texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("suisuiview-effect-source"),
            size: source_extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: TEXTURE_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
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

        let output_texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("suisuiview-effect-output"),
            size: output_extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: TEXTURE_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let params = params_for_effects(
            image.size,
            output_size,
            effects,
            DisplayUpscaler::None,
            [0, 0],
            [output_size[0] as u32, output_size[1] as u32],
            1.0,
        );
        let params_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("suisuiview-effect-params"),
                contents: bytemuck::bytes_of(&params),
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let source_view = source_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("suisuiview-effect-bind-group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&source_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: params_buffer.as_entire_binding(),
                },
            ],
        });

        let padded_bytes_per_row = align_to(
            (output_width * 4) as u32,
            wgpu::COPY_BYTES_PER_ROW_ALIGNMENT,
        );
        let output_buffer_size = padded_bytes_per_row as u64 * output_height as u64;
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("suisuiview-effect-readback"),
            size: output_buffer_size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let started = Instant::now();
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("suisuiview-effect-encoder"),
            });
        {
            let output_view = output_texture.create_view(&wgpu::TextureViewDescriptor::default());
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("suisuiview-effect-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &output_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.draw(0..3, 0..1);
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
        self.queue.submit(Some(encoder.finish()));
        let buffer_slice = readback.slice(..);
        let (tx, rx) = mpsc::channel();
        buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result.map_err(|error| error.to_string()));
        });
        self.device
            .poll(wgpu::PollType::Wait)
            .map_err(|error| format!("wgpu poll failed: {error}"))?;
        rx.recv()
            .map_err(|error| format!("wgpu readback channel failed: {error}"))?
            .map_err(|error| format!("wgpu readback failed: {error}"))?;
        let elapsed = started.elapsed();

        let mapped = buffer_slice.get_mapped_range();
        let mut output_bytes = Vec::with_capacity(output_width * output_height * 4);
        let row_bytes = output_width * 4;
        for row in 0..output_height {
            let start = row * padded_bytes_per_row as usize;
            output_bytes.extend_from_slice(&mapped[start..start + row_bytes]);
        }
        drop(mapped);
        readback.unmap();

        Ok(GpuEffectOutput {
            image: ColorImage::from_rgba_unmultiplied(output_size, &output_bytes),
            elapsed,
        })
    }
}

pub(crate) fn params_for_effects(
    source_size: [usize; 2],
    output_size: [usize; 2],
    effects: ViewEffects,
    display_upscaler: DisplayUpscaler,
    output_origin: [u32; 2],
    target_size: [u32; 2],
    opacity: f32,
) -> EffectParams {
    params_for_effects_with_shader_method(
        source_size,
        output_size,
        effects,
        display_upscaler.shader_method_id(),
        output_origin,
        target_size,
        opacity,
    )
}

pub(crate) fn params_for_effects_with_shader_method(
    source_size: [usize; 2],
    output_size: [usize; 2],
    effects: ViewEffects,
    shader_method_id: u32,
    output_origin: [u32; 2],
    target_size: [u32; 2],
    opacity: f32,
) -> EffectParams {
    let filter = match effects.filter {
        ImageFilter::None => 0,
        ImageFilter::Smooth => 1,
        ImageFilter::SmoothSharpen => 2,
        ImageFilter::RcasSharpen => 3,
    };
    EffectParams {
        source_output: [
            source_size[0] as u32,
            source_size[1] as u32,
            output_size[0] as u32,
            output_size[1] as u32,
        ],
        transform_filter: [
            effects.transform.rotation_quadrants as u32 % 4,
            effects.transform.flip_horizontal as u32,
            effects.transform.flip_vertical as u32,
            filter,
        ],
        color_origin: [
            effects.gamma as u32,
            effects.invert_colors as u32,
            output_origin[0],
            output_origin[1],
        ],
        upscale: [shader_method_id, 0, 0, 0],
        opacity: [
            opacity,
            target_size[0].max(1) as f32,
            target_size[1].max(1) as f32,
            0.0,
        ],
    }
}

pub(crate) fn output_size_for_effects(size: [usize; 2], effects: ViewEffects) -> [usize; 2] {
    if effects.transform.rotation_quadrants % 2 == 1 {
        [size[1], size[0]]
    } else {
        size
    }
}

pub(crate) fn color_image_to_rgba(image: &ColorImage) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(image.pixels.len() * 4);
    for pixel in &image.pixels {
        bytes.extend_from_slice(&[pixel.r(), pixel.g(), pixel.b(), pixel.a()]);
    }
    bytes
}

fn align_to(value: u32, alignment: u32) -> u32 {
    value.div_ceil(alignment) * alignment
}

pub fn image_diff(left: &ColorImage, right: &ColorImage) -> ImageDiff {
    if left.size != right.size {
        return ImageDiff {
            max_channel_diff: u8::MAX,
            mean_abs_diff: f64::INFINITY,
            different_pixel_ratio: 1.0,
        };
    }

    let mut max_channel_diff = 0u8;
    let mut total_abs_diff = 0u64;
    let mut different_pixels = 0usize;
    for (left, right) in left.pixels.iter().zip(&right.pixels) {
        let diffs = [
            left.r().abs_diff(right.r()),
            left.g().abs_diff(right.g()),
            left.b().abs_diff(right.b()),
            left.a().abs_diff(right.a()),
        ];
        if diffs.iter().any(|diff| *diff != 0) {
            different_pixels += 1;
        }
        for diff in diffs {
            max_channel_diff = max_channel_diff.max(diff);
            total_abs_diff += diff as u64;
        }
    }

    ImageDiff {
        max_channel_diff,
        mean_abs_diff: total_abs_diff as f64 / (left.pixels.len() * 4) as f64,
        different_pixel_ratio: different_pixels as f64 / left.pixels.len() as f64,
    }
}

pub struct ImageDiff {
    pub max_channel_diff: u8,
    pub mean_abs_diff: f64,
    pub different_pixel_ratio: f64,
}

#[cfg(test)]
mod tests {
    use super::{align_to, output_size_for_effects};
    use crate::core::effects::{ViewEffects, ViewTransform};

    #[test]
    fn gpu_output_size_matches_rotation() {
        assert_eq!(
            output_size_for_effects([10, 20], ViewEffects::default()),
            [10, 20]
        );
        assert_eq!(
            output_size_for_effects(
                [10, 20],
                ViewEffects {
                    transform: ViewTransform {
                        rotation_quadrants: 1,
                        ..ViewTransform::default()
                    },
                    ..ViewEffects::default()
                },
            ),
            [20, 10]
        );
    }

    #[test]
    fn row_alignment_rounds_up_to_copy_alignment() {
        assert_eq!(align_to(1, 256), 256);
        assert_eq!(align_to(256, 256), 256);
        assert_eq!(align_to(257, 256), 512);
    }
}
