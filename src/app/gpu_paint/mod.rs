use super::realtime_sr::RealtimeSrResources;
use super::PageCacheKey;
use crate::core::deband::ResolvedDeband;
use crate::core::effects::ViewEffects;
use crate::core::gpu_effect::output_size_for_effects;
#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
use crate::core::perf_trace::{self, PerfField};
#[cfg(test)]
use crate::core::state::FitMode;
#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
use crate::core::state::WgpuScalePlan;
use crate::core::state::{WgpuDownscaleMethod, WgpuUpscaleMethod};
use crate::core::worker::PagePixels;
use egui::{self, PaintCallbackInfo, Rect};
use egui_wgpu::{CallbackResources, CallbackTrait, ScreenDescriptor};
use lru::LruCache;
use pools::{GpuDrawState, GpuIntermediateTexture, GpuSourceTexture};
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
use std::time::Instant;

mod accounting;
mod deband;
mod initialization;
mod inspection;
mod passes;
mod pools;
mod request;
pub(super) use inspection::GpuInspection;
pub(in crate::app) mod display_status;
pub(in crate::app) mod refine;

#[cfg(test)]
use request::{
    fit_mode_allows_display_upscale, parse_experimental_wgpu_upscale_method,
    wgpu_upscale_method_from_settings,
};

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

// Read-only mirrors of the render-thread-owned `GpuPaintResources` byte counters, so the
// app/UI thread can display live GPU pool usage. These are purely for visibility: budget and
// eviction logic still runs off the owning fields. Each mutating method republishes the field
// values via `publish_gpu_pool_bytes`, and `GpuPaintResources::new` resets them to 0.
static GPU_SOURCE_TEXTURE_BYTES_LIVE: AtomicUsize = AtomicUsize::new(0);
static GPU_INTERMEDIATE_TEXTURE_BYTES_LIVE: AtomicUsize = AtomicUsize::new(0);
static GPU_DRAW_STATE_BYTES_LIVE: AtomicUsize = AtomicUsize::new(0);

/// Cached source payload plus unique intermediate allocations still owned by
/// any pool or draw state. Pool ownership counters below intentionally overlap.
pub(crate) fn gpu_cached_texture_bytes_live() -> usize {
    GPU_SOURCE_TEXTURE_BYTES_LIVE.load(Ordering::Relaxed) + accounting::intermediate_bytes()
}

