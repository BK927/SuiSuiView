use crate::core::effects::ViewEffects;
use crate::core::gpu_effect::{
    color_image_to_rgba, params_for_effects, params_for_effects_with_shader_method, EffectParams,
};
use crate::core::source::open_source_from_path;
use crate::core::state::{CpuScaleFilter, DisplayUpscaler, WgpuDownscaler};
use crate::core::worker::{
    clamp_target_long_edge, prepare_image_with_options, DecodeOptions, DecodeStrategy, PreparedPage,
};
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use wgpu::util::DeviceExt;

mod stats;

use stats::{SampleStats, SummarySet};

const DEFAULT_ITERATIONS: usize = 30;
const DEFAULT_MAX_PAGES: usize = 8;
const TEXTURE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

#[derive(Debug, Serialize)]
pub struct GpuCopyBenchReport {
    pub path: String,
    pub title: String,
    pub page_count: usize,
    pub sampled_pages: usize,
    pub target_long_edge: u32,
    pub iterations: usize,
    pub max_pages: usize,
    pub gpu_available: bool,
    pub gpu_error: Option<String>,
    pub adapter: Option<String>,
    pub failures: usize,
    pub summaries: Vec<GpuCopyBenchSummary>,
    pub pages: Vec<PageGpuCopyBench>,
}

#[derive(Debug, Serialize)]
pub struct GpuCopyBenchSummary {
    pub case: String,
    pub samples: usize,
    pub avg_ms: f64,
    pub median_ms: f64,
    pub p95_ms: f64,
    pub max_ms: f64,
    pub avg_bytes: usize,
    pub avg_bind_group_creates: f64,
    pub avg_uniform_buffer_creates: f64,
    pub interpretation: String,
}

