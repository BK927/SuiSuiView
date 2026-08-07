use crate::core::artcnn::ArtcnnVariant;
use crate::core::gpu_effect::color_image_to_rgba;
use crate::core::state::WgpuUpscaleMethod;
use egui::ColorImage;
use std::borrow::Cow;
use std::sync::mpsc;
use std::time::{Duration, Instant};
use wgpu::util::DeviceExt;

mod acnet;
mod acnet_manifest;
mod anime4k;
mod anime4k_m;
mod artcnn;
mod cunny;
mod cunny_tables;
mod nvidia_nis;
mod span;
use acnet::AcnetBench;
use anime4k::Anime4kBench;
use anime4k_m::Anime4kMBench;
use artcnn::ArtcnnBench;
use cunny::CunnyBench;
use nvidia_nis::NvidiaNisBench;
use span::SpanBench;

const TEXTURE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct UpscaleParams {
    source_output: [u32; 4],
    method: [u32; 4],
}

pub(crate) struct GpuUpscaleBench {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    nvidia_nis: Option<NvidiaNisBench>,
    anime4k: Option<Anime4kBench>,
    anime4k_m: Option<Anime4kMBench>,
    artcnn: Vec<ArtcnnBench>,
    span: Option<SpanBench>,
    acnet: Option<AcnetBench>,
    cunny: Option<CunnyBench>,
}

/// Sentinel `slot` marking the chain's visible 2x output rather than one of the
/// numbered intermediates.
pub(crate) const FINAL_OUTPUT_SLOT: usize = usize::MAX;

/// Read a whole texture back to RGBA bytes, unpadding the copy rows. Diagnostic
/// path only: it stalls the GPU.
fn read_texture_bytes(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    width: u32,
    height: u32,
) -> Result<Vec<u8>, String> {
    let padded_bytes_per_row = align_to(width * 4, wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("suisuiview-cunny-stage-readback"),
        size: padded_bytes_per_row as u64 * height as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("suisuiview-cunny-stage-readback-encoder"),
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_bytes_per_row),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(Some(encoder.finish()));

    let slice = buffer.slice(..);
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

    let mapped = slice.get_mapped_range();
    let row_bytes = width as usize * 4;
    let mut pixels = vec![0_u8; row_bytes * height as usize];
    for y in 0..height as usize {
        let src = y * padded_bytes_per_row as usize;
        let dst = y * row_bytes;
        pixels[dst..dst + row_bytes].copy_from_slice(&mapped[src..src + row_bytes]);
    }
    drop(mapped);
    buffer.unmap();
    Ok(pixels)
}

/// Summary of one texture's bytes. Enough to spot the ways a mis-ported
/// convolution chain fails — saturation at either rail, a collapse to a constant,
/// or a dead (all-zero) plane — without moving whole images around.
#[derive(Debug, Clone, Copy, serde::Serialize)]
pub(crate) struct ByteStats {
    pub(crate) mean: f64,
    pub(crate) std_dev: f64,
    pub(crate) min: u8,
    pub(crate) max: u8,
    /// Fraction of bytes at exactly 0 and exactly 255. A healthy activation map
    /// keeps both small; a broken one pins to a rail.
    pub(crate) zero_ratio: f64,
    pub(crate) saturated_ratio: f64,
}

impl ByteStats {
    pub(crate) fn of(bytes: &[u8]) -> Self {
        if bytes.is_empty() {
            return Self {
                mean: 0.0,
                std_dev: 0.0,
                min: 0,
                max: 0,
                zero_ratio: 0.0,
                saturated_ratio: 0.0,
            };
        }
        let count = bytes.len() as f64;
        let mut sum = 0.0;
        let mut sum_sq = 0.0;
        let mut min = u8::MAX;
        let mut max = u8::MIN;
        let mut zeros = 0u64;
        let mut saturated = 0u64;
        for &byte in bytes {
            let value = byte as f64;
            sum += value;
            sum_sq += value * value;
            min = min.min(byte);
            max = max.max(byte);
            if byte == 0 {
                zeros += 1;
            } else if byte == u8::MAX {
                saturated += 1;
            }
        }
        let mean = sum / count;
        Self {
            mean,
            std_dev: (sum_sq / count - mean * mean).max(0.0).sqrt(),
            min,
            max,
            zero_ratio: zeros as f64 / count,
            saturated_ratio: saturated as f64 / count,
        }
    }
}

/// One intermediate texture as it stood right after the pass that wrote it.
#[derive(Debug, Clone, Copy, serde::Serialize)]
pub(crate) struct CunnyStageSample {
    pub(crate) pass: usize,
    pub(crate) slot: usize,
    pub(crate) stats: ByteStats,
}

