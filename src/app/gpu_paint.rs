use super::realtime_sr::RealtimeSrResources;
use super::{PageCacheKey, SuiSuiViewApp};
use crate::core::effects::ViewEffects;
use crate::core::gpu_effect::{
    output_size_for_effects, params_for_effects, params_for_effects_with_shader_method,
};
#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
use crate::core::perf_trace::{self, PerfField};
use crate::core::state::{DisplayUpscaler, GpuEffectMode, RendererMode};
use eframe::egui::{self, PaintCallbackInfo, Rect};
use egui_wgpu::{CallbackResources, CallbackTrait, ScreenDescriptor};
use lru::LruCache;
use std::borrow::Cow;
use std::hash::{Hash, Hasher};
use std::num::NonZeroUsize;
use std::sync::{Arc, OnceLock};
use std::time::Duration;
#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
use std::time::Instant;
use wgpu::util::DeviceExt;

pub(super) const GPU_SOURCE_TEXTURE_BUDGET_BYTES: usize = 192 * 1024 * 1024;
pub(super) const GPU_INTERMEDIATE_TEXTURE_BUDGET_BYTES: usize = 256 * 1024 * 1024;
const GPU_SOURCE_TEXTURE_CACHE_LIMIT: usize = 32;
const GPU_DRAW_BIND_GROUP_CACHE_LIMIT: usize = 16;
const GPU_INTERMEDIATE_TEXTURE_CACHE_LIMIT: usize = 16;
const EXPERIMENT_DISPLAY_UPSCALER_ENV: &str = "SUISUIVIEW_EXPERIMENT_DISPLAY_UPSCALER";
const EXPERIMENT_SPAN_DISPLAY_ENV: &str = "SUISUIVIEW_SR_LAB_SPAN_DISPLAY";
const EXPERIMENT_SPAN_MANIFEST_ENV: &str = "SUISUIVIEW_EXPERIMENT_SPAN_MANIFEST";
const SR_LAB_SPAN_MANIFEST_ENV: &str = "SUISUIVIEW_SR_LAB_SPAN_MANIFEST";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct GpuPaintSourceKey {
    pub(super) book: u64,
    pub(super) page: PageCacheKey,
    pub(super) upscaled: bool,
    pub(super) generation: u64,
}

pub(super) struct GpuPaintRequest {
    pub(super) rect: Rect,
    pub(super) source_key: GpuPaintSourceKey,
    pub(super) image_size: [usize; 2],
    pub(super) rgba: Arc<[u8]>,
    pub(super) effects: ViewEffects,
    pub(super) display_upscaler: DisplayUpscaler,
    pub(super) opacity: f32,
}

impl SuiSuiViewApp {
    pub(super) fn gpu_paint_book_key(&self) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.book_id.hash(&mut hasher);
        self.opened_path.hash(&mut hasher);
        hasher.finish()
    }

    pub(super) fn active_display_upscaler(&self) -> DisplayUpscaler {
        if !self.gpu_effects_available
            || self.gpu_target_format.is_none()
            || matches!(self.settings.gpu_effect_mode, GpuEffectMode::CpuOnly)
            || !matches!(self.settings.renderer_mode, RendererMode::Wgpu)
        {
            return DisplayUpscaler::None;
        }
        if let Some(upscaler) = experimental_display_upscaler_override() {
            return upscaler;
        }
        match self.settings.display_upscaler {
            DisplayUpscaler::None => DisplayUpscaler::None,
            upscaler if upscaler.product_selectable() => upscaler,
            _ => DisplayUpscaler::Auto,
        }
    }

    pub(super) fn can_paint_wgsl_effects(&self) -> bool {
        let display_upscaler = self.active_display_upscaler();
        self.gpu_effects_available
            && (self.effects != ViewEffects::default() || display_upscaler != DisplayUpscaler::None)
            && matches!(
                self.settings.gpu_effect_mode,
                GpuEffectMode::Auto | GpuEffectMode::Wgsl
            )
            && self.gpu_target_format.is_some()
    }

    pub(super) fn paint_wgsl_effects(
        &self,
        painter: &egui::Painter,
        request: GpuPaintRequest,
    ) -> bool {
        let Some(target_format) = self.gpu_target_format else {
            return false;
        };
        let callback = GpuEffectCallback {
            source_key: request.source_key,
            image_size: request.image_size,
            rgba: request.rgba,
            effects: request.effects,
            display_upscaler: request.display_upscaler,
            opacity: request.opacity.clamp(0.0, 1.0),
            rect: request.rect,
            target_format,
            draw_id: draw_id(
                request.source_key,
                request.effects,
                request.display_upscaler,
                request.rect,
                request.opacity,
            ),
            ctx: painter.ctx().clone(),
        };
        painter.add(egui_wgpu::Callback::new_paint_callback(
            request.rect,
            callback,
        ));
        true
    }

    pub(super) fn paint_pending_gpu_original_inspection_cleanup(
        &mut self,
        painter: &egui::Painter,
        rect: Rect,
    ) {
        if !self.pending_gpu_original_inspection_cleanup {
            return;
        }
        self.pending_gpu_original_inspection_cleanup = false;
        painter.add(egui_wgpu::Callback::new_paint_callback(
            rect,
            GpuOriginalInspectionCleanupCallback,
        ));
    }
}

