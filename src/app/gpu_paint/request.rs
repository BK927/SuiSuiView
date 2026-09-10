//! App-side quality policy and assembly of immutable GPU paint callbacks.
use super::{
    draw_id, GpuEffectCallback, GpuOriginalInspectionCleanupCallback, GpuPaintRequest,
    GpuPoolBudgets,
};
use crate::app::SuiSuiViewApp;
use crate::core::deband::ResolvedDeband;
use crate::core::state::{FitMode, GpuEffectMode, RendererMode, WgpuUpscaleMethod};
use egui::Rect;
use std::hash::{Hash, Hasher};
use std::sync::OnceLock;

const EXPERIMENT_WGPU_UPSCALE_METHOD_ENV: &str = "SUISUIVIEW_EXPERIMENT_WGPU_UPSCALE_METHOD";
const EXPERIMENT_SPAN_DISPLAY_ENV: &str = "SUISUIVIEW_SR_LAB_SPAN_DISPLAY";
const EXPERIMENT_SPAN_MANIFEST_ENV: &str = "SUISUIVIEW_EXPERIMENT_SPAN_MANIFEST";
const SR_LAB_SPAN_MANIFEST_ENV: &str = "SUISUIVIEW_SR_LAB_SPAN_MANIFEST";

impl SuiSuiViewApp {
    pub(in crate::app) fn gpu_paint_book_key(&self) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.book_id.hash(&mut hasher);
        self.opened_path.hash(&mut hasher);
        hasher.finish()
    }

    pub(in crate::app) fn active_wgpu_upscale_method(&self) -> WgpuUpscaleMethod {
        self.active_wgpu_upscale_method_for_fit(self.fit_mode)
    }

    pub(in crate::app) fn active_wgpu_upscale_method_for_fit(
        &self,
        fit_mode: FitMode,
    ) -> WgpuUpscaleMethod {
        if self.adjustments.uncorrected {
            return WgpuUpscaleMethod::None;
        }
        self.configured_wgpu_upscale_method_for_fit(fit_mode)
    }

    /// Display-only bypasses must not change the source preparation/cache policy.
    pub(in crate::app) fn configured_wgpu_upscale_method_for_fit(
        &self,
        fit_mode: FitMode,
    ) -> WgpuUpscaleMethod {
        if !fit_mode_allows_display_upscale(fit_mode)
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
    pub(in crate::app) fn active_deband(&self) -> ResolvedDeband {
        if self.adjustments.uncorrected
            || !fit_mode_allows_display_upscale(self.fit_mode)
            || !self.gpu_effects_available
            || self.gpu_target_format.is_none()
            || matches!(self.settings.gpu_effect_mode, GpuEffectMode::CpuOnly)
            || !matches!(self.settings.renderer_mode, RendererMode::Wgpu)
        {
            return ResolvedDeband::Off;
        }
        ResolvedDeband::resolve(self.settings.deband, self.settings.custom_deband)
    }

    /// WGSL painting is governed by `gpu_effect_mode` (`CpuOnly` is the explicit
    /// opt-out) plus a usable `gpu_target_format`. The selected downscaler is
    /// resolved by `WgpuScalePlan`, including its native-passthrough case, so no
    /// per-effect/upscale gating is needed here.
    pub(in crate::app) fn can_paint_wgsl_effects(&self) -> bool {
        self.gpu_effects_available
            && matches!(
                self.settings.gpu_effect_mode,
                GpuEffectMode::Auto | GpuEffectMode::Wgsl
            )
            && self.gpu_target_format.is_some()
    }

    pub(in crate::app) fn paint_wgsl_effects(
        &self,
        painter: &egui::Painter,
        request: GpuPaintRequest,
    ) -> bool {
        let Some(target_format) = self.gpu_target_format else {
            return false;
        };
        let pool_budgets = GpuPoolBudgets {
            source_texture_bytes: crate::app::gpu_source_texture_budget_bytes(&self.settings),
            intermediate_texture_bytes: crate::app::gpu_intermediate_texture_budget_bytes(
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
            linear_downscale: request.linear_downscale,
            background_refine: self.settings.background_refine
                && self.settings.wgpu_upscale_method != WgpuUpscaleMethod::Auto
                && crate::app::realtime_sr::background::BackgroundRefiner::supports(
                    request.wgpu_upscale_method,
                ),
            inspection: request.inspection,
            pool_budgets,
            rect: request.rect,
            target_format,
            draw_id: draw_id(
                request.source_key,
                request.effects,
                request.wgpu_upscale_method,
                request.wgpu_downscale_method,
                request.fixed_2x_sr_min_scale_pct,
                request.deband,
                request.zoom_in_motion,
                request.slot,
            ) ^ if request.linear_downscale {
                0x639a850d
            } else {
                0
            },
            ctx: painter.ctx().clone(),
        };
        painter.add(egui_wgpu::Callback::new_paint_callback(
            request.rect,
            callback,
        ));
        true
    }

    pub(in crate::app) fn paint_pending_gpu_original_inspection_cleanup(
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

pub(super) fn fit_mode_allows_display_upscale(fit_mode: FitMode) -> bool {
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

pub(super) fn parse_experimental_wgpu_upscale_method(
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

pub(super) fn wgpu_upscale_method_from_settings(
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