pub(crate) struct GpuUpscaleOutput {
    pub(crate) image: ColorImage,
    pub(crate) elapsed: Duration,
}

/// Which sub-benches a constructor should compile. `All` reproduces the historical
/// "no filter" behavior (heavy benchmark-only families such as ArtCNN, SPAN, and CuNNy
/// stay out); `Only` compiles a sub-bench iff any listed method needs it.
enum MethodFilter<'a> {
    All,
    Only(&'a [WgpuUpscaleMethod]),
}

impl MethodFilter<'_> {
    /// Compiled for `All`, or when the exact method is requested.
    fn wants_method(&self, method: WgpuUpscaleMethod) -> bool {
        match self {
            MethodFilter::All => true,
            MethodFilter::Only(methods) => methods.contains(&method),
        }
    }

    /// Compiled for `All`, or when any requested method matches the group predicate.
    fn wants_group(&self, predicate: impl Fn(WgpuUpscaleMethod) -> bool) -> bool {
        match self {
            MethodFilter::All => true,
            MethodFilter::Only(methods) => methods.iter().copied().any(predicate),
        }
    }

    /// Never compiled for `All` (benchmark-only heavy family); compiled only when a
    /// requested method matches the predicate.
    fn wants_explicit(&self, predicate: impl Fn(WgpuUpscaleMethod) -> bool) -> bool {
        match self {
            MethodFilter::All => false,
            MethodFilter::Only(methods) => methods.iter().copied().any(predicate),
        }
    }

    /// First requested method matching the predicate, or None for `All`.
    fn first(&self, predicate: impl Fn(WgpuUpscaleMethod) -> bool) -> Option<WgpuUpscaleMethod> {
        match self {
            MethodFilter::All => None,
            MethodFilter::Only(methods) => {
                methods.iter().copied().find(|method| predicate(*method))
            }
        }
    }
}

impl GpuUpscaleBench {
    pub(crate) fn new_for_method(method: Option<WgpuUpscaleMethod>) -> Result<Self, String> {
        match method {
            Some(method) => Self::new_for_methods(std::slice::from_ref(&method)),
            None => pollster::block_on(Self::new_async(MethodFilter::All)),
        }
    }

    /// Build one device/queue and compile the base pipeline plus only the sub-benches any
    /// of `methods` needs. Used by the per-book upscale probe, which passes a two-method set.
    pub(crate) fn new_for_methods(methods: &[WgpuUpscaleMethod]) -> Result<Self, String> {
        pollster::block_on(Self::new_async(MethodFilter::Only(methods)))
    }