fn experimental_display_upscaler_override() -> Option<DisplayUpscaler> {
    static OVERRIDE: OnceLock<Option<DisplayUpscaler>> = OnceLock::new();
    *OVERRIDE.get_or_init(|| {
        let generic_value = std::env::var(EXPERIMENT_DISPLAY_UPSCALER_ENV).ok();
        parse_experimental_display_upscaler(
            generic_value.as_deref(),
            opt_in_env_enabled(EXPERIMENT_SPAN_DISPLAY_ENV),
            span_manifest_env_present(),
        )
    })
}

fn parse_experimental_display_upscaler(
    generic_value: Option<&str>,
    explicit_span: bool,
    span_manifest_present: bool,
) -> Option<DisplayUpscaler> {
    match generic_value.map(str::trim) {
        Some(value) if value.eq_ignore_ascii_case(DisplayUpscaler::WgslArtcnnC4F16.token()) => {
            return Some(DisplayUpscaler::WgslArtcnnC4F16);
        }
        Some(value) if value.eq_ignore_ascii_case(DisplayUpscaler::WgslSrLabSpanX2.token()) => {
            if span_manifest_present {
                return Some(DisplayUpscaler::WgslSrLabSpanX2);
            }
        }
        _ => {}
    }
    if explicit_span && span_manifest_present {
        Some(DisplayUpscaler::WgslSrLabSpanX2)
    } else {
        None
    }
}

fn span_manifest_env_present() -> bool {
    std::env::var(EXPERIMENT_SPAN_MANIFEST_ENV)
        .or_else(|_| std::env::var(SR_LAB_SPAN_MANIFEST_ENV))
        .is_ok_and(|value| !value.trim().is_empty())
}

fn opt_in_env_enabled(name: &str) -> bool {
    matches!(
        std::env::var(name).ok().as_deref().map(str::trim),
        Some(value)
            if value.eq_ignore_ascii_case("1")
                || value.eq_ignore_ascii_case("true")
                || value.eq_ignore_ascii_case("on")
                || value.eq_ignore_ascii_case("yes")
    )
}

struct GpuOriginalInspectionCleanupCallback;

impl CallbackTrait for GpuOriginalInspectionCleanupCallback {
    fn prepare(
        &self,
        _device: &wgpu::Device,
        _queue: &wgpu::Queue,
        _screen_descriptor: &ScreenDescriptor,
        _egui_encoder: &mut wgpu::CommandEncoder,
        callback_resources: &mut CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        if let Some(resources) = callback_resources.get_mut::<GpuPaintResources>() {
            resources.drop_original_inspection_sources();
        }
        Vec::new()
    }

    fn paint(
        &self,
        _info: PaintCallbackInfo,
        _render_pass: &mut wgpu::RenderPass<'static>,
        _callback_resources: &CallbackResources,
    ) {
    }
}

struct GpuEffectCallback {
    source_key: GpuPaintSourceKey,
    image_size: [usize; 2],
    rgba: Arc<[u8]>,
    effects: ViewEffects,
    display_upscaler: DisplayUpscaler,
    opacity: f32,
    rect: Rect,
    target_format: wgpu::TextureFormat,
    draw_id: u64,
    ctx: egui::Context,
}

