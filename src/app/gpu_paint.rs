use super::realtime_sr::RealtimeSrResources;
use super::{PageCacheKey, SuiSuiViewApp};
use crate::core::effects::ViewEffects;
use crate::core::gpu_effect::{
    output_size_for_effects, params_for_effects, params_for_effects_with_display,
    params_for_effects_with_shader_method, params_for_hardware_mipmap_sample,
    params_for_hardware_mipmap_sample_with_display,
};
#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
use crate::core::perf_trace::{self, PerfField};
use crate::core::state::{
    FitMode, GpuEffectMode, RendererMode, WgpuDownscaleMethod, WgpuScalePlan, WgpuUpscaleMethod,
};
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
const GPU_REALTIME_SR_DEFER_CACHE_LIMIT: usize = 64;
const EXPERIMENT_WGPU_UPSCALE_METHOD_ENV: &str = "SUISUIVIEW_EXPERIMENT_WGPU_UPSCALE_METHOD";
const EXPERIMENT_SPAN_DISPLAY_ENV: &str = "SUISUIVIEW_SR_LAB_SPAN_DISPLAY";
const EXPERIMENT_SPAN_MANIFEST_ENV: &str = "SUISUIVIEW_EXPERIMENT_SPAN_MANIFEST";
const SR_LAB_SPAN_MANIFEST_ENV: &str = "SUISUIVIEW_SR_LAB_SPAN_MANIFEST";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct GpuPaintSourceKey {
    pub(super) book: u64,
    pub(super) page: PageCacheKey,
}

pub(super) struct GpuPaintRequest {
    pub(super) rect: Rect,
    pub(super) source_key: GpuPaintSourceKey,
    pub(super) image_size: [usize; 2],
    pub(super) rgba: Arc<[u8]>,
    pub(super) effects: ViewEffects,
    pub(super) wgpu_upscale_method: WgpuUpscaleMethod,
    pub(super) wgpu_downscale_method: WgpuDownscaleMethod,
    pub(super) opacity: f32,
}