#[derive(Debug, Serialize)]
pub struct PageGpuCopyBench {
    pub index: usize,
    pub name: String,
    pub width: Option<usize>,
    pub height: Option<usize>,
    pub bytes: Option<usize>,
    pub cases: Vec<GpuCopyBenchCase>,
    pub error: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
pub struct GpuCopyBenchCase {
    pub case: String,
    pub samples: usize,
    #[serde(skip_serializing)]
    raw_samples_ms: Vec<f64>,
    pub avg_ms: f64,
    pub median_ms: f64,
    pub p95_ms: f64,
    pub max_ms: f64,
    pub bytes: usize,
    pub bind_group_creates_per_iteration: usize,
    pub uniform_buffer_creates_per_iteration: usize,
}

pub fn run_gpu_copy_bench(
    path: &Path,
    report_path: Option<&Path>,
    target_long_edge: u32,
    iterations: usize,
    max_pages: usize,
) -> Result<(), String> {
    let report = scan_gpu_copy_costs(
        path,
        clamp_target_long_edge(target_long_edge),
        iterations.max(1),
        max_pages.max(1),
    )?;
    print_report(&report);
    if let Some(report_path) = report_path {
        write_report(report_path, &report)?;
        println!("Report: {}", report_path.display());
    }
    Ok(())
}

pub fn scan_gpu_copy_costs(
    path: &Path,
    target_long_edge: u32,
    iterations: usize,
    max_pages: usize,
) -> Result<GpuCopyBenchReport, String> {
    let (source, _forced_page) = open_source_from_path(path).map_err(|error| error.to_string())?;
    let gpu = GpuCopyBench::new()?;
    let adapter = Some(gpu.adapter.clone());

    let limit = source.page_count().min(max_pages);
    let mut pages = Vec::with_capacity(limit);
    let mut summaries = SummarySet::default();
    let mut failures = 0usize;

    for index in 0..limit {
        let mut page = PageGpuCopyBench {
            index,
            name: source.page_name(index).unwrap_or("").to_owned(),
            width: None,
            height: None,
            bytes: None,
            cases: Vec::new(),
            error: None,
        };
        let result = source
            .read_page(index)
            .map_err(|error| error.to_string())
            .and_then(|bytes| prepare_page(&bytes, target_long_edge));

        match result {
            Ok(prepared) => {
                let [width, height] = prepared.image_size();
                page.width = Some(width);
                page.height = Some(height);
                page.bytes = Some(width.saturating_mul(height).saturating_mul(4));
                match gpu.measure_page(&prepared, iterations) {
                    Ok(cases) => {
                        for case in cases {
                            summaries.push(&case);
                            page.cases.push(case);
                        }
                    }
                    Err(error) => {
                        failures += 1;
                        page.error = Some(error);
                    }
                }
            }
            Err(error) => {
                failures += 1;
                page.error = Some(error);
            }
        }
        pages.push(page);
    }

    Ok(GpuCopyBenchReport {
        path: path.display().to_string(),
        title: source.title().to_owned(),
        page_count: source.page_count(),
        sampled_pages: pages.len(),
        target_long_edge,
        iterations,
        max_pages,
        gpu_available: true,
        gpu_error: None,
        adapter,
        failures,
        summaries: summaries.finish(),
        pages,
    })
}

pub fn default_gpu_copy_report_path() -> PathBuf {
    PathBuf::from("perf-fixtures").join("gpu-copy-report.json")
}

pub fn default_gpu_copy_iterations() -> usize {
    DEFAULT_ITERATIONS
}

pub fn default_gpu_copy_max_pages() -> usize {
    DEFAULT_MAX_PAGES
}

fn prepare_page(bytes: &[u8], target_long_edge: u32) -> Result<Arc<PreparedPage>, String> {
    let page = Arc::new(prepare_image_with_options(
        bytes,
        target_long_edge,
        DecodeOptions {
            strategy: DecodeStrategy::Auto,
            cpu_downscaler: CpuScaleFilter::Lanczos3,
            allow_display_upscale: false,
            ..DecodeOptions::default()
        },
    )?);
    Ok(page)
}

struct GpuCopyBench {
    device: wgpu::Device,
    queue: wgpu::Queue,
    adapter: String,
    pipeline: wgpu::RenderPipeline,
    texture_bind_group_layout: wgpu::BindGroupLayout,
    params_bind_group_layout: wgpu::BindGroupLayout,
    legacy_bind_group_layout: wgpu::BindGroupLayout,
    texture_sampler: wgpu::Sampler,
}

impl GpuCopyBench {
    fn new() -> Result<Self, String> {
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
        let adapter_name = adapter.get_info().name;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("suisuiview-gpu-copy-bench-device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::downlevel_webgl2_defaults()
                    .using_resolution(adapter.limits()),
                memory_hints: wgpu::MemoryHints::Performance,
                trace: wgpu::Trace::Off,
            })
            .await
            .map_err(|error| format!("wgpu device unavailable: {error}"))?;

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("suisuiview-gpu-copy-bench-shader"),
            source: wgpu::ShaderSource::Wgsl(std::borrow::Cow::Borrowed(include_str!(
                "gpu_effect.wgsl"
            ))),
        });
        let texture_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("suisuiview-gpu-copy-bench-texture-layout"),
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
                label: Some("suisuiview-gpu-copy-bench-params-layout"),
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
        let legacy_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("suisuiview-gpu-copy-bench-legacy-layout"),
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
            label: Some("suisuiview-gpu-copy-bench-pipeline-layout"),
            bind_group_layouts: &[&texture_bind_group_layout, &params_bind_group_layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("suisuiview-gpu-copy-bench-pipeline"),
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
            label: Some("suisuiview-gpu-copy-linear-sampler"),
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
            adapter: adapter_name,
            pipeline,
            texture_bind_group_layout,
            params_bind_group_layout,
            legacy_bind_group_layout,
            texture_sampler,
        })
    }

    fn measure_page(
        &self,
        page: &PreparedPage,
        iterations: usize,
    ) -> Result<Vec<GpuCopyBenchCase>, String> {
        let image = page.color_image();
        let bytes = &page.rgba;
        let [width, height] = image.size;
        if width == 0 || height == 0 {
            return Err("cannot benchmark an empty image".to_owned());
        }
        let byte_size = bytes.len();
        let mut cases = Vec::with_capacity(8);

        cases.push(
            self.measure_case("color_image_to_rgba", byte_size, iterations, || {
                let bytes = color_image_to_rgba(&image);
                std::hint::black_box(bytes.len());
            })?,
        );
        cases.push(self.measure_case(
            "source_texture_create_view",
            byte_size,
            iterations,
            || {
                let texture = self.create_source_texture(width, height);
                let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                std::hint::black_box(&view);
            },
        )?);
        let reused_texture = self.create_source_texture(width, height);
        cases.push(self.measure_case(
            "write_texture_reused_texture",
            byte_size,
            iterations,
            || {
                self.write_texture(&reused_texture, width, height, bytes.as_ref());
            },
        )?);
        cases.push(
            self.measure_case("precomputed_first_upload", byte_size, iterations, || {
                let texture = self.create_source_texture(width, height);
                self.write_texture(&texture, width, height, bytes.as_ref());
                let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                std::hint::black_box(&view);
            })?,
        );
        let source_texture_for_bind_group = self.create_source_texture(width, height);
        let source_view_for_bind_group =
            source_texture_for_bind_group.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group_params = params_for_effects(
            [width, height],
            [width, height],
            ViewEffects::default(),
            DisplayUpscaler::None,
            WgpuDownscaler::Bilinear,
            [0, 0],
            [width as u32, height as u32],
            1.0,
        );
        cases.push(self.measure_case_with_counts(
            "legacy_combined_bind_group_create",
            byte_size,
            iterations,
            1,
            1,
            || {
                let bind_group =
                    self.legacy_bind_group_for(&source_view_for_bind_group, bind_group_params);
                std::hint::black_box(&bind_group);
            },
        )?);
        cases.push(self.measure_case_with_counts(
            "texture_bind_group_create",
            byte_size,
            iterations,
            1,
            0,
            || {
                let bind_group = self.texture_bind_group_for(&source_view_for_bind_group);
                std::hint::black_box(&bind_group);
            },
        )?);
        cases.push(self.measure_case_with_counts(
            "params_bind_group_create",
            byte_size,
            iterations,
            1,
            1,
            || {
                let bind_group = self.params_bind_group_for(bind_group_params);
                std::hint::black_box(&bind_group);
            },
        )?);
        cases.push(
            self.measure_case("current_first_upload", byte_size, iterations, || {
                let texture = self.create_source_texture(width, height);
                let bytes = color_image_to_rgba(&image);
                self.write_texture(&texture, width, height, &bytes);
                let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                std::hint::black_box(&view);
            })?,
        );
        cases.push(self.measure_case(
            "fsr1_intermediate_texture_create_view",
            byte_size,
            iterations,
            || {
                let texture = self.create_intermediate_texture(width, height);
                let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                std::hint::black_box(&view);
            },
        )?);
        let source_texture = self.create_source_texture(width, height);
        self.write_texture(&source_texture, width, height, bytes.as_ref());
        self.flush()?;
        let source_view = source_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let source_bind_group = self.texture_bind_group_for(&source_view);
        let output_texture = self.create_output_texture(width, height);
        let output_view = output_texture.create_view(&wgpu::TextureViewDescriptor::default());
        cases.push(self.measure_case_with_counts(
            "fsr1_twopass_recreate_intermediate",
            byte_size,
            iterations,
            3,
            2,
            || {
                let intermediate = self.create_intermediate_texture(width, height);
                let intermediate_view =
                    intermediate.create_view(&wgpu::TextureViewDescriptor::default());
                let intermediate_bind_group = self.texture_bind_group_for(&intermediate_view);
                self.render_fsr1_twopass(
                    &source_bind_group,
                    &intermediate_bind_group,
                    [width, height],
                    [width as u32, height as u32],
                    &intermediate_view,
                    &output_view,
                );
            },
        )?);
        let reused_intermediate = self.create_intermediate_texture(width, height);
        let reused_intermediate_view =
            reused_intermediate.create_view(&wgpu::TextureViewDescriptor::default());
        let reused_intermediate_bind_group = self.texture_bind_group_for(&reused_intermediate_view);
        cases.push(self.measure_case_with_counts(
            "fsr1_twopass_reuse_intermediate",
            byte_size,
            iterations,
            2,
            2,
            || {
                self.render_fsr1_twopass(
                    &source_bind_group,
                    &reused_intermediate_bind_group,
                    [width, height],
                    [width as u32, height as u32],
                    &reused_intermediate_view,
                    &output_view,
                );
            },
        )?);

        Ok(cases)
    }

    fn measure_case<F>(
        &self,
        label: &str,
        bytes: usize,
        iterations: usize,
        run: F,
    ) -> Result<GpuCopyBenchCase, String>
    where
        F: FnMut(),
    {
        self.measure_case_with_counts(label, bytes, iterations, 0, 0, run)
    }

    fn measure_case_with_counts<F>(
        &self,
        label: &str,
        bytes: usize,
        iterations: usize,
        bind_group_creates_per_iteration: usize,
        uniform_buffer_creates_per_iteration: usize,
        mut run: F,
    ) -> Result<GpuCopyBenchCase, String>
    where
        F: FnMut(),
    {
        for _ in 0..iterations.min(3) {
            run();
            self.flush()?;
        }

        let mut samples = Vec::with_capacity(iterations);
        for _ in 0..iterations {
            let started = Instant::now();
            run();
            samples.push(millis(started.elapsed()));
            self.flush()?;
        }

        let stats = SampleStats::from_samples(samples);
        Ok(GpuCopyBenchCase {
            case: label.to_owned(),
            samples: stats.samples,
            raw_samples_ms: stats.samples_ms,
            avg_ms: stats.avg_ms,
            median_ms: stats.median_ms,
            p95_ms: stats.p95_ms,
            max_ms: stats.max_ms,
            bytes,
            bind_group_creates_per_iteration,
            uniform_buffer_creates_per_iteration,
        })
    }

    fn create_source_texture(&self, width: usize, height: usize) -> wgpu::Texture {
        self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("suisuiview-gpu-copy-source"),
            size: extent(width, height),
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: TEXTURE_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        })
    }

    fn create_intermediate_texture(&self, width: usize, height: usize) -> wgpu::Texture {
        self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("suisuiview-gpu-copy-intermediate"),
            size: extent(width, height),
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: TEXTURE_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
    }

    fn create_output_texture(&self, width: usize, height: usize) -> wgpu::Texture {
        self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("suisuiview-gpu-copy-output"),
            size: extent(width, height),
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: TEXTURE_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
    }

    fn write_texture(&self, texture: &wgpu::Texture, width: usize, height: usize, bytes: &[u8]) {
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some((width * 4) as u32),
                rows_per_image: Some(height as u32),
            },
            extent(width, height),
        );
    }

    fn render_fsr1_twopass(
        &self,
        source_bind_group: &wgpu::BindGroup,
        intermediate_bind_group: &wgpu::BindGroup,
        source_size: [usize; 2],
        target_size: [u32; 2],
        intermediate_view: &wgpu::TextureView,
        output_view: &wgpu::TextureView,
    ) {
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("suisuiview-gpu-copy-fsr1-encoder"),
            });
        let easu_params = params_for_effects(
            source_size,
            source_size,
            ViewEffects::default(),
            DisplayUpscaler::WgslFsr1EasuRcas,
            WgpuDownscaler::Bilinear,
            [0, 0],
            target_size,
            1.0,
        );
        let easu_params_bind_group = self.params_bind_group_for(easu_params);
        self.render_pass(
            &mut encoder,
            intermediate_view,
            source_bind_group,
            &easu_params_bind_group,
        );

        let rcas_method = DisplayUpscaler::WgslFsr1EasuRcas
            .rcas_shader_method_id()
            .expect("FSR1 EASU+RCAS should expose an RCAS shader method");
        let rcas_params = params_for_effects_with_shader_method(
            [target_size[0] as usize, target_size[1] as usize],
            [target_size[0] as usize, target_size[1] as usize],
            ViewEffects::default(),
            rcas_method,
            0,
            [0, 0],
            target_size,
            1.0,
        );
        let rcas_params_bind_group = self.params_bind_group_for(rcas_params);
        self.render_pass(
            &mut encoder,
            output_view,
            intermediate_bind_group,
            &rcas_params_bind_group,
        );
        self.queue.submit(Some(encoder.finish()));
    }

    fn texture_bind_group_for(&self, source_view: &wgpu::TextureView) -> wgpu::BindGroup {
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("suisuiview-gpu-copy-texture-bind-group"),
            layout: &self.texture_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(source_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.texture_sampler),
                },
            ],
        })
    }

    fn params_bind_group_for(&self, params: EffectParams) -> wgpu::BindGroup {
        let params_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("suisuiview-gpu-copy-params"),
                contents: bytemuck::bytes_of(&params),
                usage: wgpu::BufferUsages::UNIFORM,
            });
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("suisuiview-gpu-copy-params-bind-group"),
            layout: &self.params_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: params_buffer.as_entire_binding(),
            }],
        })
    }

    fn legacy_bind_group_for(
        &self,
        source_view: &wgpu::TextureView,
        params: EffectParams,
    ) -> wgpu::BindGroup {
        let params_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("suisuiview-gpu-copy-fsr1-params"),
                contents: bytemuck::bytes_of(&params),
                usage: wgpu::BufferUsages::UNIFORM,
            });
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("suisuiview-gpu-copy-fsr1-bind-group"),
            layout: &self.legacy_bind_group_layout,
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
        })
    }

    fn render_pass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        output_view: &wgpu::TextureView,
        texture_bind_group: &wgpu::BindGroup,
        params_bind_group: &wgpu::BindGroup,
    ) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("suisuiview-gpu-copy-fsr1-pass"),
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
        pass.set_bind_group(0, texture_bind_group, &[]);
        pass.set_bind_group(1, params_bind_group, &[]);
        pass.draw(0..3, 0..1);
    }

    fn poll(&self) -> Result<(), String> {
        self.device
            .poll(wgpu::PollType::Wait)
            .map(|_| ())
            .map_err(|error| format!("wgpu poll failed: {error}"))
    }

    fn flush(&self) -> Result<(), String> {
        self.queue.submit(std::iter::empty());
        self.poll()
    }
}

