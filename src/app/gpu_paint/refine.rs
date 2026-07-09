//! V11 정련 (idle-scheduled refine) routing for the WGSL draw path.
//!
//! The normal (fast) upscaler chain draws every frame as before. When a refine
//! upscaler is configured and the page upscales, [`GpuPaintResources`] routes the
//! realtime-SR draw through the heavier refine method — reusing an already-cached
//! refine result for free, or paying the one-time refine render only on an idle
//! frame. All of it leans on the existing realtime-SR stage cache, so nothing
//! downstream (post-SR downscale, pins, keys) changes.

use super::passes::realtime_sr_stage_texture_key;
use super::{GpuDisplayRect, GpuPaintResources, GpuPaintSourceKey};
use crate::app::realtime_sr::RealtimeSrResources;
use crate::core::deband::DebandStrength;
use crate::core::state::{
    WgpuDownscaleMethod, WgpuScaleDirection, WgpuScalePlan, WgpuUpscaleMethod,
};
use std::sync::OnceLock;

// `[refine]` diagnostic kinds (see `log_refine`).
pub(super) const REFINE_RENDER: u8 = 0;
pub(super) const REFINE_HIT: u8 = 1;
pub(super) const REFINE_SKIP: u8 = 2;

/// Whether `SUISUIVIEW_REFINE_LOG` opts into the `[refine]` diagnostic lines.
/// Resolved once; entirely free when unset.
fn refine_log_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("SUISUIVIEW_REFINE_LOG").is_ok_and(|value| {
            let value = value.trim();
            !value.is_empty() && value != "0"
        })
    })
}

impl GpuPaintResources {
    /// Decide which upscaler the realtime-SR path should use this frame: the
    /// configured refine method when the page upscales and either a cached refine
    /// result already exists or an idle frame permits rendering it now; otherwise
    /// the normal effective upscaler (unchanged fast path).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn resolve_refine_route(
        &mut self,
        refine_method: Option<WgpuUpscaleMethod>,
        allow_refine_render: bool,
        scale_plan: WgpuScalePlan,
        source_key: GpuPaintSourceKey,
        source_size: [usize; 2],
        output_size: [usize; 2],
        downscaler: WgpuDownscaleMethod,
        fixed_2x_sr_min_scale: f32,
        display_rect: GpuDisplayRect,
        deband: DebandStrength,
        effective_upscaler: WgpuUpscaleMethod,
    ) -> WgpuUpscaleMethod {
        let Some(refine_method) = refine_method else {
            return effective_upscaler;
        };
        if scale_plan.direction != WgpuScaleDirection::Upscale {
            return effective_upscaler;
        }
        // Resolve the refine method exactly like a normal upscaler: a small-scale
        // substitution would drop it out of the realtime-SR family, in which case
        // there is nothing to refine and the normal path stands.
        let refine_upscaler = WgpuScalePlan::resolve(
            output_size,
            display_rect.full_size,
            refine_method,
            downscaler,
            fixed_2x_sr_min_scale,
        )
        .effective_upscale_method;
        if !RealtimeSrResources::is_supported(refine_upscaler) {
            return effective_upscaler;
        }
        // Peek (never `get`) so the readiness probe does not perturb LRU order.
        let ready = self.refine_final_stage_ready(
            source_key,
            source_size,
            output_size,
            refine_upscaler,
            display_rect,
            deband,
        );
        if ready {
            self.log_refine(
                source_key,
                refine_upscaler,
                display_rect.full_size,
                REFINE_HIT,
            );
            refine_upscaler
        } else if allow_refine_render {
            self.log_refine(
                source_key,
                refine_upscaler,
                display_rect.full_size,
                REFINE_RENDER,
            );
            refine_upscaler
        } else {
            self.log_refine(
                source_key,
                refine_upscaler,
                display_rect.full_size,
                REFINE_SKIP,
            );
            effective_upscaler
        }
    }

    /// Whether the refine chain's final stage texture for this page already lives
    /// in the intermediate cache. Uses `peek` so it leaves LRU order untouched.
    fn refine_final_stage_ready(
        &self,
        source_key: GpuPaintSourceKey,
        source_size: [usize; 2],
        output_size: [usize; 2],
        refine_method: WgpuUpscaleMethod,
        display_rect: GpuDisplayRect,
        deband: DebandStrength,
    ) -> bool {
        let stack_passes = refine_method.fixed_2x_stack_passes(output_size, display_rect.full_size);
        if stack_passes == 0 {
            return false;
        }
        let final_index = stack_passes - 1;
        // Each realtime-SR stage doubles the size, so the final stage's input is
        // the source scaled by 2^final_index (matching the chain in
        // `prepare_realtime_sr_draw_state`).
        let scale = 1usize << final_index;
        let input_size = [
            source_size[0].saturating_mul(scale),
            source_size[1].saturating_mul(scale),
        ];
        let final_key = realtime_sr_stage_texture_key(
            source_key,
            source_size,
            refine_method,
            final_index,
            input_size,
            stack_passes,
            deband,
        );
        // Presence alone is not readiness: `ensure_intermediate_texture` inserts
        // before the render is recorded, and a deferred-warmup frame can leave an
        // entry unrendered. Route through the refine chain only when the final
        // stage actually holds pixels.
        self.intermediate_textures
            .peek(&final_key)
            .is_some_and(|texture| texture.rendered.load(std::sync::atomic::Ordering::Relaxed))
    }

    /// Emit an `[refine]` diagnostic line, deduped against the last emitted
    /// `(page, method, kind)` so a steady state logs once instead of per frame.
    /// Cheap (and silent) unless `SUISUIVIEW_REFINE_LOG` is set.
    fn log_refine(
        &mut self,
        source_key: GpuPaintSourceKey,
        method: WgpuUpscaleMethod,
        target: [u32; 2],
        kind: u8,
    ) {
        if !refine_log_enabled() {
            return;
        }
        let page_id = source_key.page.page_id.0;
        let entry = (page_id, method, kind);
        if self.last_refine_log == Some(entry) {
            return;
        }
        self.last_refine_log = Some(entry);
        let verb = match kind {
            REFINE_RENDER => "render",
            REFINE_HIT => "hit",
            _ => "skip",
        };
        eprintln!(
            "[refine] {verb} page_id={page_id} method={} size={}x{}",
            method.token(),
            target[0],
            target[1],
        );
    }
}
