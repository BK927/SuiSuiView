use crate::core::effects::{ImageFilter, ViewEffects};
use crate::core::state::{WgpuDownscaleMethod, WgpuUpscaleMethod};
use egui::ColorImage;
use std::borrow::Cow;
use std::sync::mpsc;
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use wgpu::util::DeviceExt;

const TEXTURE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// V13 linear-light downscale switch. Ships OFF (gamma-space — the pre-V13
/// behavior); the shipped default is decided by measurement, not here. When on,
/// the WGSL downscale weighted-sum legs (pyramid stages, single-pass residuals,
/// bilinear shrinks) convert each tap sRGB->linear before the weighted average
/// and back after. Textures stay gamma-encoded at rest; only the averaging math
/// changes. UPSCALE and realtime-SR paths are never affected (their params leave
/// the flag clear). Runtime override without a rebuild:
/// `SUISUIVIEW_LINEAR_DOWNSCALE=1` (or `0`/`true`/`false`/`on`/`off`), parsed once.
pub(crate) const LINEAR_DOWNSCALE: bool = false;

/// Mirror of `AppSettings.linear_light_downscale`, stored by the app each time
/// a WGSL page paint is requested (an atomic store per paint is free). A mirror
/// instead of threading the flag through every `params_for_*` signature: those
/// constructors are also called by CLI benches with no settings in scope.
static LINEAR_DOWNSCALE_SETTING: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(LINEAR_DOWNSCALE);

/// Publish the user setting for subsequent draws. Called from the app's WGSL
/// paint path; benches/tests that never call it get the const default.
// The caller lives in the binary crate's app tree (gpu_paint/mod.rs); the lib
// compilation sees no caller — same idiom as `params_for_hardware_mipmap_sample`.
#[allow(dead_code)]
pub(crate) fn set_linear_downscale_setting(enabled: bool) {
    LINEAR_DOWNSCALE_SETTING.store(enabled, std::sync::atomic::Ordering::Relaxed);
}

fn linear_downscale_env() -> Option<bool> {
    static ENV: OnceLock<Option<bool>> = OnceLock::new();
    *ENV.get_or_init(|| {
        std::env::var("SUISUIVIEW_LINEAR_DOWNSCALE")
            .ok()
            .and_then(|raw| match raw.trim() {
                "1" | "true" | "on" | "yes" => Some(true),
                "0" | "false" | "off" | "no" => Some(false),
                _ => None,
            })
    })
}

/// Whether downscale params should carry the linear-light flag. Precedence:
/// per-render test override (offscreen measurement only) → env (diagnostic
/// pin, wins over the UI so an A/B session cannot be disturbed mid-run) →
/// the user setting mirror.
pub(crate) fn linear_downscale_enabled() -> bool {
    #[cfg(test)]
    if let Some(forced) = linear_downscale_test_override() {
        return forced;
    }
    linear_downscale_env()
        .unwrap_or_else(|| LINEAR_DOWNSCALE_SETTING.load(std::sync::atomic::Ordering::Relaxed))
}

// The offscreen measurement renders BOTH legs in one process, which the env
// OnceLock cannot do (it resolves once). This per-render override lets that test
// force each leg via the params path without a rebuild or env. Production never
// touches it.
#[cfg(test)]
static LINEAR_DOWNSCALE_TEST_OVERRIDE: std::sync::atomic::AtomicU8 =
    std::sync::atomic::AtomicU8::new(0);

#[cfg(test)]
fn linear_downscale_test_override() -> Option<bool> {
    match LINEAR_DOWNSCALE_TEST_OVERRIDE.load(std::sync::atomic::Ordering::Relaxed) {
        1 => Some(false),
        2 => Some(true),
        _ => None,
    }
}