fn extent(width: usize, height: usize) -> wgpu::Extent3d {
    wgpu::Extent3d {
        width: width as u32,
        height: height as u32,
        depth_or_array_layers: 1,
    }
}

fn print_report(report: &GpuCopyBenchReport) {
    println!("SuiSuiView GPU copy bench");
    println!("Path: {}", report.path);
    println!("Book: {}", report.title);
    println!(
        "Pages: {} sampled / {} total, {} failed",
        report.sampled_pages, report.page_count, report.failures
    );
    println!("Target long edge: {}", report.target_long_edge);
    println!("Iterations: {}", report.iterations);
    println!(
        "GPU: {}",
        report
            .adapter
            .as_deref()
            .or(report.gpu_error.as_deref())
            .unwrap_or("unavailable")
    );
    for summary in &report.summaries {
        let creation_note =
            if summary.avg_bind_group_creates > 0.0 || summary.avg_uniform_buffer_creates > 0.0 {
                format!(
                    ", bind groups {:.1}, uniform buffers {:.1}",
                    summary.avg_bind_group_creates, summary.avg_uniform_buffer_creates
                )
            } else {
                String::new()
            };
        println!(
            "{:<36} avg {:>7.3} ms, p95 {:>7.3} ms, max {:>7.3} ms{}",
            summary.case, summary.avg_ms, summary.p95_ms, summary.max_ms, creation_note
        );
    }
}

fn write_report(path: &Path, report: &GpuCopyBenchReport) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let json = serde_json::to_string_pretty(report).map_err(|error| error.to_string())?;
    fs::write(path, json).map_err(|error| error.to_string())
}

fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}