    async fn new_async(filter: MethodFilter<'_>) -> Result<Self, String> {
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
                label: Some("suisuiview-upscale-bench-device"),
                required_features: wgpu::Features::empty(),
                // Compute SR candidates use storage textures; WebGL2-style limits report none.
                required_limits: wgpu::Limits::default().using_resolution(adapter.limits()),
                memory_hints: wgpu::MemoryHints::Performance,
                trace: wgpu::Trace::Off,
            })
            .await
            .map_err(|error| format!("wgpu device unavailable: {error}"))?;

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("suisuiview-upscale-bench-shader"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(include_str!("../gpu_upscale.wgsl"))),
        });
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("suisuiview-upscale-bench-bind-group-layout"),
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
            label: Some("suisuiview-upscale-bench-pipeline-layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("suisuiview-upscale-bench-pipeline"),
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

        let nvidia_nis = if filter.wants_method(WgpuUpscaleMethod::NvidiaNis) {
            NvidiaNisBench::try_new(&device).await
        } else {
            None
        };
        let anime4k = if filter.wants_method(WgpuUpscaleMethod::WgslAnime4kV32CnnX2S) {
            Anime4kBench::try_new(&device).await
        } else {
            None
        };
        let anime4k_m = if filter.wants_method(WgpuUpscaleMethod::WgslAnime4kV32CnnX2M) {
            Anime4kMBench::try_new(&device).await
        } else {
            None
        };
        let mut artcnn = Vec::new();
        for variant in ArtcnnVariant::ALL {
            if filter
                .wants_explicit(|method| ArtcnnBench::variant_for_method(method) == Some(variant))
            {
                if let Some(bench) = ArtcnnBench::try_new(&device, variant).await {
                    artcnn.push(bench);
                }
            }
        }
        let span = if filter.wants_explicit(|method| method == WgpuUpscaleMethod::WgslSrLabSpanX2) {
            SpanBench::try_new()
        } else {
            None
        };
        let acnet = if filter.wants_group(is_acnet_method) {
            AcnetBench::try_new(&device).await
        } else {
            None
        };
        let cunny_method = filter.first(WgpuUpscaleMethod::is_cunny);
        let cunny = if cunny_method.is_some() {
            CunnyBench::try_new(&device, cunny_method).await
        } else {
            None
        };

        Ok(Self {
            device,
            queue,
            pipeline,
            bind_group_layout,
            nvidia_nis,
            anime4k,
            anime4k_m,
            artcnn,
            span,
            acnet,
            cunny,
        })
    }

    /// Per-pass activation statistics for one CuNNy variant (see
    /// [`cunny::CunnyBench::stage_stats`]). Only the CuNNy family is chained
    /// deeply enough for this to say anything, so other methods are rejected
    /// rather than silently returning an empty run.
    pub(crate) fn cunny_stage_stats(
        &self,
        method: WgpuUpscaleMethod,
        image: &ColorImage,
    ) -> Result<Vec<CunnyStageSample>, String> {
        if !method.is_cunny() {
            return Err(format!(
                "{} is not a CuNNy variant; stage stats only apply to the CuNNy chain",
                method.label()
            ));
        }
        self.cunny
            .as_ref()
            .ok_or_else(|| "CuNNy GPU pipelines unavailable".to_owned())?
            .stage_stats(method, &self.device, &self.queue, image)
    }

    pub(crate) fn apply(
        &self,
        image: &ColorImage,
        output_size: [usize; 2],
        method: WgpuUpscaleMethod,
    ) -> Result<GpuUpscaleOutput, String> {
        let [source_width, source_height] = image.size;
        let [output_width, output_height] = output_size;
        if source_width == 0 || source_height == 0 || output_width == 0 || output_height == 0 {
            return Err("cannot upscale an empty image".to_owned());
        }
        if method == WgpuUpscaleMethod::WgslAnime4kV32CnnX2S {
            return self
                .anime4k
                .as_ref()
                .ok_or_else(|| "Anime4K v3.2 CNN x2 S GPU pipelines unavailable".to_owned())?
                .apply(&self.device, &self.queue, image, output_size);
        }
        if method == WgpuUpscaleMethod::WgslAnime4kV32CnnX2M {
            return self
                .anime4k_m
                .as_ref()
                .ok_or_else(|| "Anime4K v3.2 CNN x2 M GPU pipelines unavailable".to_owned())?
                .apply(&self.device, &self.queue, image, output_size);
        }
        if let Some(variant) = ArtcnnBench::variant_for_method(method) {
            return self
                .artcnn
                .iter()
                .find(|bench| bench.variant() == variant)
                .ok_or_else(|| format!("{} GPU pipelines unavailable", variant.label()))?
                .apply(&self.device, &self.queue, image, output_size);
        }
        if method == WgpuUpscaleMethod::WgslSrLabSpanX2 {
            return self
                .span
                .as_ref()
                .ok_or_else(|| "SR Lab SPAN x2 GPU pipeline unavailable".to_owned())?
                .apply(image, output_size)
                .map(|output| GpuUpscaleOutput {
                    image: output.image,
                    elapsed: output.elapsed,
                });
        }
        if method == WgpuUpscaleMethod::NvidiaNis {
            return self
                .nvidia_nis
                .as_ref()
                .ok_or_else(|| "NVIDIA Image Scaling GPU pipeline unavailable".to_owned())?
                .apply(&self.device, &self.queue, image, output_size);
        }
        if matches!(
            method,
            WgpuUpscaleMethod::WgslAcnetF8B4Luma
                | WgpuUpscaleMethod::WgslAcnetF8B4BoxLuma
                | WgpuUpscaleMethod::WgslAcnetF8B4HdnLuma
                | WgpuUpscaleMethod::WgslAcnetF8B4BoxHdnLuma
        ) {
            return self
                .acnet
                .as_ref()
                .ok_or_else(|| "ACNet F8B4 luma GPU pipelines unavailable".to_owned())?
                .apply(method, &self.device, &self.queue, image, output_size);
        }
        if matches!(
            method,
            WgpuUpscaleMethod::CunnyVeryfastNvl
                | WgpuUpscaleMethod::CunnyVeryfastSoft
                | WgpuUpscaleMethod::CunnyFasterNvl
                | WgpuUpscaleMethod::CunnyFasterSoft
                | WgpuUpscaleMethod::CunnyFasterDs
                | WgpuUpscaleMethod::CunnyFastNvl
                | WgpuUpscaleMethod::CunnyFastSoft
                | WgpuUpscaleMethod::CunnyFastDs
                | WgpuUpscaleMethod::Cunny2x12Soft
                | WgpuUpscaleMethod::Cunny2x12Ds
                | WgpuUpscaleMethod::Cunny3x12Nvl
                | WgpuUpscaleMethod::Cunny3x12Soft
                | WgpuUpscaleMethod::Cunny3x12Ds
                | WgpuUpscaleMethod::Cunny4x12Nvl
                | WgpuUpscaleMethod::Cunny4x12Soft
                | WgpuUpscaleMethod::Cunny4x12Ds
                | WgpuUpscaleMethod::Cunny4x16Nvl
                | WgpuUpscaleMethod::Cunny4x16Soft
                | WgpuUpscaleMethod::Cunny4x16Ds
                | WgpuUpscaleMethod::Cunny4x24Nvl
                | WgpuUpscaleMethod::Cunny4x24Soft
                | WgpuUpscaleMethod::Cunny4x24Ds
                | WgpuUpscaleMethod::Cunny4x32Nvl
                | WgpuUpscaleMethod::Cunny4x32Soft
                | WgpuUpscaleMethod::Cunny4x32Ds
                | WgpuUpscaleMethod::Cunny8x32Nvl
                | WgpuUpscaleMethod::Cunny8x32Ds
        ) {
            return self
                .cunny
                .as_ref()
                .ok_or_else(|| "CuNNy NVL GPU pipelines unavailable".to_owned())?
                .apply(method, &self.device, &self.queue, image, output_size);
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

        let source_texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("suisuiview-upscale-source"),
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
            label: Some("suisuiview-upscale-output"),
            size: output_extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: TEXTURE_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let source_view = source_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let padded_bytes_per_row = align_to(
            (output_width * 4) as u32,
            wgpu::COPY_BYTES_PER_ROW_ALIGNMENT,
        );
        let output_buffer_size = padded_bytes_per_row as u64 * output_height as u64;
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("suisuiview-upscale-readback"),
            size: output_buffer_size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("suisuiview-upscale-encoder"),
            });

        if let Some(rcas_method_id) = method.rcas_shader_method_id() {
            let intermediate_texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("suisuiview-upscale-intermediate"),
                size: output_extent,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: TEXTURE_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let intermediate_view =
                intermediate_texture.create_view(&wgpu::TextureViewDescriptor::default());
            self.render_pass(
                &mut encoder,
                &source_view,
                [source_width as u32, source_height as u32],
                [output_width as u32, output_height as u32],
                method.shader_method_id(),
                &intermediate_view,
            );

            let output_view = output_texture.create_view(&wgpu::TextureViewDescriptor::default());
            self.render_pass(
                &mut encoder,
                &intermediate_view,
                [output_width as u32, output_height as u32],
                [output_width as u32, output_height as u32],
                rcas_method_id,
                &output_view,
            );
        } else {
            let output_view = output_texture.create_view(&wgpu::TextureViewDescriptor::default());
            self.render_pass(
                &mut encoder,
                &source_view,
                [source_width as u32, source_height as u32],
                [output_width as u32, output_height as u32],
                method.shader_method_id(),
                &output_view,
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

        Ok(GpuUpscaleOutput {
            image: ColorImage::from_rgba_unmultiplied(output_size, &output_bytes),
            elapsed,
        })
    }

    fn render_pass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        source_view: &wgpu::TextureView,
        source_size: [u32; 2],
        output_size: [u32; 2],
        method_id: u32,
        output_view: &wgpu::TextureView,
    ) {
        let params = UpscaleParams {
            source_output: [
                source_size[0],
                source_size[1],
                output_size[0],
                output_size[1],
            ],
            method: [method_id, 0, 0, 0],
        };
        let params_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("suisuiview-upscale-params"),
                contents: bytemuck::bytes_of(&params),
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("suisuiview-upscale-bind-group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(source_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: params_buffer.as_entire_binding(),
                },
            ],
        });

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("suisuiview-upscale-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: output_view,
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
}

fn is_acnet_method(method: WgpuUpscaleMethod) -> bool {
    matches!(
        method,
        WgpuUpscaleMethod::WgslAcnetF8B4Luma
            | WgpuUpscaleMethod::WgslAcnetF8B4BoxLuma
            | WgpuUpscaleMethod::WgslAcnetF8B4HdnLuma
            | WgpuUpscaleMethod::WgslAcnetF8B4BoxHdnLuma
    )
}

fn align_to(value: u32, alignment: u32) -> u32 {
    value.div_ceil(alignment) * alignment
}