impl CallbackTrait for GpuEffectCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        screen_descriptor: &ScreenDescriptor,
        egui_encoder: &mut wgpu::CommandEncoder,
        callback_resources: &mut CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        if callback_resources.get::<GpuPaintResources>().is_none() {
            callback_resources.insert(GpuPaintResources::new(device, self.target_format));
        }
        let resources = callback_resources
            .get_mut::<GpuPaintResources>()
            .expect("GPU paint resources should be inserted before use");
        if resources.target_format != self.target_format {
            *resources = GpuPaintResources::new(device, self.target_format);
        }
        resources.ensure_source_texture(
            device,
            queue,
            self.source_key,
            self.image_size,
            &self.rgba,
        );

        let output_size = output_size_for_effects(self.image_size, self.effects);
        let (origin, target_size) = viewport_rect(self.rect, screen_descriptor);
        if let Some(source_bind_group) = resources
            .source_textures
            .peek(&self.source_key)
            .map(|source| source.bind_group.clone())
        {
            let draw_state = resources.prepare_draw_state(
                device,
                egui_encoder,
                self.source_key,
                source_bind_group,
                self.image_size,
                output_size,
                self.effects,
                self.display_upscaler,
                origin,
                target_size,
                self.opacity,
                &self.ctx,
            );
            resources.insert_draw_state(self.draw_id, draw_state);
        }
        Vec::new()
    }

    fn paint(
        &self,
        _info: PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        callback_resources: &CallbackResources,
    ) {
        let Some(resources) = callback_resources.get::<GpuPaintResources>() else {
            return;
        };
        let Some(draw_state) = resources.draw_bind_groups.peek(&self.draw_id) else {
            return;
        };
        render_pass.set_pipeline(&resources.pipeline);
        render_pass.set_bind_group(0, draw_state.texture_bind_group.as_ref(), &[]);
        render_pass.set_bind_group(1, &draw_state.params_bind_group, &[]);
        render_pass.draw(0..3, 0..1);
    }
}

struct GpuPaintResources {
    target_format: wgpu::TextureFormat,
    texture_bind_group_layout: wgpu::BindGroupLayout,
    params_bind_group_layout: wgpu::BindGroupLayout,
    pipeline: wgpu::RenderPipeline,
    source_textures: LruCache<GpuPaintSourceKey, GpuSourceTexture>,
    source_texture_bytes: usize,
    draw_bind_groups: LruCache<u64, GpuDrawState>,
    draw_state_intermediate_bytes: usize,
    intermediate_textures: LruCache<u64, Arc<GpuIntermediateTexture>>,
    intermediate_texture_bytes: usize,
    realtime_sr: RealtimeSrResources,
}

struct GpuSourceTexture {
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
    bind_group: Arc<wgpu::BindGroup>,
    byte_size: usize,
}

struct GpuDrawState {
    texture_bind_group: Arc<wgpu::BindGroup>,
    params_bind_group: wgpu::BindGroup,
    _intermediate_pin: Option<Arc<GpuIntermediateTexture>>,
    intermediate_byte_size: usize,
}

struct GpuIntermediateTexture {
    _texture: wgpu::Texture,
    _view: wgpu::TextureView,
    bind_group: Arc<wgpu::BindGroup>,
    size: [usize; 2],
    byte_size: usize,
}

impl GpuDrawState {
    fn new(
        texture_bind_group: Arc<wgpu::BindGroup>,
        params_bind_group: wgpu::BindGroup,
        intermediate_pin: Option<Arc<GpuIntermediateTexture>>,
    ) -> Self {
        let intermediate_byte_size = intermediate_pin
            .as_ref()
            .map_or(0, |texture| texture.byte_size);
        Self {
            texture_bind_group,
            params_bind_group,
            _intermediate_pin: intermediate_pin,
            intermediate_byte_size,
        }
    }
}