/// Force (or clear) the linear-downscale decision for subsequent renders on this
/// process. `None` restores the env/const resolution. Test-only.
#[cfg(test)]
#[allow(dead_code)]
pub(crate) fn set_linear_downscale_test_override(value: Option<bool>) {
    let encoded = match value {
        None => 0,
        Some(false) => 1,
        Some(true) => 2,
    };
    LINEAR_DOWNSCALE_TEST_OVERRIDE.store(encoded, std::sync::atomic::Ordering::Relaxed);
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct EffectParams {
    source_output: [u32; 4],
    transform_filter: [u32; 4],
    color_origin: [u32; 4],
    // `upscale`: x = shader upscale method, y = downscale method, z = hardware-mipmap
    // flag, w = output-dither flag (V12; see `with_dither`). `w` was a free pad slot,
    // so enabling dither adds no uniform-layout churn.
    upscale: [u32; 4],
    opacity: [f32; 4],
    display: [f32; 4],
}

impl EffectParams {
    /// Turn on the final-composite output dither (`gpu_effect.wgsl` reads
    /// `params.upscale.w`). Set only on the draw that composites to the egui
    /// target when its sampled texture is an fp16 quality-chain intermediate, so
    /// the last 8-bit quantization becomes coordinate-stable noise. Left off for
    /// direct-source (native) draws so untouched pixels stay bit-exact, and never
    /// set on intermediate render passes (they already write fp16).
    // Only the binary crate's paint chain (app::gpu_paint::passes) sets this; the
    // lib crate builds params without it, mirroring `params_for_hardware_mipmap_sample`.
    #[allow(dead_code)]
    pub(crate) fn with_dither(mut self) -> Self {
        self.upscale[3] |= 1;
        self
    }

    /// Turn on the V13 linear-light downscale flag (`gpu_effect.wgsl` reads
    /// `params.upscale.w` bit 1). Set only on params whose draw actually runs a
    /// downscale weighted-sum leg. Packs alongside `with_dither` (bit 0); the two
    /// never coincide in practice, but both OR in so neither clobbers the other.
    pub(crate) fn with_linear_downscale(mut self) -> Self {
        self.upscale[3] |= 2;
        self
    }
}

pub struct GpuEffectBench {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    texture_bind_group_layout: wgpu::BindGroupLayout,
    params_bind_group_layout: wgpu::BindGroupLayout,
    texture_sampler: wgpu::Sampler,
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
        let texture_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("suisuiview-effect-bench-texture-layout"),
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
                label: Some("suisuiview-effect-bench-params-layout"),
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
            label: Some("suisuiview-effect-bench-pipeline-layout"),
            bind_group_layouts: &[&texture_bind_group_layout, &params_bind_group_layout],
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
        let texture_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("suisuiview-effect-bench-linear-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        Ok(Self {
            device,
            queue,
            pipeline,
            texture_bind_group_layout,
            params_bind_group_layout,
            texture_sampler,
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
            WgpuUpscaleMethod::None,
            WgpuDownscaleMethod::Bilinear,
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
        let texture_bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("suisuiview-effect-texture-bind-group"),
            layout: &self.texture_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&source_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.texture_sampler),
                },
            ],
        });
        let params_bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("suisuiview-effect-params-bind-group"),
            layout: &self.params_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: params_buffer.as_entire_binding(),
            }],
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
            pass.set_bind_group(0, &texture_bind_group, &[]);
            pass.set_bind_group(1, &params_bind_group, &[]);
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

