use std::borrow::Cow;
use std::sync::mpsc;
use std::time::Instant;

use wgpu::util::DeviceExt;

use super::wgpu_worker::elapsed_ms;

const TEXTURE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct EffectParams {
    source_output: [u32; 4],
    transform_filter: [u32; 4],
    color_origin: [u32; 4],
    upscale: [u32; 4],
    opacity: [f32; 4],
    display: [f32; 4],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WgpuEffectScenario {
    NativeBase,
    NativeRcas,
    UpscaleFsr1Style2x,
    UpscaleFsr1EasuRcas2x,
    DownscaleHammingHalf,
    DownscaleLanczos3Half,
}

impl WgpuEffectScenario {
    pub(crate) const ALL: [Self; 6] = [
        Self::NativeBase,
        Self::NativeRcas,
        Self::UpscaleFsr1Style2x,
        Self::UpscaleFsr1EasuRcas2x,
        Self::DownscaleHammingHalf,
        Self::DownscaleLanczos3Half,
    ];

    pub(crate) fn token(self) -> &'static str {
        match self {
            Self::NativeBase => "effect-native-base",
            Self::NativeRcas => "effect-native-rcas",
            Self::UpscaleFsr1Style2x => "effect-upscale-fsr1-style-2x",
            Self::UpscaleFsr1EasuRcas2x => "effect-upscale-fsr1-easu-rcas-2x",
            Self::DownscaleHammingHalf => "effect-downscale-hamming-half",
            Self::DownscaleLanczos3Half => "effect-downscale-lanczos3-half",
        }
    }

    pub(crate) fn from_token(token: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|scenario| token.eq_ignore_ascii_case(scenario.token()))
    }

    fn output_size(self, source_size: [usize; 2]) -> [usize; 2] {
        match self {
            Self::NativeBase | Self::NativeRcas => source_size,
            Self::UpscaleFsr1Style2x | Self::UpscaleFsr1EasuRcas2x => [
                source_size[0].saturating_mul(2).max(1),
                source_size[1].saturating_mul(2).max(1),
            ],
            Self::DownscaleHammingHalf | Self::DownscaleLanczos3Half => [
                source_size[0].div_ceil(2).max(1),
                source_size[1].div_ceil(2).max(1),
            ],
        }
    }

    fn params(self, source_size: [usize; 2], render_target_size: [usize; 2]) -> EffectParams {
        let filter = match self {
            Self::NativeRcas => 3,
            _ => 0,
        };
        let upscale_method = match self {
            Self::UpscaleFsr1Style2x => 2,
            Self::UpscaleFsr1EasuRcas2x => 4,
            _ => 0,
        };
        let downscale_method = match self {
            Self::DownscaleHammingHalf => 4,
            Self::DownscaleLanczos3Half => 8,
            _ => 2,
        };
        EffectParams {
            source_output: [
                source_size[0] as u32,
                source_size[1] as u32,
                source_size[0] as u32,
                source_size[1] as u32,
            ],
            transform_filter: [0, 0, 0, filter],
            color_origin: [0, 0, 0, 0],
            upscale: [upscale_method, downscale_method, 0, 0],
            opacity: [
                1.0,
                render_target_size[0].max(1) as f32,
                render_target_size[1].max(1) as f32,
                0.0,
            ],
            display: [
                0.0,
                0.0,
                render_target_size[0].max(1) as f32,
                render_target_size[1].max(1) as f32,
            ],
        }
    }

    fn rcas_params(self, render_target_size: [usize; 2]) -> Option<EffectParams> {
        (self == Self::UpscaleFsr1EasuRcas2x).then_some(EffectParams {
            source_output: [
                render_target_size[0] as u32,
                render_target_size[1] as u32,
                render_target_size[0] as u32,
                render_target_size[1] as u32,
            ],
            transform_filter: [0, 0, 0, 0],
            color_origin: [0, 0, 0, 0],
            upscale: [5, 0, 0, 0],
            opacity: [
                1.0,
                render_target_size[0].max(1) as f32,
                render_target_size[1].max(1) as f32,
                0.0,
            ],
            display: [
                0.0,
                0.0,
                render_target_size[0].max(1) as f32,
                render_target_size[1].max(1) as f32,
            ],
        })
    }
}