impl GpuPaintResources {
    fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("suisuiview-gpu-effect-shader"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(include_str!(
                "../core/gpu_effect.wgsl"
            ))),
        });
        let texture_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("suisuiview-gpu-effect-texture-layout"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                }],
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
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("suisuiview-gpu-effect-pipeline"),
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
                    format: target_format,
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
        Self {
            target_format,
            texture_bind_group_layout,
            params_bind_group_layout,
            pipeline,
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
            realtime_sr: RealtimeSrResources::new(),
        }
    }

    fn ensure_source_texture(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        key: GpuPaintSourceKey,
        image_size: [usize; 2],
        rgba: &[u8],
    ) {
        if self.source_textures.get(&key).is_some() {
            return;
        }
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        let upload_started = Instant::now();
        let [width, height] = image_size;
        let byte_size = width.saturating_mul(height).saturating_mul(4);
        if rgba.len() != byte_size {
            return;
        }
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("suisuiview-gpu-effect-source"),
            size: wgpu::Extent3d {
                width: width as u32,
                height: height as u32,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some((width * 4) as u32),
                rows_per_image: Some(height as u32),
            },
            wgpu::Extent3d {
                width: width as u32,
                height: height as u32,
                depth_or_array_layers: 1,
            },
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = Arc::new(self.texture_bind_group_for(device, &view));
        if let Some((_old_key, old_texture)) = self.source_textures.push(
            key,
            GpuSourceTexture {
                _texture: texture,
                view,
                bind_group,
                byte_size,
            },
        ) {
            self.source_texture_bytes = self
                .source_texture_bytes
                .saturating_sub(old_texture.byte_size);
        }
        self.source_texture_bytes = self.source_texture_bytes.saturating_add(byte_size);
        self.prune_source_textures();
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        perf_trace::record_duration_if_at_least(
            "gpu_texture_upload",
            upload_started.elapsed(),
            Duration::from_millis(16),
            &[
                PerfField::Usize("width", width),
                PerfField::Usize("height", height),
                PerfField::Bool("upscaled", key.upscaled),
            ],
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn prepare_draw_state(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        source_key: GpuPaintSourceKey,
        source_bind_group: Arc<wgpu::BindGroup>,
        source_size: [usize; 2],
        output_size: [usize; 2],
        effects: ViewEffects,
        display_upscaler: DisplayUpscaler,
        origin: [u32; 2],
        target_size: [u32; 2],
        opacity: f32,
        ctx: &egui::Context,
    ) -> GpuDrawState {
        let effective_upscaler = display_upscaler
            .resolve_for_render(output_size, target_size)
            .unwrap_or(DisplayUpscaler::None);
        if RealtimeSrResources::is_supported(effective_upscaler) {
            let sr_key = realtime_sr_texture_key(source_key, source_size, effective_upscaler);
            self.ensure_realtime_sr_texture(
                device,
                encoder,
                sr_key,
                source_key,
                source_size,
                effective_upscaler,
            );
            if self.realtime_sr.has_pending_async_work(effective_upscaler) {
                ctx.request_repaint_after(Duration::from_millis(16));
            }
            if let Some(intermediate) = self.intermediate_textures.peek(&sr_key).cloned() {
                record_display_upscaler_render(
                    effective_upscaler,
                    source_size,
                    output_size,
                    target_size,
                    intermediate.size,
                    "realtime_sr",
                );
                let params = params_for_effects(
                    intermediate.size,
                    output_size_for_effects(intermediate.size, effects),
                    effects,
                    DisplayUpscaler::None,
                    origin,
                    target_size,
                    opacity,
                );
                return GpuDrawState::new(
                    intermediate.bind_group.clone(),
                    self.params_bind_group_for(device, params),
                    Some(intermediate),
                );
            }
        }
        if let Some(rcas_method) = effective_upscaler.rcas_shader_method_id() {
            let intermediate_key = intermediate_texture_key(
                source_key,
                source_size,
                output_size,
                effects,
                effective_upscaler,
                target_size,
            );
            self.ensure_intermediate_texture(device, intermediate_key, target_size);
            let intermediate = self
                .intermediate_textures
                .peek(&intermediate_key)
                .expect("intermediate texture should be cached before rendering")
                .clone();
            let intermediate_bind_group = intermediate.bind_group.clone();
            let intermediate_view = &intermediate._view;
            let easu_params = params_for_effects(
                source_size,
                output_size,
                effects,
                effective_upscaler,
                [0, 0],
                target_size,
                1.0,
            );
            let easu_params_bind_group = self.params_bind_group_for(device, easu_params);
            self.render_fullscreen(
                encoder,
                intermediate_view,
                &source_bind_group,
                &easu_params_bind_group,
            );

            let rcas_params = params_for_effects_with_shader_method(
                [target_size[0] as usize, target_size[1] as usize],
                [target_size[0] as usize, target_size[1] as usize],
                ViewEffects::default(),
                rcas_method,
                origin,
                target_size,
                opacity,
            );
            let params_bind_group = self.params_bind_group_for(device, rcas_params);
            record_display_upscaler_render(
                effective_upscaler,
                source_size,
                output_size,
                target_size,
                [target_size[0] as usize, target_size[1] as usize],
                "easu_rcas",
            );
            return GpuDrawState::new(
                intermediate_bind_group,
                params_bind_group,
                Some(intermediate),
            );
        }

        if effective_upscaler.shader_method_id() != 0 {
            record_display_upscaler_render(
                effective_upscaler,
                source_size,
                output_size,
                target_size,
                target_size.map(|dimension| dimension as usize),
                "single_pass",
            );
        }
        let params = params_for_effects(
            source_size,
            output_size,
            effects,
            effective_upscaler,
            origin,
            target_size,
            opacity,
        );
        GpuDrawState::new(
            source_bind_group,
            self.params_bind_group_for(device, params),
            None,
        )
    }

    fn ensure_intermediate_texture(
        &mut self,
        device: &wgpu::Device,
        key: u64,
        target_size: [u32; 2],
    ) {
        if self.intermediate_textures.get(&key).is_some() {
            return;
        }
        let byte_size = (target_size[0] as usize)
            .saturating_mul(target_size[1] as usize)
            .saturating_mul(4);
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("suisuiview-gpu-upscale-intermediate"),
            size: wgpu::Extent3d {
                width: target_size[0],
                height: target_size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = Arc::new(self.texture_bind_group_for(device, &view));
        if let Some((_old_key, old_texture)) = self.intermediate_textures.push(
            key,
            Arc::new(GpuIntermediateTexture {
                _texture: texture,
                _view: view,
                bind_group,
                size: [target_size[0] as usize, target_size[1] as usize],
                byte_size,
            }),
        ) {
            self.intermediate_texture_bytes = self
                .intermediate_texture_bytes
                .saturating_sub(old_texture.byte_size);
        }
        self.intermediate_texture_bytes = self.intermediate_texture_bytes.saturating_add(byte_size);
        self.prune_intermediate_textures();
    }

    fn ensure_realtime_sr_texture(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        key: u64,
        source_key: GpuPaintSourceKey,
        source_size: [usize; 2],
        method: DisplayUpscaler,
    ) {
        if self.intermediate_textures.get(&key).is_some() {
            return;
        }
        let Some(source) = self.source_textures.peek(&source_key) else {
            return;
        };
        let Some(output) =
            self.realtime_sr
                .render(method, device, encoder, &source.view, source_size)
        else {
            return;
        };
        let output_size = output.size;
        let output_byte_size = output.byte_size;
        let bind_group = Arc::new(self.texture_bind_group_for(device, &output.view));
        if let Some((_old_key, old_texture)) = self.intermediate_textures.push(
            key,
            Arc::new(GpuIntermediateTexture {
                _texture: output.texture,
                _view: output.view,
                bind_group,
                size: output_size,
                byte_size: output_byte_size,
            }),
        ) {
            self.intermediate_texture_bytes = self
                .intermediate_texture_bytes
                .saturating_sub(old_texture.byte_size);
        }
        self.intermediate_texture_bytes = self
            .intermediate_texture_bytes
            .saturating_add(output_byte_size);
        self.prune_intermediate_textures();
    }

    fn insert_draw_state(&mut self, key: u64, draw_state: GpuDrawState) {
        let byte_size = draw_state.intermediate_byte_size;
        if let Some((_old_key, old_state)) = self.draw_bind_groups.push(key, draw_state) {
            self.draw_state_intermediate_bytes = self
                .draw_state_intermediate_bytes
                .saturating_sub(old_state.intermediate_byte_size);
        }
        self.draw_state_intermediate_bytes =
            self.draw_state_intermediate_bytes.saturating_add(byte_size);
        self.prune_draw_states();
    }

    fn texture_bind_group_for(
        &self,
        device: &wgpu::Device,
        source_view: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("suisuiview-gpu-effect-texture-bind-group"),
            layout: &self.texture_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(source_view),
            }],
        })
    }

    fn params_bind_group_for(
        &self,
        device: &wgpu::Device,
        params: crate::core::gpu_effect::EffectParams,
    ) -> wgpu::BindGroup {
        let params_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("suisuiview-gpu-effect-params"),
            contents: bytemuck::bytes_of(&params),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("suisuiview-gpu-effect-params-bind-group"),
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
        output_view: &wgpu::TextureView,
        texture_bind_group: &wgpu::BindGroup,
        params_bind_group: &wgpu::BindGroup,
    ) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("suisuiview-gpu-upscale-intermediate-pass"),
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

    fn prune_source_textures(&mut self) {
        while self.source_texture_bytes > GPU_SOURCE_TEXTURE_BUDGET_BYTES
            && self.source_textures.len() > 1
        {
            let Some((_key, texture)) = self.source_textures.pop_lru() else {
                break;
            };
            self.source_texture_bytes = self.source_texture_bytes.saturating_sub(texture.byte_size);
        }
    }

    fn drop_original_inspection_sources(&mut self) {
        let keys = self
            .source_textures
            .iter()
            .filter_map(|(key, _texture)| {
                (key.page.target_long_edge > crate::core::worker::MAX_TARGET_LONG_EDGE)
                    .then_some(*key)
            })
            .collect::<Vec<_>>();
        if keys.is_empty() {
            return;
        }

        for key in keys {
            if let Some(texture) = self.source_textures.pop(&key) {
                self.source_texture_bytes =
                    self.source_texture_bytes.saturating_sub(texture.byte_size);
            }
        }

        self.draw_bind_groups.clear();
        self.draw_state_intermediate_bytes = 0;
        self.intermediate_textures.clear();
        self.intermediate_texture_bytes = 0;
    }

    fn prune_intermediate_textures(&mut self) {
        while self.intermediate_texture_bytes > GPU_INTERMEDIATE_TEXTURE_BUDGET_BYTES
            && self.intermediate_textures.len() > 1
        {
            let Some((_key, texture)) = self.intermediate_textures.pop_lru() else {
                break;
            };
            self.intermediate_texture_bytes = self
                .intermediate_texture_bytes
                .saturating_sub(texture.byte_size);
        }
    }

    fn prune_draw_states(&mut self) {
        while self.draw_state_intermediate_bytes > GPU_INTERMEDIATE_TEXTURE_BUDGET_BYTES
            && self.draw_bind_groups.len() > 1
        {
            let Some((_key, draw_state)) = self.draw_bind_groups.pop_lru() else {
                break;
            };
            self.draw_state_intermediate_bytes = self
                .draw_state_intermediate_bytes
                .saturating_sub(draw_state.intermediate_byte_size);
        }
    }
}