/// Live GPU pool usage in bytes as `(source_textures, intermediate_textures, draw_states)`.
/// Reflects the most recent state published by the render thread; returns zeros before any
/// GPU paint resources exist.
#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
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
    /// Stable draw-slot discriminator for this request, so two draws that share
    /// a source_key but coexist in one frame (debug-compare panes, the two legs
    /// of a page-turn transition) get distinct `draw_id`s and never recycle the
    /// same params buffer against each other. 0 = the main viewer draw.
    pub(super) slot: u8,
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
    pub(super) deband: ResolvedDeband,
    /// Set while an interactive zoom gesture is in motion. When the resolved plan
    /// is a downscale, this reroutes rendering through the cached hardware-mipmap
    /// path so a continuous zoom does not re-render the quality downscale at a new
    /// content key every frame.
    pub(super) zoom_in_motion: bool,
    pub(super) linear_downscale: bool,
    pub(super) inspection: Option<GpuInspection>,
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
    deband: ResolvedDeband,
    zoom_in_motion: bool,
    linear_downscale: bool,
    background_refine: bool,
    inspection: Option<GpuInspection>,
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
            resources.set_target_format(device, self.target_format);
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
        resources
            .realtime_sr
            .begin_active_frame(resources.current_pass);
        let source_uploaded = resources.ensure_source_texture(
            device,
            queue,
            self.source_key,
            self.image_size,
            &self.pixels,
        );
        #[cfg(not(any(feature = "perf-dev", feature = "perf-diagnostics")))]
        let _ = source_uploaded;

        resources.request_linear_downscale = self.linear_downscale;
        resources.request_background_refine = self.background_refine;
        let output_size = output_size_for_effects(self.image_size, self.effects);
        let destination_rect = viewport_rect(self.rect, screen_descriptor);
        let display_rect = self
            .inspection
            .map_or(destination_rect, |inspection| GpuDisplayRect {
                origin: [0, 0],
                visible_size: inspection.reference_size,
                sample_offset: [0, 0],
                full_size: inspection.reference_size,
            });
        let source_slot = if self.inspection.is_some() {
            self.draw_id ^ 0xa53e_2389_a148_f7d0
        } else {
            self.draw_id
        };
        resources
            .display_scale_capture
            .begin_draw(&self.ctx, source_slot, self.source_key);
        resources.request_inspection = self
            .inspection
            .map(|inspection| (inspection, destination_rect, self.draw_id));
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
                queue,
                source_slot,
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
                &self.ctx,
            );
            let draw_state = if let Some(inspection) = self.inspection {
                resources.prepare_inspection_draw(
                    device,
                    queue,
                    egui_encoder,
                    self.draw_id,
                    draw_state,
                    inspection,
                    destination_rect,
                )
            } else {
                draw_state
            };
            resources.insert_draw_state(self.draw_id, draw_state);
        }
        resources.display_scale_capture.publish(&self.ctx);
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

    fn finish_prepare(
        &self,
        _device: &wgpu::Device,
        _queue: &wgpu::Queue,
        _encoder: &mut wgpu::CommandEncoder,
        callback_resources: &mut CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        if let Some(resources) = callback_resources.get_mut::<GpuPaintResources>() {
            resources.finish_normal_sr_frame(&self.ctx);
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
        render_pass.set_bind_group(1, draw_state.params_bind_group.as_ref(), &[]);
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
    /// Deband pre-pass pipeline (fp16 intermediate target since V12), sharing the
    /// effect pipeline layout so the source bind group feeds it unchanged.
    deband_pipeline: Option<wgpu::RenderPipeline>,
    request_linear_downscale: bool,
    request_background_refine: bool,
    request_inspection: Option<(GpuInspection, GpuDisplayRect, u64)>,
    inspection_pipeline: Option<wgpu::RenderPipeline>,
    display_pipeline_backup: Option<(wgpu::TextureFormat, wgpu::RenderPipeline)>,
    display_scale_capture: display_status::DisplayScaleCapture,
    refine: refine::RefineCoordinator,
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
    /// Test-only tally of params uniform buffers freshly allocated by
    /// `create_params_pair` (a slot miss). The V15 recycle test asserts a second
    /// prepare at the same slot does NOT bump this. Absent from product builds.
    #[cfg(test)]
    params_buffer_creations: AtomicUsize,
}

#[allow(clippy::too_many_arguments)]
fn draw_id(
    source_key: GpuPaintSourceKey,
    effects: ViewEffects,
    wgpu_upscale_method: WgpuUpscaleMethod,
    wgpu_downscale_method: WgpuDownscaleMethod,
    fixed_2x_sr_min_scale_pct: u32,
    deband: ResolvedDeband,
    zoom_in_motion: bool,
    slot: u8,
) -> u64 {
    // A STABLE draw-slot identity: deliberately independent of the display rect
    // and opacity, so scrolling (which changes both every frame) reuses the one
    // draw state — and its persistent params buffer — for a page instead of
    // minting a new id + buffer per frame (V15). Rect/opacity now live only in
    // the params uniform, rewritten in place via `recycle_params_pair`.
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    source_key.hash(&mut hasher);
    effects.hash(&mut hasher);
    wgpu_upscale_method.token().hash(&mut hasher);
    wgpu_downscale_method.token().hash(&mut hasher);
    fixed_2x_sr_min_scale_pct.hash(&mut hasher);
    // Distinct draw states per strength so a strength change re-renders instead
    // of reusing the previous strength's cached draw.
    deband.hash(&mut hasher);
    // The bool changes which pipeline renders (cached mipmap sample vs. the
    // quality downscale), so distinct draw states must not collide.
    zoom_in_motion.hash(&mut hasher);
    // Separates draws that share every field above but coexist in one frame
    // (debug-compare panes, the two legs of a page-turn transition), so their
    // per-slot params buffers never write over each other.
    slot.hash(&mut hasher);
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
mod ownership_tests;
#[cfg(test)]
mod tests;