impl SuiSuiViewApp {
    pub(super) fn gpu_paint_book_key(&self) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.book_id.hash(&mut hasher);
        self.opened_path.hash(&mut hasher);
        hasher.finish()
    }

    pub(super) fn active_wgpu_upscale_method(&self) -> WgpuUpscaleMethod {
        if !fit_mode_allows_display_upscale(self.fit_mode)
            || !self.gpu_effects_available
            || self.gpu_target_format.is_none()
            || matches!(self.settings.gpu_effect_mode, GpuEffectMode::CpuOnly)
            || !matches!(self.settings.renderer_mode, RendererMode::Wgpu)
        {
            return WgpuUpscaleMethod::None;
        }
        if let Some(upscaler) = experimental_wgpu_upscale_method_override() {
            return upscaler;
        }
        let span_manifest_present = matches!(
            self.settings.wgpu_upscale_method,
            WgpuUpscaleMethod::WgslSrLabSpanX2
        ) && span_manifest_env_present();
        wgpu_upscale_method_from_settings(self.settings.wgpu_upscale_method, span_manifest_present)
    }

    pub(super) fn can_paint_wgsl_effects(&self) -> bool {
        let wgpu_upscale_method = self.active_wgpu_upscale_method();
        self.gpu_effects_available
            && (self.effects != ViewEffects::default()
                || wgpu_upscale_method != WgpuUpscaleMethod::None
                || self.settings.wgpu_downscale_method != WgpuDownscaleMethod::Bilinear)
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
            wgpu_upscale_method: request.wgpu_upscale_method,
            wgpu_downscale_method: request.wgpu_downscale_method,
            opacity: request.opacity.clamp(0.0, 1.0),
            rect: request.rect,
            target_format,
            draw_id: draw_id(
                request.source_key,
                request.effects,
                request.wgpu_upscale_method,
                request.wgpu_downscale_method,
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

fn fit_mode_allows_display_upscale(fit_mode: FitMode) -> bool {
    !matches!(fit_mode, FitMode::Manual | FitMode::Original)
}

fn experimental_wgpu_upscale_method_override() -> Option<WgpuUpscaleMethod> {
    static OVERRIDE: OnceLock<Option<WgpuUpscaleMethod>> = OnceLock::new();
    *OVERRIDE.get_or_init(|| {
        let generic_value = std::env::var(EXPERIMENT_WGPU_UPSCALE_METHOD_ENV).ok();
        parse_experimental_wgpu_upscale_method(
            generic_value.as_deref(),
            opt_in_env_enabled(EXPERIMENT_SPAN_DISPLAY_ENV),
            span_manifest_env_present(),
        )
    })
}

fn parse_experimental_wgpu_upscale_method(
    generic_value: Option<&str>,
    explicit_span: bool,
    span_manifest_present: bool,
) -> Option<WgpuUpscaleMethod> {
    if let Some(value) = generic_value.map(str::trim) {
        if let Some(method) = WgpuUpscaleMethod::GPU_METHODS
            .iter()
            .copied()
            .find(|method| method.is_artcnn() && value.eq_ignore_ascii_case(method.token()))
        {
            return Some(method);
        }
        if value.eq_ignore_ascii_case(WgpuUpscaleMethod::WgslSrLabSpanX2.token()) {
            if span_manifest_present {
                return Some(WgpuUpscaleMethod::WgslSrLabSpanX2);
            }
        }
    }
    if explicit_span && span_manifest_present {
        Some(WgpuUpscaleMethod::WgslSrLabSpanX2)
    } else {
        None
    }
}

fn wgpu_upscale_method_from_settings(
    upscaler: WgpuUpscaleMethod,
    span_manifest_present: bool,
) -> WgpuUpscaleMethod {
    match upscaler {
        WgpuUpscaleMethod::None => WgpuUpscaleMethod::None,
        WgpuUpscaleMethod::WgslSrLabSpanX2 if !span_manifest_present => WgpuUpscaleMethod::Auto,
        upscaler if upscaler.user_selectable() => upscaler,
        _ => WgpuUpscaleMethod::Auto,
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
    wgpu_upscale_method: WgpuUpscaleMethod,
    wgpu_downscale_method: WgpuDownscaleMethod,
    opacity: f32,
    rect: Rect,
    target_format: wgpu::TextureFormat,
    draw_id: u64,
    ctx: egui::Context,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct GpuDisplayRect {
    origin: [u32; 2],
    visible_size: [u32; 2],
    sample_offset: [u32; 2],
    full_size: [u32; 2],
}

impl GpuDisplayRect {
    fn is_clipped(self) -> bool {
        self.sample_offset != [0, 0] || self.visible_size != self.full_size
    }
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
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        let prepare_started = Instant::now();
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        let mut resources_created = false;
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        let mut resources_recreated = false;
        if callback_resources.get::<GpuPaintResources>().is_none() {
            callback_resources.insert(GpuPaintResources::new(device, self.target_format));
            #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
            {
                resources_created = true;
            }
        }
        let resources = callback_resources
            .get_mut::<GpuPaintResources>()
            .expect("GPU paint resources should be inserted before use");
        if resources.target_format != self.target_format {
            *resources = GpuPaintResources::new(device, self.target_format);
            #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
            {
                resources_recreated = true;
            }
        }
        let source_uploaded = resources.ensure_source_texture(
            device,
            queue,
            self.source_key,
            self.image_size,
            &self.rgba,
        );
        #[cfg(not(any(feature = "perf-dev", feature = "perf-diagnostics")))]
        let _ = source_uploaded;

        let output_size = output_size_for_effects(self.image_size, self.effects);
        let display_rect = viewport_rect(self.rect, screen_descriptor);
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        let scale_plan = WgpuScalePlan::resolve(
            output_size,
            display_rect.full_size,
            self.wgpu_upscale_method,
            self.wgpu_downscale_method,
        );
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
                self.wgpu_upscale_method,
                self.wgpu_downscale_method,
                display_rect,
                self.opacity,
                &self.ctx,
            );
            resources.insert_draw_state(self.draw_id, draw_state);
        }
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        perf_trace::record_duration(
            "gpu_effect_prepare",
            prepare_started.elapsed(),
            &[
                PerfField::Usize("width", self.image_size[0]),
                PerfField::Usize("height", self.image_size[1]),
                PerfField::Usize("output_width", output_size[0]),
                PerfField::Usize("output_height", output_size[1]),
                PerfField::U32("target_width", display_rect.full_size[0]),
                PerfField::U32("target_height", display_rect.full_size[1]),
                PerfField::U32("visible_target_width", display_rect.visible_size[0]),
                PerfField::U32("visible_target_height", display_rect.visible_size[1]),
                PerfField::U32("sample_offset_x", display_rect.sample_offset[0]),
                PerfField::U32("sample_offset_y", display_rect.sample_offset[1]),
                PerfField::Bool("resources_created", resources_created),
                PerfField::Bool("resources_recreated", resources_recreated),
                PerfField::Bool("source_uploaded", source_uploaded),
                PerfField::Str("wgpu_upscale_method", self.wgpu_upscale_method.token()),
                PerfField::Str(
                    "effective_wgpu_upscale_method",
                    scale_plan.effective_upscale_method.token(),
                ),
                PerfField::Str("wgpu_downscale_method", self.wgpu_downscale_method.token()),
                PerfField::Str(
                    "effective_wgpu_downscale_method",
                    scale_plan.effective_downscale_method.token(),
                ),
                PerfField::Str("scale_direction", scale_plan.direction.token()),
            ],
        );
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
    texture_sampler: wgpu::Sampler,
    pipeline: wgpu::RenderPipeline,
    intermediate_pipeline: wgpu::RenderPipeline,
    source_textures: LruCache<GpuPaintSourceKey, GpuSourceTexture>,
    source_texture_bytes: usize,
    draw_bind_groups: LruCache<u64, GpuDrawState>,
    draw_state_intermediate_bytes: usize,
    intermediate_textures: LruCache<u64, Arc<GpuIntermediateTexture>>,
    intermediate_texture_bytes: usize,
    deferred_realtime_sr_first_frames: LruCache<u64, ()>,
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
    _intermediate_pins: Vec<Arc<GpuIntermediateTexture>>,
    intermediate_byte_size: usize,
}

struct GpuIntermediateTexture {
    _texture: wgpu::Texture,
    _view: wgpu::TextureView,
    mip_views: Vec<wgpu::TextureView>,
    bind_group: Arc<wgpu::BindGroup>,
    size: [usize; 2],
    byte_size: usize,
}

impl GpuDrawState {
    fn new(
        texture_bind_group: Arc<wgpu::BindGroup>,
        params_bind_group: wgpu::BindGroup,
        intermediate_pins: Vec<Arc<GpuIntermediateTexture>>,
    ) -> Self {
        let intermediate_byte_size = intermediate_pins
            .iter()
            .map(|texture| texture.byte_size)
            .sum();
        Self {
            texture_bind_group,
            params_bind_group,
            _intermediate_pins: intermediate_pins,
            intermediate_byte_size,
        }
    }
}

impl GpuPaintResources {
    fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Self {
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        let started = Instant::now();
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("suisuiview-gpu-effect-shader"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(include_str!(
                "../core/gpu_effect.wgsl"
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
        let pipeline = create_effect_pipeline_timed(
            device,
            &shader,
            &pipeline_layout,
            target_format,
            "suisuiview-gpu-effect-pipeline",
        );
        let intermediate_pipeline = create_effect_pipeline_timed(
            device,
            &shader,
            &pipeline_layout,
            wgpu::TextureFormat::Rgba8Unorm,
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
            deferred_realtime_sr_first_frames: LruCache::new(
                NonZeroUsize::new(GPU_REALTIME_SR_DEFER_CACHE_LIMIT).unwrap(),
            ),
            realtime_sr: RealtimeSrResources::new(),
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
        resources
    }

    fn ensure_source_texture(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        key: GpuPaintSourceKey,
        image_size: [usize; 2],
        rgba: &[u8],
    ) -> bool {
        if self.source_textures.get(&key).is_some() {
            return false;
        }
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        let upload_started = Instant::now();
        let [width, height] = image_size;
        let byte_size = width.saturating_mul(height).saturating_mul(4);
        if rgba.len() != byte_size {
            return false;
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
        perf_trace::record_duration(
            "gpu_texture_upload",
            upload_started.elapsed(),
            &[
                PerfField::Usize("width", width),
                PerfField::Usize("height", height),
            ],
        );
        true
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
        wgpu_upscale_method: WgpuUpscaleMethod,
        wgpu_downscale_method: WgpuDownscaleMethod,
        display_rect: GpuDisplayRect,
        opacity: f32,
        ctx: &egui::Context,
    ) -> GpuDrawState {
        let scale_plan = WgpuScalePlan::resolve(
            output_size,
            display_rect.full_size,
            wgpu_upscale_method,
            wgpu_downscale_method,
        );
        let effective_upscaler = scale_plan.effective_upscale_method;
        let effective_downscaler = scale_plan.effective_downscale_method;
        self.realtime_sr
            .cancel_inactive_pending_work(effective_upscaler);
        if RealtimeSrResources::is_supported(effective_upscaler) {
            let sr_key = realtime_sr_texture_key(source_key, source_size, effective_upscaler);
            if self.should_defer_realtime_sr_first_frame(sr_key, effective_upscaler) {
                self.realtime_sr.warm_up_async(effective_upscaler, device);
                ctx.request_repaint_after(Duration::from_millis(16));
            } else {
                self.ensure_realtime_sr_texture(
                    device,
                    encoder,
                    sr_key,
                    source_key,
                    source_size,
                    effective_upscaler,
                );
            }
            if self.realtime_sr.has_pending_async_work(effective_upscaler) {
                ctx.request_repaint_after(Duration::from_millis(16));
            }
            if let Some(intermediate) = self.intermediate_textures.peek(&sr_key).cloned() {
                record_wgpu_upscale_method_render(
                    effective_upscaler,
                    source_size,
                    output_size,
                    display_rect.full_size,
                    intermediate.size,
                    "realtime_sr",
                );
                let params = params_for_effects_with_display(
                    intermediate.size,
                    output_size_for_effects(intermediate.size, effects),
                    effects,
                    WgpuUpscaleMethod::None,
                    effective_downscaler,
                    display_rect.origin,
                    display_rect.visible_size,
                    display_rect.sample_offset,
                    display_rect.full_size,
                    opacity,
                );
                return GpuDrawState::new(
                    intermediate.bind_group.clone(),
                    self.params_bind_group_for(device, params),
                    vec![intermediate],
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
                display_rect,
            );
            self.ensure_intermediate_texture(device, intermediate_key, display_rect.visible_size);
            let intermediate = self
                .intermediate_textures
                .peek(&intermediate_key)
                .expect("intermediate texture should be cached before rendering")
                .clone();
            let intermediate_bind_group = intermediate.bind_group.clone();
            let intermediate_view = intermediate
                .mip_views
                .first()
                .expect("intermediate textures should expose a renderable mip 0 view");
            let easu_params = params_for_effects_with_display(
                source_size,
                output_size,
                effects,
                effective_upscaler,
                WgpuDownscaleMethod::Bilinear,
                [0, 0],
                display_rect.visible_size,
                display_rect.sample_offset,
                display_rect.full_size,
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
                [
                    display_rect.visible_size[0] as usize,
                    display_rect.visible_size[1] as usize,
                ],
                [
                    display_rect.visible_size[0] as usize,
                    display_rect.visible_size[1] as usize,
                ],
                ViewEffects::default(),
                rcas_method,
                0,
                display_rect.origin,
                display_rect.visible_size,
                opacity,
            );
            let params_bind_group = self.params_bind_group_for(device, rcas_params);
            record_wgpu_upscale_method_render(
                effective_upscaler,
                source_size,
                output_size,
                display_rect.full_size,
                [
                    display_rect.visible_size[0] as usize,
                    display_rect.visible_size[1] as usize,
                ],
                "easu_rcas",
            );
            return GpuDrawState::new(
                intermediate_bind_group,
                params_bind_group,
                vec![intermediate],
            );
        }

        if effective_downscaler.is_hardware_mipmap() {
            return self.prepare_hardware_mipmap_draw_state(
                device,
                encoder,
                source_key,
                source_bind_group,
                source_size,
                output_size,
                effects,
                display_rect,
                opacity,
            );
        }

        if effective_downscaler.is_pyramid()
            && !display_rect.is_clipped()
            && needs_multi_pass_downscale(output_size, display_rect.full_size)
        {
            return self.prepare_pyramid_downscale_draw_state(
                device,
                encoder,
                source_key,
                source_bind_group,
                source_size,
                output_size,
                effects,
                effective_downscaler,
                display_rect.origin,
                display_rect.visible_size,
                opacity,
            );
        }

        if effective_upscaler.shader_method_id() != 0 {
            record_wgpu_upscale_method_render(
                effective_upscaler,
                source_size,
                output_size,
                display_rect.full_size,
                display_rect
                    .visible_size
                    .map(|dimension| dimension as usize),
                "single_pass",
            );
        }
        let params = params_for_effects_with_display(
            source_size,
            output_size,
            effects,
            effective_upscaler,
            effective_downscaler,
            display_rect.origin,
            display_rect.visible_size,
            display_rect.sample_offset,
            display_rect.full_size,
            opacity,
        );
        GpuDrawState::new(
            source_bind_group,
            self.params_bind_group_for(device, params),
            Vec::new(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn prepare_pyramid_downscale_draw_state(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        source_key: GpuPaintSourceKey,
        source_bind_group: Arc<wgpu::BindGroup>,
        source_size: [usize; 2],
        output_size: [usize; 2],
        effects: ViewEffects,
        downscaler: WgpuDownscaleMethod,
        origin: [u32; 2],
        target_size: [u32; 2],
        opacity: f32,
    ) -> GpuDrawState {
        let mut pins = Vec::new();
        let mut current_bind_group = source_bind_group.clone();
        let mut current_size = output_size;
        let mut first_stage = true;
        let mut stage_index = 0u32;

        while needs_multi_pass_downscale(current_size, target_size) {
            let stage_size = next_pyramid_stage_size(current_size, target_size);
            if stage_size == current_size.map(|dimension| dimension as u32) {
                break;
            }
            let stage_filter = downscaler.pyramid_stage_filter();
            let intermediate = self.render_downscale_stage(
                device,
                encoder,
                source_key,
                source_size,
                output_size,
                effects,
                downscaler,
                stage_filter,
                &current_bind_group,
                current_size,
                stage_size,
                stage_index,
                first_stage,
            );
            current_bind_group = intermediate.bind_group.clone();
            current_size = intermediate.size;
            pins.push(intermediate);
            first_stage = false;
            stage_index = stage_index.saturating_add(1);
        }

        let final_size = target_size.map(|dimension| dimension.max(1) as usize);
        if current_size != final_size {
            let intermediate = self.render_downscale_stage(
                device,
                encoder,
                source_key,
                source_size,
                output_size,
                effects,
                downscaler,
                downscaler.base_filter(),
                &current_bind_group,
                current_size,
                target_size.map(|dimension| dimension.max(1)),
                stage_index,
                first_stage,
            );
            current_bind_group = intermediate.bind_group.clone();
            current_size = intermediate.size;
            pins.push(intermediate);
        }

        if pins.is_empty() {
            let params = params_for_effects(
                source_size,
                output_size,
                effects,
                WgpuUpscaleMethod::None,
                downscaler.base_filter(),
                origin,
                target_size,
                opacity,
            );
            return GpuDrawState::new(
                source_bind_group,
                self.params_bind_group_for(device, params),
                Vec::new(),
            );
        }

        let params = params_for_effects(
            current_size,
            current_size,
            ViewEffects::default(),
            WgpuUpscaleMethod::None,
            WgpuDownscaleMethod::Bilinear,
            origin,
            target_size,
            opacity,
        );
        GpuDrawState::new(
            current_bind_group,
            self.params_bind_group_for(device, params),
            pins,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn render_downscale_stage(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        source_key: GpuPaintSourceKey,
        source_size: [usize; 2],
        output_size: [usize; 2],
        effects: ViewEffects,
        downscaler: WgpuDownscaleMethod,
        stage_filter: WgpuDownscaleMethod,
        current_bind_group: &wgpu::BindGroup,
        current_size: [usize; 2],
        stage_size: [u32; 2],
        stage_index: u32,
        first_stage: bool,
    ) -> Arc<GpuIntermediateTexture> {
        let stage_size = stage_size.map(|dimension| dimension.max(1));
        let stage_key = downscale_intermediate_texture_key(
            "pyramid",
            source_key,
            source_size,
            output_size,
            effects,
            downscaler,
            [stage_size[0], stage_size[1]],
            current_size,
            stage_index,
        );
        self.ensure_intermediate_texture(device, stage_key, stage_size);
        let intermediate = self
            .intermediate_textures
            .peek(&stage_key)
            .expect("pyramid stage texture should be cached before rendering")
            .clone();
        let params = if first_stage {
            params_for_effects(
                source_size,
                output_size,
                effects,
                WgpuUpscaleMethod::None,
                stage_filter,
                [0, 0],
                stage_size,
                1.0,
            )
        } else {
            params_for_effects(
                current_size,
                current_size,
                ViewEffects::default(),
                WgpuUpscaleMethod::None,
                stage_filter,
                [0, 0],
                stage_size,
                1.0,
            )
        };
        let params_bind_group = self.params_bind_group_for(device, params);
        let stage_view = intermediate
            .mip_views
            .first()
            .expect("pyramid stage textures should expose a renderable mip 0 view");
        self.render_fullscreen(encoder, stage_view, current_bind_group, &params_bind_group);
        intermediate
    }

    #[allow(clippy::too_many_arguments)]
    fn prepare_hardware_mipmap_draw_state(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        source_key: GpuPaintSourceKey,
        source_bind_group: Arc<wgpu::BindGroup>,
        source_size: [usize; 2],
        output_size: [usize; 2],
        effects: ViewEffects,
        display_rect: GpuDisplayRect,
        opacity: f32,
    ) -> GpuDrawState {
        let mip_levels = mip_level_count(output_size);
        let mip_key =
            mipmap_intermediate_texture_key(source_key, source_size, output_size, effects);
        self.ensure_mipmapped_intermediate_texture(
            device,
            mip_key,
            output_size.map(|dimension| dimension.max(1) as u32),
            mip_levels,
        );
        let intermediate = self
            .intermediate_textures
            .peek(&mip_key)
            .expect("mipmapped intermediate texture should be cached before rendering")
            .clone();
        let mip0_params = params_for_effects(
            source_size,
            output_size,
            effects,
            WgpuUpscaleMethod::None,
            WgpuDownscaleMethod::Bilinear,
            [0, 0],
            output_size.map(|dimension| dimension.max(1) as u32),
            1.0,
        );
        let mip0_params_bind_group = self.params_bind_group_for(device, mip0_params);
        self.render_fullscreen(
            encoder,
            &intermediate.mip_views[0],
            &source_bind_group,
            &mip0_params_bind_group,
        );

        for level in 1..mip_levels {
            let prev_size = mip_size(output_size, level - 1);
            let next_size = mip_size(output_size, level);
            let prev_bind_group =
                self.texture_bind_group_for(device, &intermediate.mip_views[level as usize - 1]);
            let params = params_for_hardware_mipmap_sample(
                prev_size,
                [0, 0],
                next_size.map(|dimension| dimension as u32),
                1.0,
                0.0,
            );
            let params_bind_group = self.params_bind_group_for(device, params);
            self.render_fullscreen(
                encoder,
                &intermediate.mip_views[level as usize],
                &prev_bind_group,
                &params_bind_group,
            );
        }

        let lod = downscale_lod(output_size, display_rect.full_size)
            .min(mip_levels.saturating_sub(1) as f32);
        let params = params_for_hardware_mipmap_sample_with_display(
            output_size,
            display_rect.origin,
            display_rect.visible_size,
            display_rect.sample_offset,
            display_rect.full_size,
            opacity,
            lod,
        );
        GpuDrawState::new(
            intermediate.bind_group.clone(),
            self.params_bind_group_for(device, params),
            vec![intermediate],
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
        let byte_size = texture_byte_size(target_size, 1);
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
        let mip_views = vec![texture.create_view(&mip_view_descriptor(0))];
        let bind_group = Arc::new(self.texture_bind_group_for(device, &view));
        if let Some((_old_key, old_texture)) = self.intermediate_textures.push(
            key,
            Arc::new(GpuIntermediateTexture {
                _texture: texture,
                _view: view,
                mip_views,
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

    fn ensure_mipmapped_intermediate_texture(
        &mut self,
        device: &wgpu::Device,
        key: u64,
        target_size: [u32; 2],
        mip_levels: u32,
    ) {
        if self.intermediate_textures.get(&key).is_some() {
            return;
        }
        let byte_size = texture_byte_size(target_size, mip_levels);
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("suisuiview-gpu-downscale-mipmap-intermediate"),
            size: wgpu::Extent3d {
                width: target_size[0],
                height: target_size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: mip_levels,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mip_views = (0..mip_levels)
            .map(|level| texture.create_view(&mip_view_descriptor(level)))
            .collect::<Vec<_>>();
        let bind_group = Arc::new(self.texture_bind_group_for(device, &view));
        if let Some((_old_key, old_texture)) = self.intermediate_textures.push(
            key,
            Arc::new(GpuIntermediateTexture {
                _texture: texture,
                _view: view,
                mip_views,
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

    fn should_defer_realtime_sr_first_frame(
        &mut self,
        key: u64,
        method: WgpuUpscaleMethod,
    ) -> bool {
        if !defer_initial_realtime_sr_frame(method)
            || self.intermediate_textures.peek(&key).is_some()
        {
            return false;
        }
        if self.deferred_realtime_sr_first_frames.get(&key).is_some() {
            return false;
        }
        self.deferred_realtime_sr_first_frames.push(key, ());
        true
    }

    fn ensure_realtime_sr_texture(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        key: u64,
        source_key: GpuPaintSourceKey,
        source_size: [usize; 2],
        method: WgpuUpscaleMethod,
    ) {
        if self.intermediate_textures.get(&key).is_some() {
            return;
        }
        let Some(source) = self.source_textures.peek(&source_key) else {
            return;
        };
        let Some(output) =
            self.realtime_sr
                .render(method, key, device, encoder, &source.view, source_size)
        else {
            return;
        };
        let output_size = output.size;
        let output_byte_size = output.byte_size;
        let bind_group = Arc::new(self.texture_bind_group_for(device, &output.view));
        let mip_views = vec![output.texture.create_view(&mip_view_descriptor(0))];
        let evicted_on_insert = if let Some((_old_key, old_texture)) =
            self.intermediate_textures.push(
                key,
                Arc::new(GpuIntermediateTexture {
                    _texture: output.texture,
                    _view: output.view,
                    mip_views,
                    bind_group,
                    size: output_size,
                    byte_size: output_byte_size,
                }),
            ) {
            self.intermediate_texture_bytes = self
                .intermediate_texture_bytes
                .saturating_sub(old_texture.byte_size);
            true
        } else {
            false
        };
        self.intermediate_texture_bytes = self
            .intermediate_texture_bytes
            .saturating_add(output_byte_size);
        self.prune_intermediate_textures();
        record_realtime_sr_texture_ready(
            method,
            source_size,
            output_size,
            output_byte_size,
            self.intermediate_textures.len(),
            self.intermediate_texture_bytes,
            evicted_on_insert,
        );
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
        pass.set_pipeline(&self.intermediate_pipeline);
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
fn record_wgpu_upscale_method_render(
    method: WgpuUpscaleMethod,
    source_size: [usize; 2],
    output_size: [usize; 2],
    target_size: [u32; 2],
    rendered_size: [usize; 2],
    path: &'static str,
) {
    perf_trace::record_duration(
        "wgpu_upscale_method_render",
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

#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
fn record_realtime_sr_texture_ready(
    method: WgpuUpscaleMethod,
    source_size: [usize; 2],
    output_size: [usize; 2],
    output_byte_size: usize,
    cache_entries: usize,
    cache_bytes: usize,
    evicted_on_insert: bool,
) {
    perf_trace::record_duration(
        "realtime_sr_texture_ready",
        Duration::ZERO,
        &[
            PerfField::Str("method", method.token()),
            PerfField::Usize("source_width", source_size[0]),
            PerfField::Usize("source_height", source_size[1]),
            PerfField::Usize("output_width", output_size[0]),
            PerfField::Usize("output_height", output_size[1]),
            PerfField::Usize("output_bytes", output_byte_size),
            PerfField::Usize("cache_entries", cache_entries),
            PerfField::Usize("cache_bytes", cache_bytes),
            PerfField::Bool("evicted_on_insert", evicted_on_insert),
        ],
    );
}

#[cfg(not(any(feature = "perf-dev", feature = "perf-diagnostics")))]
fn record_realtime_sr_texture_ready(
    _method: WgpuUpscaleMethod,
    _source_size: [usize; 2],
    _output_size: [usize; 2],
    _output_byte_size: usize,
    _cache_entries: usize,
    _cache_bytes: usize,
    _evicted_on_insert: bool,
) {
}

#[cfg(not(any(feature = "perf-dev", feature = "perf-diagnostics")))]
fn record_wgpu_upscale_method_render(
    _method: WgpuUpscaleMethod,
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
    wgpu_upscale_method: WgpuUpscaleMethod,
    wgpu_downscale_method: WgpuDownscaleMethod,
    rect: Rect,
    opacity: f32,
) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    source_key.hash(&mut hasher);
    effects.hash(&mut hasher);
    wgpu_upscale_method.token().hash(&mut hasher);
    wgpu_downscale_method.token().hash(&mut hasher);
    rect.min.x.to_bits().hash(&mut hasher);
    rect.min.y.to_bits().hash(&mut hasher);
    rect.max.x.to_bits().hash(&mut hasher);
    rect.max.y.to_bits().hash(&mut hasher);
    opacity.to_bits().hash(&mut hasher);
    hasher.finish()
}

fn viewport_rect(rect: Rect, screen_descriptor: &ScreenDescriptor) -> GpuDisplayRect {
    let screen_width = screen_descriptor.size_in_pixels[0] as i32;
    let screen_height = screen_descriptor.size_in_pixels[1] as i32;
    let pixels_per_point = screen_descriptor.pixels_per_point;
    let full_left = (pixels_per_point * rect.min.x).round() as i32;
    let full_top = (pixels_per_point * rect.min.y).round() as i32;
    let full_right = (pixels_per_point * rect.max.x).round() as i32;
    let full_bottom = (pixels_per_point * rect.max.y).round() as i32;
    let left = full_left.clamp(0, screen_width) as u32;
    let top = full_top.clamp(0, screen_height) as u32;
    let right_raw = full_right.clamp(0, screen_width) as u32;
    let bottom_raw = full_bottom.clamp(0, screen_height) as u32;
    let right = right_raw
        .max(left.saturating_add(1))
        .min(screen_width as u32);
    let bottom = bottom_raw
        .max(top.saturating_add(1))
        .min(screen_height as u32);
    GpuDisplayRect {
        origin: [left, top],
        visible_size: [
            right.saturating_sub(left).max(1),
            bottom.saturating_sub(top).max(1),
        ],
        sample_offset: [
            (left as i32).saturating_sub(full_left).max(0) as u32,
            (top as i32).saturating_sub(full_top).max(0) as u32,
        ],
        full_size: [
            full_right.saturating_sub(full_left).max(1) as u32,
            full_bottom.saturating_sub(full_top).max(1) as u32,
        ],
    }
}

fn intermediate_texture_key(
    source_key: GpuPaintSourceKey,
    source_size: [usize; 2],
    output_size: [usize; 2],
    effects: ViewEffects,
    wgpu_upscale_method: WgpuUpscaleMethod,
    display_rect: GpuDisplayRect,
) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    source_key.hash(&mut hasher);
    source_size.hash(&mut hasher);
    output_size.hash(&mut hasher);
    effects.hash(&mut hasher);
    wgpu_upscale_method.token().hash(&mut hasher);
    display_rect.visible_size.hash(&mut hasher);
    display_rect.sample_offset.hash(&mut hasher);
    display_rect.full_size.hash(&mut hasher);
    hasher.finish()
}

fn downscale_intermediate_texture_key(
    namespace: &'static str,
    source_key: GpuPaintSourceKey,
    source_size: [usize; 2],
    output_size: [usize; 2],
    effects: ViewEffects,
    downscaler: WgpuDownscaleMethod,
    stage_size: [u32; 2],
    current_size: [usize; 2],
    stage_index: u32,
) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    namespace.hash(&mut hasher);
    source_key.hash(&mut hasher);
    source_size.hash(&mut hasher);
    output_size.hash(&mut hasher);
    effects.hash(&mut hasher);
    downscaler.token().hash(&mut hasher);
    stage_size.hash(&mut hasher);
    current_size.hash(&mut hasher);
    stage_index.hash(&mut hasher);
    hasher.finish()
}

fn mipmap_intermediate_texture_key(
    source_key: GpuPaintSourceKey,
    source_size: [usize; 2],
    output_size: [usize; 2],
    effects: ViewEffects,
) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    "hardware_mipmap_linear".hash(&mut hasher);
    source_key.hash(&mut hasher);
    source_size.hash(&mut hasher);
    output_size.hash(&mut hasher);
    effects.hash(&mut hasher);
    hasher.finish()
}

fn needs_multi_pass_downscale(source_size: [usize; 2], target_size: [u32; 2]) -> bool {
    downscale_ratio(source_size, target_size) > 2.0
}

fn downscale_ratio(source_size: [usize; 2], target_size: [u32; 2]) -> f32 {
    let target_width = target_size[0].max(1) as f32;
    let target_height = target_size[1].max(1) as f32;
    ((source_size[0].max(1) as f32) / target_width)
        .max((source_size[1].max(1) as f32) / target_height)
}

fn downscale_lod(source_size: [usize; 2], target_size: [u32; 2]) -> f32 {
    downscale_ratio(source_size, target_size).max(1.0).log2()
}

fn next_pyramid_stage_size(current_size: [usize; 2], target_size: [u32; 2]) -> [u32; 2] {
    [
        next_pyramid_stage_dimension(current_size[0], target_size[0]),
        next_pyramid_stage_dimension(current_size[1], target_size[1]),
    ]
}

fn next_pyramid_stage_dimension(current: usize, target: u32) -> u32 {
    let current = current.max(1);
    let target = target.max(1) as usize;
    if target >= current {
        current as u32
    } else if current > target.saturating_mul(2) {
        ((current + 1) / 2).max(target) as u32
    } else {
        target as u32
    }
}

fn mip_level_count(size: [usize; 2]) -> u32 {
    let mut width = size[0].max(1);
    let mut height = size[1].max(1);
    let mut levels = 1u32;
    while width > 1 || height > 1 {
        width = (width / 2).max(1);
        height = (height / 2).max(1);
        levels = levels.saturating_add(1);
    }
    levels
}

fn mip_size(size: [usize; 2], level: u32) -> [usize; 2] {
    [
        size[0].checked_shr(level).unwrap_or(0).max(1),
        size[1].checked_shr(level).unwrap_or(0).max(1),
    ]
}

fn create_effect_pipeline_timed(
    device: &wgpu::Device,
    shader: &wgpu::ShaderModule,
    pipeline_layout: &wgpu::PipelineLayout,
    target_format: wgpu::TextureFormat,
    label: &'static str,
) -> wgpu::RenderPipeline {
    #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
    let started = Instant::now();
    let pipeline = create_effect_pipeline(device, shader, pipeline_layout, target_format, label);
    #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
    perf_trace::record_duration(
        "gpu_effect_pipeline_create",
        started.elapsed(),
        &[
            PerfField::Str("label", label),
            PerfField::Str("target_format", texture_format_label(target_format)),
        ],
    );
    pipeline
}

fn create_effect_pipeline(
    device: &wgpu::Device,
    shader: &wgpu::ShaderModule,
    pipeline_layout: &wgpu::PipelineLayout,
    target_format: wgpu::TextureFormat,
    label: &'static str,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(pipeline_layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
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
    })
}

#[cfg_attr(
    not(any(feature = "perf-dev", feature = "perf-diagnostics")),
    allow(dead_code)
)]
fn texture_format_label(format: wgpu::TextureFormat) -> &'static str {
    match format {
        wgpu::TextureFormat::Rgba8Unorm => "rgba8_unorm",
        wgpu::TextureFormat::Rgba8UnormSrgb => "rgba8_unorm_srgb",
        wgpu::TextureFormat::Bgra8Unorm => "bgra8_unorm",
        wgpu::TextureFormat::Bgra8UnormSrgb => "bgra8_unorm_srgb",
        _ => "other",
    }
}

fn texture_byte_size(size: [u32; 2], mip_levels: u32) -> usize {
    (0..mip_levels)
        .map(|level| {
            let mip_size = mip_size([size[0] as usize, size[1] as usize], level);
            mip_size[0].saturating_mul(mip_size[1]).saturating_mul(4)
        })
        .sum()
}

fn mip_view_descriptor(base_mip_level: u32) -> wgpu::TextureViewDescriptor<'static> {
    wgpu::TextureViewDescriptor {
        label: Some("suisuiview-gpu-effect-mip-view"),
        dimension: Some(wgpu::TextureViewDimension::D2),
        base_mip_level,
        mip_level_count: Some(1),
        ..Default::default()
    }
}

fn defer_initial_realtime_sr_frame(method: WgpuUpscaleMethod) -> bool {
    method.is_artcnn() || matches!(method, WgpuUpscaleMethod::WgslSrLabSpanX2)
}

fn realtime_sr_texture_key(
    source_key: GpuPaintSourceKey,
    source_size: [usize; 2],
    wgpu_upscale_method: WgpuUpscaleMethod,
) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    source_key.hash(&mut hasher);
    source_size.hash(&mut hasher);
    wgpu_upscale_method.token().hash(&mut hasher);
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
        };
        let left = Rect::from_min_size(pos2(0.0, 0.0), vec2(640.0, 900.0));
        let right = Rect::from_min_size(pos2(640.0, 0.0), vec2(640.0, 900.0));

        assert_ne!(
            draw_id(
                source_key,
                ViewEffects::default(),
                WgpuUpscaleMethod::WgslFsr1Style,
                WgpuDownscaleMethod::Bilinear,
                left,
                1.0,
            ),
            draw_id(
                source_key,
                ViewEffects::default(),
                WgpuUpscaleMethod::WgslFsr1Style,
                WgpuDownscaleMethod::Bilinear,
                right,
                1.0,
            )
        );
    }

    #[test]
    fn viewport_rect_keeps_full_target_for_oversized_clipped_rect() {
        let screen = ScreenDescriptor {
            size_in_pixels: [800, 600],
            pixels_per_point: 1.0,
        };
        let rect = Rect::from_min_size(pos2(-200.0, -100.0), vec2(1000.0, 800.0));

        assert_eq!(
            viewport_rect(rect, &screen),
            GpuDisplayRect {
                origin: [0, 0],
                visible_size: [800, 600],
                sample_offset: [200, 100],
                full_size: [1000, 800],
            }
        );
    }

    #[test]
    fn viewport_rect_converts_points_to_physical_pixels() {
        let screen = ScreenDescriptor {
            size_in_pixels: [800, 600],
            pixels_per_point: 1.5,
        };
        let rect = Rect::from_min_size(pos2(10.0, 20.0), vec2(200.0, 100.0));

        assert_eq!(
            viewport_rect(rect, &screen),
            GpuDisplayRect {
                origin: [15, 30],
                visible_size: [300, 150],
                sample_offset: [0, 0],
                full_size: [300, 150],
            }
        );
    }

    #[test]
    fn manual_and_original_modes_disable_display_upscalers() {
        assert!(!fit_mode_allows_display_upscale(FitMode::Manual));
        assert!(!fit_mode_allows_display_upscale(FitMode::Original));
        assert!(fit_mode_allows_display_upscale(FitMode::FitPage));
        assert!(fit_mode_allows_display_upscale(FitMode::FitWidth));
        assert!(fit_mode_allows_display_upscale(FitMode::FitHeight));
    }

    #[test]
    fn experimental_wgpu_upscale_method_parses_hidden_artcnn() {
        assert_eq!(
            parse_experimental_wgpu_upscale_method(Some(" artcnn_c4f16 "), false, false),
            Some(WgpuUpscaleMethod::WgslArtcnnC4F16)
        );
        assert_eq!(
            parse_experimental_wgpu_upscale_method(Some("artcnn_c4f32_ds"), false, false),
            Some(WgpuUpscaleMethod::WgslArtcnnC4F32Ds)
        );
    }

    #[test]
    fn experimental_wgpu_upscale_method_keeps_span_manifest_gated() {
        assert_eq!(
            parse_experimental_wgpu_upscale_method(Some("srlab_span_x2"), false, false),
            None
        );
        assert_eq!(
            parse_experimental_wgpu_upscale_method(Some("srlab_span_x2"), false, true),
            Some(WgpuUpscaleMethod::WgslSrLabSpanX2)
        );
        assert_eq!(
            parse_experimental_wgpu_upscale_method(None, true, true),
            Some(WgpuUpscaleMethod::WgslSrLabSpanX2)
        );
    }

    #[test]
    fn settings_span_upscaler_requires_manifest_for_render() {
        assert_eq!(
            wgpu_upscale_method_from_settings(WgpuUpscaleMethod::WgslSrLabSpanX2, false),
            WgpuUpscaleMethod::Auto
        );
        assert_eq!(
            wgpu_upscale_method_from_settings(WgpuUpscaleMethod::WgslSrLabSpanX2, true),
            WgpuUpscaleMethod::WgslSrLabSpanX2
        );
        assert_eq!(
            wgpu_upscale_method_from_settings(WgpuUpscaleMethod::WgslArtcnnC4F16, false),
            WgpuUpscaleMethod::WgslArtcnnC4F16
        );
        assert_eq!(
            wgpu_upscale_method_from_settings(WgpuUpscaleMethod::NvidiaNis, true),
            WgpuUpscaleMethod::Auto
        );
    }

    #[test]
    fn hidden_realtime_sr_methods_defer_the_first_frame() {
        assert!(defer_initial_realtime_sr_frame(
            WgpuUpscaleMethod::WgslArtcnnC4F16
        ));
        assert!(defer_initial_realtime_sr_frame(
            WgpuUpscaleMethod::WgslArtcnnC4F32Ds
        ));
        assert!(defer_initial_realtime_sr_frame(
            WgpuUpscaleMethod::WgslSrLabSpanX2
        ));
        assert!(!defer_initial_realtime_sr_frame(
            WgpuUpscaleMethod::WgslAnime4kV32CnnX2S
        ));
    }

    #[test]
    fn pyramid_stage_size_halves_until_target_is_within_two_x() {
        assert!(needs_multi_pass_downscale([4096, 4096], [1024, 1024]));
        assert_eq!(
            next_pyramid_stage_size([4096, 4096], [1024, 1024]),
            [2048, 2048]
        );
        assert!(!needs_multi_pass_downscale([2048, 2048], [1024, 1024]));
        assert_eq!(
            next_pyramid_stage_size([4096, 1000], [1024, 1200]),
            [2048, 1000]
        );
    }

    #[test]
    fn mip_helpers_match_expected_floor_chain() {
        assert_eq!(mip_level_count([4096, 1024]), 13);
        assert_eq!(mip_size([4096, 1024], 0), [4096, 1024]);
        assert_eq!(mip_size([4096, 1024], 2), [1024, 256]);
        assert_eq!(mip_size([3, 3], 1), [1, 1]);
    }

    #[test]
    #[ignore = "requires a local WGPU adapter and reads back large render targets"]
    fn wgpu_pyramid_downscalers_render_nonblank_output() {
        let cases = [
            WgpuDownscaleMethod::HardwareMipmapLinear,
            WgpuDownscaleMethod::PyramidBoxTent,
            WgpuDownscaleMethod::PyramidHamming,
            WgpuDownscaleMethod::PyramidMitchell,
            WgpuDownscaleMethod::PyramidLanczos2,
            WgpuDownscaleMethod::PyramidLanczos3,
        ];
        pollster::block_on(async {
            let Some((device, queue)) = smoke_device().await else {
                eprintln!("Skipping WGPU downscaler smoke: no adapter available");
                return;
            };
            for source_size in [[2048, 2048], [4096, 4096]] {
                for downscaler in cases {
                    assert!(
                        render_downscale_smoke(&device, &queue, source_size, downscaler),
                        "{} {:?} -> 1024x1024 produced a blank output",
                        downscaler.token(),
                        source_size
                    );
                }
            }
        });
    }

    #[test]
    #[ignore = "release timing probe for local GPU downscale paths"]
    fn wgpu_default_downscaler_timing_probe() {
        pollster::block_on(async {
            let Some((device, queue)) = smoke_device().await else {
                eprintln!("Skipping WGPU downscaler timing: no adapter available");
                return;
            };
            for source_size in [[2048, 2048], [4096, 4096]] {
                for downscaler in [
                    WgpuDownscaleMethod::Hamming,
                    WgpuDownscaleMethod::PyramidLanczos3,
                ] {
                    let mut fixture = DownscaleSmokeFixture::new(&device, &queue, source_size);
                    for _ in 0..2 {
                        assert!(render_downscale_frame(
                            &device,
                            &queue,
                            &mut fixture,
                            downscaler
                        ));
                    }
                    let mut samples = Vec::with_capacity(12);
                    for _ in 0..12 {
                        let started = std::time::Instant::now();
                        assert!(render_downscale_frame(
                            &device,
                            &queue,
                            &mut fixture,
                            downscaler
                        ));
                        samples.push(started.elapsed().as_secs_f64() * 1000.0);
                    }
                    samples.sort_by(|left, right| left.total_cmp(right));
                    let avg = samples.iter().sum::<f64>() / samples.len() as f64;
                    let p95_index = (samples.len() * 95).div_ceil(100).saturating_sub(1);
                    println!(
                        "wgpu_downscale_timing source={}x{} target=1024x1024 method={} avg_ms={:.3} p95_ms={:.3}",
                        source_size[0],
                        source_size[1],
                        downscaler.token(),
                        avg,
                        samples[p95_index]
                    );
                }
            }
        });
    }

    async fn smoke_device() -> Option<(wgpu::Device, wgpu::Queue)> {
        let instance = wgpu::Instance::default();
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: None,
            })
            .await
            .ok()?;
        adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("suisuiview-gpu-downscale-smoke-device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::downlevel_webgl2_defaults()
                    .using_resolution(adapter.limits()),
                memory_hints: wgpu::MemoryHints::Performance,
                trace: wgpu::Trace::Off,
            })
            .await
            .ok()
    }

    fn render_downscale_smoke(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        source_size: [usize; 2],
        downscaler: WgpuDownscaleMethod,
    ) -> bool {
        let mut fixture = DownscaleSmokeFixture::new(device, queue, source_size);
        render_downscale_frame(device, queue, &mut fixture, downscaler)
    }

    struct DownscaleSmokeFixture {
        resources: GpuPaintResources,
        source_key: GpuPaintSourceKey,
        source_size: [usize; 2],
    }

    impl DownscaleSmokeFixture {
        fn new(device: &wgpu::Device, queue: &wgpu::Queue, source_size: [usize; 2]) -> Self {
            let mut resources = GpuPaintResources::new(device, wgpu::TextureFormat::Rgba8Unorm);
            let source_key = GpuPaintSourceKey {
                book: 1,
                page: PageCacheKey {
                    index: source_size[0],
                    target_long_edge: source_size[0] as u32,
                    decode: DecodeOptions::default(),
                },
            };
            let rgba = smoke_rgba(source_size);
            resources.ensure_source_texture(device, queue, source_key, source_size, &rgba);
            Self {
                resources,
                source_key,
                source_size,
            }
        }
    }

    fn render_downscale_frame(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        fixture: &mut DownscaleSmokeFixture,
        downscaler: WgpuDownscaleMethod,
    ) -> bool {
        let resources = &mut fixture.resources;
        let source_key = fixture.source_key;
        let source_bind_group = resources
            .source_textures
            .peek(&source_key)
            .expect("smoke source texture should be uploaded")
            .bind_group
            .clone();
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("suisuiview-gpu-downscale-smoke-encoder"),
        });
        let draw_state = resources.prepare_draw_state(
            device,
            &mut encoder,
            source_key,
            source_bind_group,
            fixture.source_size,
            fixture.source_size,
            ViewEffects::default(),
            WgpuUpscaleMethod::None,
            downscaler,
            GpuDisplayRect {
                origin: [0, 0],
                visible_size: [1024, 1024],
                sample_offset: [0, 0],
                full_size: [1024, 1024],
            },
            1.0,
            &egui::Context::default(),
        );
        let output_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("suisuiview-gpu-downscale-smoke-output"),
            size: wgpu::Extent3d {
                width: 1024,
                height: 1024,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let output_view = output_texture.create_view(&wgpu::TextureViewDescriptor::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("suisuiview-gpu-downscale-smoke-pass"),
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
            pass.set_pipeline(&resources.pipeline);
            pass.set_bind_group(0, draw_state.texture_bind_group.as_ref(), &[]);
            pass.set_bind_group(1, &draw_state.params_bind_group, &[]);
            pass.draw(0..3, 0..1);
        }

        let padded_bytes_per_row = align_to_smoke(1024 * 4, wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("suisuiview-gpu-downscale-smoke-readback"),
            size: padded_bytes_per_row as u64 * 1024,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
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
                    rows_per_image: Some(1024),
                },
            },
            wgpu::Extent3d {
                width: 1024,
                height: 1024,
                depth_or_array_layers: 1,
            },
        );
        queue.submit(Some(encoder.finish()));
        let buffer_slice = readback.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        device.poll(wgpu::PollType::Wait).unwrap();
        rx.recv().unwrap().unwrap();
        let mapped = buffer_slice.get_mapped_range();
        let nonblank = mapped
            .chunks_exact(4)
            .any(|pixel| pixel[0] != 0 || pixel[1] != 0 || pixel[2] != 0 || pixel[3] != 0);
        drop(mapped);
        readback.unmap();
        nonblank
    }

    fn smoke_rgba(size: [usize; 2]) -> Vec<u8> {
        let [width, height] = size;
        let mut rgba = vec![0u8; width * height * 4];
        for y in 0..height {
            for x in 0..width {
                let offset = (y * width + x) * 4;
                rgba[offset] = (x % 251) as u8;
                rgba[offset + 1] = (y % 241) as u8;
                rgba[offset + 2] = ((x + y) % 239) as u8;
                rgba[offset + 3] = 255;
            }
        }
        rgba
    }

    fn align_to_smoke(value: u32, alignment: u32) -> u32 {
        value.div_ceil(alignment) * alignment
    }
}