#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
fn record_display_upscaler_render(
    method: DisplayUpscaler,
    source_size: [usize; 2],
    output_size: [usize; 2],
    target_size: [u32; 2],
    rendered_size: [usize; 2],
    path: &'static str,
) {
    perf_trace::record_duration(
        "display_upscaler_render",
        Duration::ZERO,
        &[
            PerfField::Str("method", method.token()),
            PerfField::Str("path", path),
            PerfField::Usize("source_width", source_size[0]),
            PerfField::Usize("source_height", source_size[1]),
            PerfField::Usize("output_width", output_size[0]),
            PerfField::Usize("output_height", output_size[1]),
            PerfField::U32("target_width", target_size[0]),
            PerfField::U32("target_height", target_size[1]),
            PerfField::Usize("rendered_width", rendered_size[0]),
            PerfField::Usize("rendered_height", rendered_size[1]),
        ],
    );
}

#[cfg(not(any(feature = "perf-dev", feature = "perf-diagnostics")))]
fn record_display_upscaler_render(
    _method: DisplayUpscaler,
    _source_size: [usize; 2],
    _output_size: [usize; 2],
    _target_size: [u32; 2],
    _rendered_size: [usize; 2],
    _path: &'static str,
) {
}

fn draw_id(
    source_key: GpuPaintSourceKey,
    effects: ViewEffects,
    display_upscaler: DisplayUpscaler,
    rect: Rect,
    opacity: f32,
) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    source_key.hash(&mut hasher);
    effects.hash(&mut hasher);
    display_upscaler.token().hash(&mut hasher);
    rect.min.x.to_bits().hash(&mut hasher);
    rect.min.y.to_bits().hash(&mut hasher);
    rect.max.x.to_bits().hash(&mut hasher);
    rect.max.y.to_bits().hash(&mut hasher);
    opacity.to_bits().hash(&mut hasher);
    hasher.finish()
}

