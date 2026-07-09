use super::realtime_sr::RealtimeSrResources;
use super::{PageCacheKey, SuiSuiViewApp};
use crate::core::deband::DebandStrength;
use crate::core::effects::ViewEffects;
use crate::core::gpu_effect::output_size_for_effects;
#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
use crate::core::perf_trace::{self, PerfField};
use crate::core::state::{
    FitMode, GpuEffectMode, RendererMode, WgpuDownscaleMethod, WgpuScaleDirection, WgpuScalePlan,
    WgpuUpscaleMethod,
};
use crate::core::worker::PagePixels;
use egui::{self, PaintCallbackInfo, Rect};
use egui_wgpu::{CallbackResources, CallbackTrait, ScreenDescriptor};
use lru::LruCache;
use pools::{GpuDrawState, GpuIntermediateTexture, GpuSourceTexture};
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
use std::time::Instant;

mod deband;
mod passes;
mod pools;
mod refine;

// Re-exported into the coordination module solely so the `tests` submodule can reach the
// pool/pass helpers through its `use super::*` glob after the split.
#[cfg(test)]
use passes::{
    defer_initial_realtime_sr_frame, needs_multi_pass_downscale, next_pyramid_stage_size,
    post_realtime_sr_downscale_method, realtime_sr_stage_texture_key,
};
#[cfg(test)]
use pools::{
    downscale_intermediate_texture_key, mip_level_count, mip_size, mipmap_intermediate_texture_key,
};

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
    pub(super) fixed_2x_sr_min_scale_pct: u32,
    pub(super) opacity: f32,
    /// Debanding strength resolved for this page (already gated off for
    /// Manual/Original inspection views and non-WGPU backends).
    pub(super) deband: DebandStrength,
    /// Set while an interactive zoom gesture is in motion. When the resolved plan
    /// is a downscale, this reroutes rendering through the cached hardware-mipmap
    /// path so a continuous zoom does not re-render the quality downscale at a new
    /// content key every frame.
    pub(super) zoom_in_motion: bool,
    /// The heavy idle-refine (정련) upscaler to try for this page, or `None` when
    /// refine is off / not applicable. Already gated app-side to a WGPU upscale
    /// path whose scale actually enlarges.
    pub(super) refine_method: Option<WgpuUpscaleMethod>,
    /// True only on an idle frame: the render side may pay the one-time refine
    /// render cost this frame. When false, an already-cached refine result is
    /// still used, but no new refine render is started.
    pub(super) allow_refine_render: bool,
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

    /// The debanding strength active for the current view, or `Off`. Gated on the
    /// same conditions as the display upscaler: WGPU backend live, GPU effects
    /// available, WGSL not opted out, and a fit mode that is not the
    /// Manual/Original source-inspection path (those must show true pixels).
    pub(super) fn active_deband(&self) -> DebandStrength {
        if !fit_mode_allows_display_upscale(self.fit_mode)
            || !self.gpu_effects_available
            || self.gpu_target_format.is_none()
            || matches!(self.settings.gpu_effect_mode, GpuEffectMode::CpuOnly)
            || !matches!(self.settings.renderer_mode, RendererMode::Wgpu)
        {
            return DebandStrength::Off;
        }
        self.settings.deband
    }

    /// The idle-refine (정련) upscaler to attempt for a page of `image_size`
    /// drawn to `target_size`, or `None` when refine is off / not applicable.
    /// Gated exactly like the display upscaler and deband (WGPU live, GPU effects
    /// available, WGSL not opted out, non-inspection fit), and additionally only
    /// when the page actually upscales through a realtime-SR method.
    pub(super) fn active_refine_method(
        &self,
        image_size: [usize; 2],
        target_size: [u32; 2],
        effects: ViewEffects,
        wgpu_downscale_method: WgpuDownscaleMethod,
        fixed_2x_sr_min_scale: f32,
    ) -> Option<WgpuUpscaleMethod> {
        let method = self.settings.refine_upscaler.method()?;
        if !fit_mode_allows_display_upscale(self.fit_mode)
            || !self.gpu_effects_available
            || self.gpu_target_format.is_none()
            || matches!(self.settings.gpu_effect_mode, GpuEffectMode::CpuOnly)
            || !matches!(self.settings.renderer_mode, RendererMode::Wgpu)
            || !RealtimeSrResources::is_supported(method)
        {
            return None;
        }
        // Only when the page upscales and the refine method is not substituted out
        // (a below-min-scale substitution would leave the realtime-SR family).
        let output_size = output_size_for_effects(image_size, effects);
        let plan = WgpuScalePlan::resolve(
            output_size,
            target_size,
            method,
            wgpu_downscale_method,
            fixed_2x_sr_min_scale,
        );
        (plan.direction == WgpuScaleDirection::Upscale && plan.effective_upscale_method == method)
            .then_some(method)
    }

    /// WGSL painting is governed by `gpu_effect_mode` (`CpuOnly` is the explicit
    /// opt-out) plus a usable `gpu_target_format`. The display downscaler is now a
    /// fixed pyramid Lanczos3 (`WGPU_DOWNSCALE_METHOD`), so the WGSL path is always
    /// the right target when GPU effects are available — no per-effect/upscale
    /// gating is needed here; `WgpuScalePlan` handles the native-passthrough case.
    pub(super) fn can_paint_wgsl_effects(&self) -> bool {
        self.gpu_effects_available
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
            fixed_2x_sr_min_scale_pct: request.fixed_2x_sr_min_scale_pct,
            opacity: request.opacity.clamp(0.0, 1.0),
            deband: request.deband,
            zoom_in_motion: request.zoom_in_motion,
            refine_method: request.refine_method,
            allow_refine_render: request.allow_refine_render,
            pool_budgets,
            rect: request.rect,
            target_format,
            draw_id: draw_id(
                request.source_key,
                request.effects,
                request.wgpu_upscale_method,
                request.wgpu_downscale_method,
                request.fixed_2x_sr_min_scale_pct,
                request.rect,
                request.opacity,
                request.deband,
                request.zoom_in_motion,
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
        if value.eq_ignore_ascii_case(WgpuUpscaleMethod::WgslSrLabSpanX2.token())
            && span_manifest_present
        {
            return Some(WgpuUpscaleMethod::WgslSrLabSpanX2);
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
    fixed_2x_sr_min_scale_pct: u32,
    opacity: f32,
    deband: DebandStrength,
    zoom_in_motion: bool,
    refine_method: Option<WgpuUpscaleMethod>,
    allow_refine_render: bool,
    pool_budgets: GpuPoolBudgets,
    rect: Rect,
    target_format: wgpu::TextureFormat,
    draw_id: u64,
    ctx: egui::Context,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct GpuDisplayRect {
    pub(super) origin: [u32; 2],
    pub(super) visible_size: [u32; 2],
    pub(super) sample_offset: [u32; 2],
    pub(super) full_size: [u32; 2],
}

impl GpuDisplayRect {
    pub(super) fn is_clipped(self) -> bool {
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
        resources.current_pass = self.ctx.cumulative_pass_nr();
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
            self.fixed_2x_sr_min_scale_pct as f32 / 100.0,
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
                self.fixed_2x_sr_min_scale_pct as f32 / 100.0,
                display_rect,
                self.opacity,
                self.zoom_in_motion,
                self.deband,
                self.refine_method,
                self.allow_refine_render,
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
    /// Deband pre-pass pipeline (Rgba8Unorm intermediate target), sharing the
    /// effect pipeline layout so the source bind group feeds it unchanged.
    deband_pipeline: wgpu::RenderPipeline,
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
    /// egui pass number of the pass currently being prepared; pool pruning
    /// never evicts entries stamped with it (multi-page strip frames need
    /// their whole working set alive through the paint callbacks).
    pub(super) current_pass: u64,
    deferred_realtime_sr_first_frames: LruCache<u64, ()>,
    realtime_sr: RealtimeSrResources,
    /// Last emitted `[refine]` log signature `(page_id, method, kind)`, so the
    /// diagnostic dedups a steady state to one line instead of per-frame spam.
    last_refine_log: Option<(u32, WgpuUpscaleMethod, u8)>,
}

#[allow(clippy::too_many_arguments)]
fn draw_id(
    source_key: GpuPaintSourceKey,
    effects: ViewEffects,
    wgpu_upscale_method: WgpuUpscaleMethod,
    wgpu_downscale_method: WgpuDownscaleMethod,
    fixed_2x_sr_min_scale_pct: u32,
    rect: Rect,
    opacity: f32,
    deband: DebandStrength,
    zoom_in_motion: bool,
) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    source_key.hash(&mut hasher);
    effects.hash(&mut hasher);
    wgpu_upscale_method.token().hash(&mut hasher);
    wgpu_downscale_method.token().hash(&mut hasher);
    fixed_2x_sr_min_scale_pct.hash(&mut hasher);
    rect.min.x.to_bits().hash(&mut hasher);
    rect.min.y.to_bits().hash(&mut hasher);
    rect.max.x.to_bits().hash(&mut hasher);
    rect.max.y.to_bits().hash(&mut hasher);
    opacity.to_bits().hash(&mut hasher);
    // Distinct draw states per strength so a strength change re-renders instead
    // of reusing the previous strength's cached draw.
    deband.token().hash(&mut hasher);
    // The bool changes which pipeline renders (cached mipmap sample vs. the
    // quality downscale), so distinct draw states must not collide.
    zoom_in_motion.hash(&mut hasher);
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

#[cfg(test)]
mod linear_downscale_tests;
#[cfg(test)]
mod tests;