pub(crate) struct WgpuEffectRenderer {
    texture_bind_group_layout: wgpu::BindGroupLayout,
    params_bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    pipeline: wgpu::RenderPipeline,
    pub(crate) shader_module_ms: f64,
    pub(crate) pipeline_ms: f64,
}

pub(crate) struct WgpuEffectRun {
    pub(crate) output_size: [usize; 2],
    pub(crate) shader_module_ms: f64,
    pub(crate) pipeline_ms: f64,
    pub(crate) upload_ms: f64,
    pub(crate) setup_ms: f64,
    pub(crate) encode_submit_ms: f64,
    pub(crate) readback_ms: f64,
    pub(crate) total_ms: f64,
    pub(crate) checksum: u64,
    pub(crate) rgba: Vec<u8>,
}

impl WgpuEffectRenderer {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let shader_started = Instant::now();
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("suisuiview-runtime-probe-effect-shader"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(include_str!(
                "../../core/gpu_effect.wgsl"
            ))),
        });
        let shader_module_ms = elapsed_ms(shader_started.elapsed());

        let texture_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("suisuiview-runtime-probe-effect-texture-layout"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });
        let params_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("suisuiview-runtime-probe-effect-params-layout"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("suisuiview-runtime-probe-effect-pipeline-layout"),
            bind_group_layouts: &[&texture_bind_group_layout, &params_bind_group_layout],
            push_constant_ranges: &[],
        });
        let pipeline_started = Instant::now();
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("suisuiview-runtime-probe-effect-pipeline"),
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
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
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
        let pipeline_ms = elapsed_ms(pipeline_started.elapsed());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("suisuiview-runtime-probe-effect-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self {
            texture_bind_group_layout,
            params_bind_group_layout,
            sampler,
            pipeline,
            shader_module_ms,
            pipeline_ms,
        }
    }

    pub(crate) fn run(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        source_size: [usize; 2],
        rgba: &[u8],
        scenario: WgpuEffectScenario,
    ) -> Result<WgpuEffectRun, String> {
        validate_rgba(source_size, rgba)?;
        let output_size = scenario.output_size(source_size);
        let total_started = Instant::now();
        let source_extent = extent(source_size);
        let output_extent = extent(output_size);
        let source_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("suisuiview-runtime-probe-effect-source"),
            size: source_extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: TEXTURE_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let upload_started = Instant::now();
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &source_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some((source_size[0] * 4) as u32),
                rows_per_image: Some(source_size[1] as u32),
            },
            source_extent,
        );
        let upload_ms = elapsed_ms(upload_started.elapsed());

        let setup_started = Instant::now();
        let output_texture = render_target_texture(device, output_extent, "effect-output");
        let maybe_intermediate = scenario
            .rcas_params(output_size)
            .map(|_| render_bindable_texture(device, output_extent, "effect-intermediate"));
        let source_view = source_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let source_bind_group = self.texture_bind_group(device, &source_view);
        let first_params =
            self.params_bind_group(device, scenario.params(source_size, output_size));
        let final_params = scenario
            .rcas_params(output_size)
            .map(|params| self.params_bind_group(device, params));
        let padded_bytes_per_row = align_to(
            (output_size[0] * 4) as u32,
            wgpu::COPY_BYTES_PER_ROW_ALIGNMENT,
        );
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("suisuiview-runtime-probe-effect-readback"),
            size: padded_bytes_per_row as u64 * output_size[1] as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let setup_ms = elapsed_ms(setup_started.elapsed());

        let encode_started = Instant::now();
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("suisuiview-runtime-probe-effect-encoder"),
        });
        if let Some(intermediate) = maybe_intermediate.as_ref() {
            let intermediate_view =
                intermediate.create_view(&wgpu::TextureViewDescriptor::default());
            self.render_fullscreen(
                &mut encoder,
                &intermediate_view,
                &source_bind_group,
                &first_params,
            );
            let intermediate_source_bind_group =
                self.texture_bind_group(device, &intermediate_view);
            let output_view = output_texture.create_view(&wgpu::TextureViewDescriptor::default());
            self.render_fullscreen(
                &mut encoder,
                &output_view,
                &intermediate_source_bind_group,
                final_params
                    .as_ref()
                    .expect("RCAS params should exist for two-pass scenario"),
            );
        } else {
            let output_view = output_texture.create_view(&wgpu::TextureViewDescriptor::default());
            self.render_fullscreen(
                &mut encoder,
                &output_view,
                &source_bind_group,
                &first_params,
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
                    rows_per_image: Some(output_size[1] as u32),
                },
            },
            output_extent,
        );
        queue.submit(Some(encoder.finish()));
        let encode_submit_ms = elapsed_ms(encode_started.elapsed());

        let readback_started = Instant::now();
        let buffer_slice = readback.slice(..);
        let (tx, rx) = mpsc::channel();
        buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        device
            .poll(wgpu::PollType::Wait)
            .map_err(|error| format!("wgpu poll failed: {error}"))?;
        rx.recv()
            .map_err(|error| format!("wgpu readback channel failed: {error}"))?
            .map_err(|error| format!("wgpu readback failed: {error}"))?;
        let mapped = buffer_slice.get_mapped_range();
        let mut output = Vec::with_capacity(output_size[0] * output_size[1] * 4);
        let row_bytes = output_size[0] * 4;
        for row in 0..output_size[1] {
            let start = row * padded_bytes_per_row as usize;
            output.extend_from_slice(&mapped[start..start + row_bytes]);
        }
        drop(mapped);
        readback.unmap();
        let readback_ms = elapsed_ms(readback_started.elapsed());
        let checksum = output
            .iter()
            .fold(0u64, |sum, byte| sum.wrapping_add(*byte as u64));
        Ok(WgpuEffectRun {
            output_size,
            shader_module_ms: self.shader_module_ms,
            pipeline_ms: self.pipeline_ms,
            upload_ms,
            setup_ms,
            encode_submit_ms,
            readback_ms,
            total_ms: elapsed_ms(total_started.elapsed()),
            checksum,
            rgba: output,
        })
    }

    fn texture_bind_group(
        &self,
        device: &wgpu::Device,
        view: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("suisuiview-runtime-probe-effect-texture-bind-group"),
            layout: &self.texture_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        })
    }

    fn params_bind_group(&self, device: &wgpu::Device, params: EffectParams) -> wgpu::BindGroup {
        let params_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("suisuiview-runtime-probe-effect-params"),
            contents: bytemuck::bytes_of(&params),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("suisuiview-runtime-probe-effect-params-bind-group"),
            layout: &self.params_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: params_buffer.as_entire_binding(),
            }],
        })
    }

    fn render_fullscreen(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        texture_bind_group: &wgpu::BindGroup,
        params_bind_group: &wgpu::BindGroup,
    ) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("suisuiview-runtime-probe-effect-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
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
        pass.set_bind_group(0, texture_bind_group, &[]);
        pass.set_bind_group(1, params_bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

fn validate_rgba(size: [usize; 2], rgba: &[u8]) -> Result<(), String> {
    let expected = size[0]
        .checked_mul(size[1])
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or("effect source byte size overflowed")?;
    if rgba.len() != expected {
        return Err(format!(
            "rgba length {} does not match expected {expected}",
            rgba.len()
        ));
    }
    Ok(())
}

fn render_target_texture(
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
        format: TEXTURE_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

fn render_bindable_texture(
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
        format: TEXTURE_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    })
}

fn extent(size: [usize; 2]) -> wgpu::Extent3d {
    wgpu::Extent3d {
        width: size[0].max(1) as u32,
        height: size[1].max(1) as u32,
        depth_or_array_layers: 1,
    }
}

fn align_to(value: u32, alignment: u32) -> u32 {
    value.div_ceil(alignment) * alignment
}