// established call surface; a params struct would be pure boilerplate
#[allow(clippy::too_many_arguments)]
pub(crate) fn params_for_effects(
    source_size: [usize; 2],
    output_size: [usize; 2],
    effects: ViewEffects,
    wgpu_upscale_method: WgpuUpscaleMethod,
    wgpu_downscale_method: WgpuDownscaleMethod,
    output_origin: [u32; 2],
    target_size: [u32; 2],
    opacity: f32,
) -> EffectParams {
    params_for_effects_with_display(
        source_size,
        output_size,
        effects,
        wgpu_upscale_method,
        wgpu_downscale_method,
        output_origin,
        target_size,
        [0, 0],
        target_size,
        opacity,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn params_for_effects_with_display(
    source_size: [usize; 2],
    output_size: [usize; 2],
    effects: ViewEffects,
    wgpu_upscale_method: WgpuUpscaleMethod,
    wgpu_downscale_method: WgpuDownscaleMethod,
    output_origin: [u32; 2],
    visible_target_size: [u32; 2],
    sample_offset: [u32; 2],
    full_target_size: [u32; 2],
    opacity: f32,
) -> EffectParams {
    params_for_effects_with_shader_method_and_display(
        source_size,
        output_size,
        effects,
        wgpu_upscale_method.shader_method_id(),
        wgpu_downscale_method.shader_method_id(),
        output_origin,
        visible_target_size,
        sample_offset,
        full_target_size,
        opacity,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn params_for_effects_with_shader_method(
    source_size: [usize; 2],
    output_size: [usize; 2],
    effects: ViewEffects,
    shader_method_id: u32,
    downscale_method_id: u32,
    output_origin: [u32; 2],
    target_size: [u32; 2],
    opacity: f32,
) -> EffectParams {
    params_for_effects_with_shader_method_and_display(
        source_size,
        output_size,
        effects,
        shader_method_id,
        downscale_method_id,
        output_origin,
        target_size,
        [0, 0],
        target_size,
        opacity,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn params_for_effects_with_shader_method_and_display(
    source_size: [usize; 2],
    output_size: [usize; 2],
    effects: ViewEffects,
    shader_method_id: u32,
    downscale_method_id: u32,
    output_origin: [u32; 2],
    visible_target_size: [u32; 2],
    sample_offset: [u32; 2],
    full_target_size: [u32; 2],
    opacity: f32,
) -> EffectParams {
    let filter = match effects.filter {
        ImageFilter::None => 0,
        ImageFilter::Smooth => 1,
        ImageFilter::SmoothSharpen => 2,
        ImageFilter::RcasSharpen => 3,
    };
    let params = EffectParams {
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
        upscale: [shader_method_id, downscale_method_id, 0, 0],
        opacity: [
            opacity,
            visible_target_size[0].max(1) as f32,
            visible_target_size[1].max(1) as f32,
            0.0,
        ],
        display: [
            sample_offset[0] as f32,
            sample_offset[1] as f32,
            full_target_size[0].max(1) as f32,
            full_target_size[1].max(1) as f32,
        ],
    };
    // V13: tag downscale draws for linear-light averaging. The condition mirrors
    // the shader's own downscale routing in `sample_display` exactly — a downscale
    // filter is selected (`downscale_method_id != 0`) and the target is smaller
    // than the effect output in at least one axis — so the flag is set precisely
    // when a weighted-sum downscale leg runs. Upscale/native/mixed draws (target
    // not smaller) and the hardware-mipmap path (built elsewhere) leave it clear.
    let is_downscale = downscale_method_id != 0
        && (full_target_size[0] < output_size[0] as u32
            || full_target_size[1] < output_size[1] as u32);
    if is_downscale && linear_downscale_enabled() {
        params.with_linear_downscale()
    } else {
        params
    }
}

#[allow(dead_code)]
pub(crate) fn params_for_hardware_mipmap_sample(
    source_size: [usize; 2],
    output_origin: [u32; 2],
    target_size: [u32; 2],
    opacity: f32,
    lod: f32,
) -> EffectParams {
    params_for_hardware_mipmap_sample_with_display(
        source_size,
        output_origin,
        target_size,
        [0, 0],
        target_size,
        opacity,
        lod,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn params_for_hardware_mipmap_sample_with_display(
    source_size: [usize; 2],
    output_origin: [u32; 2],
    visible_target_size: [u32; 2],
    sample_offset: [u32; 2],
    full_target_size: [u32; 2],
    opacity: f32,
    lod: f32,
) -> EffectParams {
    EffectParams {
        source_output: [
            source_size[0] as u32,
            source_size[1] as u32,
            source_size[0] as u32,
            source_size[1] as u32,
        ],
        transform_filter: [0, 0, 0, 0],
        color_origin: [0, 0, output_origin[0], output_origin[1]],
        upscale: [0, 0, 1, 0],
        opacity: [
            opacity,
            visible_target_size[0].max(1) as f32,
            visible_target_size[1].max(1) as f32,
            lod.max(0.0),
        ],
        display: [
            sample_offset[0] as f32,
            sample_offset[1] as f32,
            full_target_size[0].max(1) as f32,
            full_target_size[1].max(1) as f32,
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
        bytes.extend_from_slice(&pixel.to_srgba_unmultiplied());
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
    use super::{align_to, color_image_to_rgba, output_size_for_effects};
    use crate::core::effects::{ViewEffects, ViewTransform};
    use egui::{Color32, ColorImage};

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

    #[test]
    fn color_image_to_rgba_returns_unmultiplied_channels() {
        let image = ColorImage::new(
            [1, 1],
            vec![Color32::from_rgba_unmultiplied(255, 0, 0, 128)],
        );

        assert_eq!(color_image_to_rgba(&image), vec![255, 0, 0, 128]);
    }

    // Rust mirror of the exact piecewise sRGB transfer pair in
    // src/core/gpu_effect.wgsl. Pins the math the shader's linear-light downscale
    // legs must match, without a GPU adapter.
    fn srgb_to_linear(c: f64) -> f64 {
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    }

    fn linear_to_srgb(c: f64) -> f64 {
        if c <= 0.0031308 {
            c * 12.92
        } else {
            1.055 * c.powf(1.0 / 2.4) - 0.055
        }
    }

    #[test]
    fn srgb_transfer_pair_round_trips() {
        // Sweep the [0,1] range: OETF∘EOTF and EOTF∘OETF are inverses to < 1e-5.
        for step in 0..=1000 {
            let v = step as f64 / 1000.0;
            assert!(
                (linear_to_srgb(srgb_to_linear(v)) - v).abs() < 1e-5,
                "srgb->linear->srgb drifted at {v}"
            );
            assert!(
                (srgb_to_linear(linear_to_srgb(v)) - v).abs() < 1e-5,
                "linear->srgb->linear drifted at {v}"
            );
        }
    }

    #[test]
    fn srgb_transfer_pair_hits_known_anchors() {
        // 0.5 linear encodes to ~0.7354 sRGB (the physically-correct mean light
        // of a 50/50 black/white mix — the whole point of linear-light downscale).
        assert!((linear_to_srgb(0.5) - 0.735_36).abs() < 1e-4);
        // Mid sRGB decodes well below its code value (0.5 -> ~0.214 light).
        assert!((srgb_to_linear(0.5) - 0.214_04).abs() < 1e-4);
        // Endpoints are exact fixed points.
        assert_eq!(srgb_to_linear(0.0), 0.0);
        assert!((linear_to_srgb(1.0) - 1.0).abs() < 1e-9);
    }
}
