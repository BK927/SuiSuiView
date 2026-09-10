//! Creation and attachment-format changes for the shared paint resources.
#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
use super::pools::texture_format_label;
use super::pools::INTERMEDIATE_TEXTURE_FORMAT;
use super::{
    GpuPaintResources, GPU_DRAW_BIND_GROUP_CACHE_LIMIT, GPU_INTERMEDIATE_TEXTURE_BUDGET_BYTES,
    GPU_INTERMEDIATE_TEXTURE_CACHE_LIMIT, GPU_REALTIME_SR_DEFER_CACHE_LIMIT,
    GPU_SOURCE_TEXTURE_BUDGET_BYTES, GPU_SOURCE_TEXTURE_CACHE_LIMIT,
};
use crate::app::realtime_sr::RealtimeSrResources;
#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
use crate::core::perf_trace::{self, PerfField};
use lru::LruCache;
#[cfg(test)]
use std::sync::atomic::AtomicUsize;
#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
use std::time::Instant;
use std::{borrow::Cow, num::NonZeroUsize};

impl GpuPaintResources {
    pub(super) fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Self {
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        let started = Instant::now();
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("suisuiview-gpu-effect-shader"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(include_str!(
                "../../core/gpu_effect.wgsl"
            ))),
        });
        let texture_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("suisuiview-gpu-effect-texture-layout"),
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
                label: Some("suisuiview-gpu-effect-params-layout"),
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
            label: Some("suisuiview-gpu-effect-pipeline-layout"),
            bind_group_layouts: &[&texture_bind_group_layout, &params_bind_group_layout],
            push_constant_ranges: &[],
        });
        let pipeline = super::passes::create_effect_pipeline_timed(
            device,
            &shader,
            &pipeline_layout,
            target_format,
            "suisuiview-gpu-effect-pipeline",
        );
        let intermediate_pipeline = super::passes::create_effect_pipeline_timed(
            device,
            &shader,
            &pipeline_layout,
            INTERMEDIATE_TEXTURE_FORMAT,
            "suisuiview-gpu-effect-intermediate-pipeline",
        );

        let texture_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("suisuiview-gpu-effect-linear-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let resources = Self {
            target_format,
            texture_bind_group_layout,
            params_bind_group_layout,
            texture_sampler,
            pipeline,
            intermediate_pipeline,
            deband_pipeline: None,
            request_linear_downscale: false,
            request_background_refine: false,
            request_inspection: None,
            inspection_pipeline: None,
            display_pipeline_backup: None,
            display_scale_capture: Default::default(),
            refine: super::refine::RefineCoordinator::default(),
            source_textures: LruCache::new(
                NonZeroUsize::new(GPU_SOURCE_TEXTURE_CACHE_LIMIT).unwrap(),
            ),
            source_texture_bytes: 0,
            draw_bind_groups: LruCache::new(
                NonZeroUsize::new(GPU_DRAW_BIND_GROUP_CACHE_LIMIT).unwrap(),
            ),
            draw_state_intermediate_bytes: 0,
            intermediate_textures: LruCache::new(
                NonZeroUsize::new(GPU_INTERMEDIATE_TEXTURE_CACHE_LIMIT).unwrap(),
            ),
            intermediate_texture_bytes: 0,
            source_texture_budget_bytes: GPU_SOURCE_TEXTURE_BUDGET_BYTES,
            intermediate_texture_budget_bytes: GPU_INTERMEDIATE_TEXTURE_BUDGET_BYTES,
            current_pass: 0,
            deferred_realtime_sr_first_frames: LruCache::new(
                NonZeroUsize::new(GPU_REALTIME_SR_DEFER_CACHE_LIMIT).unwrap(),
            ),
            realtime_sr: RealtimeSrResources::new(),
            #[cfg(test)]
            params_buffer_creations: AtomicUsize::new(0),
        };
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        perf_trace::record_duration(
            "gpu_paint_resources_create",
            started.elapsed(),
            &[PerfField::Str(
                "target_format",
                texture_format_label(target_format),
            )],
        );
        // Reset the read-only mirrors so a recreation (e.g. target-format change) does not leave
        // stale byte counts visible to the UI thread.
        resources.publish_gpu_pool_bytes();
        resources
    }

    /// ICC scene routing changes only the final attachment format. Preserve
    /// source uploads, prepared SR output and worker ownership across the move.
    pub(super) fn set_target_format(&mut self, device: &wgpu::Device, format: wgpu::TextureFormat) {
        if self.target_format == format {
            return;
        }
        if self.target_format != INTERMEDIATE_TEXTURE_FORMAT {
            self.display_pipeline_backup = Some((self.target_format, self.pipeline.clone()));
        }
        self.pipeline = if format == INTERMEDIATE_TEXTURE_FORMAT {
            self.intermediate_pipeline.clone()
        } else if let Some((_, pipeline)) = self
            .display_pipeline_backup
            .as_ref()
            .filter(|(saved, _)| *saved == format)
        {
            pipeline.clone()
        } else {
            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("suisuiview-display-format-shader"),
                source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(include_str!(
                    "../../core/gpu_effect.wgsl"
                ))),
            });
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("suisuiview-display-format-layout"),
                bind_group_layouts: &[
                    &self.texture_bind_group_layout,
                    &self.params_bind_group_layout,
                ],
                push_constant_ranges: &[],
            });
            super::passes::create_effect_pipeline_timed(
                device,
                &shader,
                &layout,
                format,
                "suisuiview-display-format-pipeline",
            )
        };
        self.target_format = format;
    }
}