fn viewport_rect(rect: Rect, screen_descriptor: &ScreenDescriptor) -> ([u32; 2], [u32; 2]) {
    let screen_width = screen_descriptor.size_in_pixels[0] as i32;
    let screen_height = screen_descriptor.size_in_pixels[1] as i32;
    let left = (screen_descriptor.pixels_per_point * rect.min.x)
        .round()
        .clamp(0.0, screen_width as f32) as u32;
    let top = (screen_descriptor.pixels_per_point * rect.min.y)
        .round()
        .clamp(0.0, screen_height as f32) as u32;
    let right_raw = (screen_descriptor.pixels_per_point * rect.max.x)
        .round()
        .clamp(0.0, screen_width as f32) as u32;
    let bottom_raw = (screen_descriptor.pixels_per_point * rect.max.y)
        .round()
        .clamp(0.0, screen_height as f32) as u32;
    let right = right_raw
        .max(left.saturating_add(1))
        .min(screen_width as u32);
    let bottom = bottom_raw
        .max(top.saturating_add(1))
        .min(screen_height as u32);
    (
        [left, top],
        [
            right.saturating_sub(left).max(1),
            bottom.saturating_sub(top).max(1),
        ],
    )
}

fn intermediate_texture_key(
    source_key: GpuPaintSourceKey,
    source_size: [usize; 2],
    output_size: [usize; 2],
    effects: ViewEffects,
    display_upscaler: DisplayUpscaler,
    target_size: [u32; 2],
) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    source_key.hash(&mut hasher);
    source_size.hash(&mut hasher);
    output_size.hash(&mut hasher);
    effects.hash(&mut hasher);
    display_upscaler.token().hash(&mut hasher);
    target_size.hash(&mut hasher);
    hasher.finish()
}

