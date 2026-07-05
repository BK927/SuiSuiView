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
use crate::core::worker::PagePixels;
use egui::{self, PaintCallbackInfo, Rect};
use egui_wgpu::{CallbackResources, CallbackTrait, ScreenDescriptor};
use lru::LruCache;
use std::borrow::Cow;
use std::hash::{Hash, Hasher};
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicUsize, Ordering};
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

// Read-only mirrors of the render-thread-owned `GpuPaintResources` byte counters, so the
// app/UI thread can display live GPU pool usage. These are purely for visibility: budget and
// eviction logic still runs off the owning fields. Each mutating method republishes the field
// values via `publish_gpu_pool_bytes`, and `GpuPaintResources::new` resets them to 0.
static GPU_SOURCE_TEXTURE_BYTES_LIVE: AtomicUsize = AtomicUsize::new(0);
static GPU_INTERMEDIATE_TEXTURE_BYTES_LIVE: AtomicUsize = AtomicUsize::new(0);
static GPU_DRAW_STATE_BYTES_LIVE: AtomicUsize = AtomicUsize::new(0);

/// Live GPU pool usage in bytes as `(source_textures, intermediate_textures, draw_states)`.
/// Reflects the most recent state published by the render thread; returns zeros before any
/// GPU paint resources exist.
pub(crate) fn gpu_pool_bytes_live() -> (usize, usize, usize) {
    (
        GPU_SOURCE_TEXTURE_BYTES_LIVE.load(Ordering::Relaxed),
        GPU_INTERMEDIATE_TEXTURE_BYTES_LIVE.load(Ordering::Relaxed),
        GPU_DRAW_STATE_BYTES_LIVE.load(Ordering::Relaxed),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct GpuPaintSourceKey {
    pub(super) book: u64,
    pub(super) page: PageCacheKey,
}

pub(super) struct GpuPaintRequest {
    pub(super) rect: Rect,
    pub(super) source_key: GpuPaintSourceKey,
    pub(super) image_size: [usize; 2],
    pub(super) pixels: PagePixels,
    pub(super) effects: ViewEffects,
    pub(super) wgpu_upscale_method: WgpuUpscaleMethod,
    pub(super) wgpu_downscale_method: WgpuDownscaleMethod,
    pub(super) opacity: f32,
}

/// GPU pool budgets (bytes) carried from the app/settings thread into the render thread. The
/// render thread owns eviction, so each paint restates the current caps and the prune routines
/// enforce them; [`GPU_SOURCE_TEXTURE_BUDGET_BYTES`] / [`GPU_INTERMEDIATE_TEXTURE_BUDGET_BYTES`]
/// remain as the built-in floor if a paint ever arrives before the caps are published.
#[derive(Debug, Clone, Copy)]
pub(super) struct GpuPoolBudgets {
    pub(super) source_texture_bytes: usize,
    pub(super) intermediate_texture_bytes: usize,
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
        let pool_budgets = GpuPoolBudgets {
            source_texture_bytes: super::gpu_source_texture_budget_bytes(&self.settings),
            intermediate_texture_bytes: super::gpu_intermediate_texture_budget_bytes(
                &self.settings,
            ),
        };
        let callback = GpuEffectCallback {
            source_key: request.source_key,
            image_size: request.image_size,
            pixels: request.pixels,
            effects: request.effects,
            wgpu_upscale_method: request.wgpu_upscale_method,
            wgpu_downscale_method: request.wgpu_downscale_method,
            opacity: request.opacity.clamp(0.0, 1.0),
            pool_budgets,
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
    pixels: PagePixels,
    effects: ViewEffects,
    wgpu_upscale_method: WgpuUpscaleMethod,
    wgpu_downscale_method: WgpuDownscaleMethod,
    opacity: f32,
    pool_budgets: GpuPoolBudgets,
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
        // Restate the settings-derived pool caps before any upload/prune this frame. The app side
        // already floored them at the functional minimum (current page + SR round-trip), so the
        // total budget dominates from here.
        resources.source_texture_budget_bytes = self.pool_budgets.source_texture_bytes;
        resources.intermediate_texture_budget_bytes = self.pool_budgets.intermediate_texture_bytes;
        let source_uploaded = resources.ensure_source_texture(
            device,
            queue,
            self.source_key,
            self.image_size,
            &self.pixels,
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
    // Current pool caps, restated by the app on every paint. Seeded with the built-in constants so
    // eviction is well-defined even before the first `prepare` publishes the settings-derived caps.
    source_texture_budget_bytes: usize,
    intermediate_texture_budget_bytes: usize,
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
    content_key: u64,
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

    fn with_intermediate_pin(mut self, intermediate: Arc<GpuIntermediateTexture>) -> Self {
        self.intermediate_byte_size = self
            .intermediate_byte_size
            .saturating_add(intermediate.byte_size);
        self._intermediate_pins.push(intermediate);
        self
    }
}

impl GpuPaintResources {
    fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Self {
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
            source_texture_budget_bytes: GPU_SOURCE_TEXTURE_BUDGET_BYTES,
            intermediate_texture_budget_bytes: GPU_INTERMEDIATE_TEXTURE_BUDGET_BYTES,
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
        // Reset the read-only mirrors so a recreation (e.g. target-format change) does not leave
        // stale byte counts visible to the UI thread.
        resources.publish_gpu_pool_bytes();
        resources
    }

    fn ensure_source_texture(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        key: GpuPaintSourceKey,
        image_size: [usize; 2],
        pixels: &PagePixels,
    ) -> bool {
        if self.source_textures.get(&key).is_some() {
            return false;
        }
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        let upload_started = Instant::now();
        let [width, height] = image_size;
        let byte_size = width.saturating_mul(height).saturating_mul(4);
        // VRAM is always RGBA. Expand luma -> RGBA here, after the LRU-miss check, so the cost is
        // paid at most once per source texture (per-frame repaints hit the early return above). For
        // RGBA pages `to_rgba_vec` just clones the retained buffer.
        let rgba = pixels.to_rgba_vec(width, height);
        if rgba.len() != byte_size {
            return false;
        }
        let rgba = rgba.as_slice();
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
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        let write_started = Instant::now();
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
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        record_gpu_texture_upload_stage("gpu_texture_write", write_started, width, height);
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        let bind_group_started = Instant::now();
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = Arc::new(self.texture_bind_group_for(device, &view));
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        record_gpu_texture_upload_stage(
            "gpu_texture_bind_group",
            bind_group_started,
            width,
            height,
        );
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
        let source_content_key =
            source_texture_content_key(source_key, source_size, output_size, effects);
        self.realtime_sr
            .cancel_inactive_pending_work(effective_upscaler);
        if RealtimeSrResources::is_supported(effective_upscaler) {
            if let Some(draw_state) = self.prepare_realtime_sr_draw_state(
                device,
                encoder,
                source_key,
                source_size,
                output_size,
                effects,
                effective_upscaler,
                wgpu_downscale_method,
                display_rect,
                opacity,
                ctx,
            ) {
                return draw_state;
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

        // Unreachable in product after settings sanitize (HardwareMipmapLinear folds
        // to Bilinear); retained for tests and potential future re-exposure.
        if effective_downscaler.is_hardware_mipmap() {
            return self.prepare_hardware_mipmap_draw_state(
                device,
                encoder,
                source_content_key,
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
                source_content_key,
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
    fn prepare_realtime_sr_draw_state(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        source_key: GpuPaintSourceKey,
        source_size: [usize; 2],
        output_size: [usize; 2],
        effects: ViewEffects,
        upscaler: WgpuUpscaleMethod,
        downscaler: WgpuDownscaleMethod,
        display_rect: GpuDisplayRect,
        opacity: f32,
        ctx: &egui::Context,
    ) -> Option<GpuDrawState> {
        let stack_passes = upscaler.fixed_2x_stack_passes(output_size, display_rect.full_size);
        let mut current_size = source_size;
        let mut current_intermediate: Option<Arc<GpuIntermediateTexture>> = None;
        let mut best_ready: Option<Arc<GpuIntermediateTexture>> = None;

        for stage_index in 0..stack_passes {
            let stage_key = realtime_sr_stage_texture_key(
                source_key,
                source_size,
                upscaler,
                stage_index,
                current_size,
                stack_passes,
            );
            if self.should_defer_realtime_sr_first_frame(stage_key, upscaler) {
                self.realtime_sr.warm_up_async(upscaler, device);
                ctx.request_repaint_after(Duration::from_millis(16));
                break;
            }

            let next_intermediate = if stage_index == 0 {
                self.ensure_realtime_sr_stage_texture_from_source(
                    device,
                    encoder,
                    stage_key,
                    source_key,
                    current_size,
                    upscaler,
                )
            } else {
                let input = current_intermediate.as_ref()?;
                self.ensure_realtime_sr_stage_texture_from_view(
                    device,
                    encoder,
                    stage_key,
                    &input._view,
                    current_size,
                    upscaler,
                )
            };
            if self.realtime_sr.has_pending_async_work(upscaler) {
                ctx.request_repaint_after(Duration::from_millis(16));
            }
            let Some(next_intermediate) = next_intermediate else {
                break;
            };

            current_size = next_intermediate.size;
            best_ready = Some(next_intermediate.clone());
            current_intermediate = Some(next_intermediate);
        }

        let intermediate = best_ready?;
        record_wgpu_upscale_method_render(
            upscaler,
            source_size,
            output_size,
            display_rect.full_size,
            intermediate.size,
            if stack_passes > 1 {
                "realtime_sr_stacked"
            } else {
                "realtime_sr"
            },
        );
        Some(self.prepare_realtime_sr_presentation_draw_state(
            device,
            encoder,
            effects,
            downscaler,
            display_rect,
            opacity,
            intermediate,
        ))
    }

    #[allow(clippy::too_many_arguments)]
    fn prepare_realtime_sr_presentation_draw_state(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        effects: ViewEffects,
        downscaler: WgpuDownscaleMethod,
        display_rect: GpuDisplayRect,
        opacity: f32,
        intermediate: Arc<GpuIntermediateTexture>,
    ) -> GpuDrawState {
        let sr_output_size = output_size_for_effects(intermediate.size, effects);
        let post_downscaler =
            post_realtime_sr_downscale_method(sr_output_size, display_rect.full_size, downscaler);
        // Unreachable in product after settings sanitize (HardwareMipmapLinear folds
        // to Bilinear); retained for tests and potential future re-exposure.
        if post_downscaler.is_hardware_mipmap() {
            return self
                .prepare_hardware_mipmap_draw_state(
                    device,
                    encoder,
                    intermediate.content_key,
                    intermediate.bind_group.clone(),
                    intermediate.size,
                    sr_output_size,
                    effects,
                    display_rect,
                    opacity,
                )
                .with_intermediate_pin(intermediate);
        }
        if post_downscaler.is_pyramid()
            && !display_rect.is_clipped()
            && needs_multi_pass_downscale(sr_output_size, display_rect.full_size)
        {
            return self
                .prepare_pyramid_downscale_draw_state(
                    device,
                    encoder,
                    intermediate.content_key,
                    intermediate.bind_group.clone(),
                    intermediate.size,
                    sr_output_size,
                    effects,
                    post_downscaler,
                    display_rect.origin,
                    display_rect.visible_size,
                    opacity,
                )
                .with_intermediate_pin(intermediate);
        }

        let params = params_for_effects_with_display(
            intermediate.size,
            sr_output_size,
            effects,
            WgpuUpscaleMethod::None,
            post_downscaler,
            display_rect.origin,
            display_rect.visible_size,
            display_rect.sample_offset,
            display_rect.full_size,
            opacity,
        );
        GpuDrawState::new(
            intermediate.bind_group.clone(),
            self.params_bind_group_for(device, params),
            vec![intermediate],
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn prepare_pyramid_downscale_draw_state(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        content_key: u64,
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
                content_key,
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
                content_key,
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
        content_key: u64,
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
            content_key,
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
        content_key: u64,
        source_bind_group: Arc<wgpu::BindGroup>,
        source_size: [usize; 2],
        output_size: [usize; 2],
        effects: ViewEffects,
        display_rect: GpuDisplayRect,
        opacity: f32,
    ) -> GpuDrawState {
        let mip_levels = mip_level_count(output_size);
        let mip_key = mipmap_intermediate_texture_key(content_key);
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
                content_key: key,
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
                content_key: key,
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

    fn ensure_realtime_sr_stage_texture_from_source(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        key: u64,
        source_key: GpuPaintSourceKey,
        source_size: [usize; 2],
        method: WgpuUpscaleMethod,
    ) -> Option<Arc<GpuIntermediateTexture>> {
        if let Some(intermediate) = self.intermediate_textures.get(&key).cloned() {
            return Some(intermediate);
        }
        let Some(source) = self.source_textures.peek(&source_key) else {
            return None;
        };
        let Some(output) =
            self.realtime_sr
                .render(method, key, device, encoder, &source.view, source_size)
        else {
            return None;
        };
        Some(self.insert_realtime_sr_stage_texture(device, key, method, source_size, output))
    }

    fn ensure_realtime_sr_stage_texture_from_view(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        key: u64,
        source_view: &wgpu::TextureView,
        source_size: [usize; 2],
        method: WgpuUpscaleMethod,
    ) -> Option<Arc<GpuIntermediateTexture>> {
        if let Some(intermediate) = self.intermediate_textures.get(&key).cloned() {
            return Some(intermediate);
        }
        let Some(output) =
            self.realtime_sr
                .render(method, key, device, encoder, source_view, source_size)
        else {
            return None;
        };
        Some(self.insert_realtime_sr_stage_texture(device, key, method, source_size, output))
    }

    fn insert_realtime_sr_stage_texture(
        &mut self,
        device: &wgpu::Device,
        key: u64,
        method: WgpuUpscaleMethod,
        source_size: [usize; 2],
        output: super::realtime_sr::RealtimeSrOutput,
    ) -> Arc<GpuIntermediateTexture> {
        let output_size = output.size;
        let output_byte_size = output.byte_size;
        let bind_group = Arc::new(self.texture_bind_group_for(device, &output.view));
        let mip_views = vec![output.texture.create_view(&mip_view_descriptor(0))];
        let intermediate = Arc::new(GpuIntermediateTexture {
            _texture: output.texture,
            _view: output.view,
            mip_views,
            bind_group,
            size: output_size,
            content_key: key,
            byte_size: output_byte_size,
        });
        let evicted_on_insert = if let Some((_old_key, old_texture)) =
            self.intermediate_textures.push(key, intermediate.clone())
        {
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
        intermediate
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

    /// Mirror the current byte counters into the read-only `*_LIVE` statics so the app/UI thread
    /// can display live GPU pool usage. Display-only; does not affect budget or eviction logic.
    fn publish_gpu_pool_bytes(&self) {
        GPU_SOURCE_TEXTURE_BYTES_LIVE.store(self.source_texture_bytes, Ordering::Relaxed);
        GPU_INTERMEDIATE_TEXTURE_BYTES_LIVE
            .store(self.intermediate_texture_bytes, Ordering::Relaxed);
        GPU_DRAW_STATE_BYTES_LIVE.store(self.draw_state_intermediate_bytes, Ordering::Relaxed);
    }

    fn prune_source_textures(&mut self) {
        while self.source_texture_bytes > self.source_texture_budget_bytes
            && self.source_textures.len() > 1
        {
            let Some((_key, texture)) = self.source_textures.pop_lru() else {
                break;
            };
            self.source_texture_bytes = self.source_texture_bytes.saturating_sub(texture.byte_size);
        }
        self.publish_gpu_pool_bytes();
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
        self.publish_gpu_pool_bytes();
    }

    fn prune_intermediate_textures(&mut self) {
        while self.intermediate_texture_bytes > self.intermediate_texture_budget_bytes
            && self.intermediate_textures.len() > 1
        {
            let Some((_key, texture)) = self.intermediate_textures.pop_lru() else {
                break;
            };
            self.intermediate_texture_bytes = self
                .intermediate_texture_bytes
                .saturating_sub(texture.byte_size);
        }
        self.publish_gpu_pool_bytes();
    }

    fn prune_draw_states(&mut self) {
        while self.draw_state_intermediate_bytes > self.intermediate_texture_budget_bytes
            && self.draw_bind_groups.len() > 1
        {
            let Some((_key, draw_state)) = self.draw_bind_groups.pop_lru() else {
                break;
            };
            self.draw_state_intermediate_bytes = self
                .draw_state_intermediate_bytes
                .saturating_sub(draw_state.intermediate_byte_size);
        }
        self.publish_gpu_pool_bytes();
    }
}

#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
fn record_gpu_texture_upload_stage(
    event: &'static str,
    started: Instant,
    width: usize,
    height: usize,
) {
    perf_trace::record_duration_if_at_least(
        event,
        started.elapsed(),
        Duration::from_millis(1),
        &[
            PerfField::Usize("width", width),
            PerfField::Usize("height", height),
        ],
    );
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

fn source_texture_content_key(
    source_key: GpuPaintSourceKey,
    source_size: [usize; 2],
    output_size: [usize; 2],
    effects: ViewEffects,
) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    "source_texture_content".hash(&mut hasher);
    source_key.hash(&mut hasher);
    source_size.hash(&mut hasher);
    output_size.hash(&mut hasher);
    effects.hash(&mut hasher);
    hasher.finish()
}

fn downscale_intermediate_texture_key(
    namespace: &'static str,
    content_key: u64,
    downscaler: WgpuDownscaleMethod,
    stage_size: [u32; 2],
    current_size: [usize; 2],
    stage_index: u32,
) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    namespace.hash(&mut hasher);
    content_key.hash(&mut hasher);
    downscaler.token().hash(&mut hasher);
    stage_size.hash(&mut hasher);
    current_size.hash(&mut hasher);
    stage_index.hash(&mut hasher);
    hasher.finish()
}

fn mipmap_intermediate_texture_key(content_key: u64) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    "hardware_mipmap_linear".hash(&mut hasher);
    content_key.hash(&mut hasher);
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

fn post_realtime_sr_downscale_method(
    output_size: [usize; 2],
    target_size: [u32; 2],
    requested_downscaler: WgpuDownscaleMethod,
) -> WgpuDownscaleMethod {
    requested_downscaler.resolve_for_downscale(output_size, target_size)
}

fn realtime_sr_stage_texture_key(
    source_key: GpuPaintSourceKey,
    base_source_size: [usize; 2],
    wgpu_upscale_method: WgpuUpscaleMethod,
    stage_index: usize,
    input_size: [usize; 2],
    stack_passes: usize,
) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    "realtime_sr_stage".hash(&mut hasher);
    source_key.hash(&mut hasher);
    base_source_size.hash(&mut hasher);
    wgpu_upscale_method.token().hash(&mut hasher);
    stage_index.hash(&mut hasher);
    input_size.hash(&mut hasher);
    stack_passes.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests;