fn realtime_sr_texture_key(
    source_key: GpuPaintSourceKey,
    source_size: [usize; 2],
    display_upscaler: DisplayUpscaler,
) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    source_key.hash(&mut hasher);
    source_size.hash(&mut hasher);
    display_upscaler.token().hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::worker::DecodeOptions;
    use eframe::egui::{pos2, vec2};

    #[test]
    fn draw_id_separates_same_page_in_different_panes() {
        let source_key = GpuPaintSourceKey {
            book: 7,
            page: PageCacheKey {
                index: 3,
                target_long_edge: 2048,
                decode: DecodeOptions::default(),
            },
            upscaled: false,
            generation: 1,
        };
        let left = Rect::from_min_size(pos2(0.0, 0.0), vec2(640.0, 900.0));
        let right = Rect::from_min_size(pos2(640.0, 0.0), vec2(640.0, 900.0));

        assert_ne!(
            draw_id(
                source_key,
                ViewEffects::default(),
                DisplayUpscaler::WgslFsr1Style,
                left,
                1.0,
            ),
            draw_id(
                source_key,
                ViewEffects::default(),
                DisplayUpscaler::WgslFsr1Style,
                right,
                1.0,
            )
        );
    }

    #[test]
    fn experimental_display_upscaler_parses_hidden_artcnn() {
        assert_eq!(
            parse_experimental_display_upscaler(Some(" artcnn_c4f16 "), false, false),
            Some(DisplayUpscaler::WgslArtcnnC4F16)
        );
    }

    #[test]
    fn experimental_display_upscaler_keeps_span_manifest_gated() {
        assert_eq!(
            parse_experimental_display_upscaler(Some("srlab_span_x2"), false, false),
            None
        );
        assert_eq!(
            parse_experimental_display_upscaler(Some("srlab_span_x2"), false, true),
            Some(DisplayUpscaler::WgslSrLabSpanX2)
        );
        assert_eq!(
            parse_experimental_display_upscaler(None, true, true),
            Some(DisplayUpscaler::WgslSrLabSpanX2)
        );
    }
}
