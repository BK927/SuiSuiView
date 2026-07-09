use super::super::{gpu_paint::GpuPaintSourceKey, KernelChoice, PageCacheKey};
use crate::core::deband::DebandStrength;
use crate::core::effects::ViewEffects;
use crate::core::gpu_effect::output_size_for_effects;
use crate::core::state::{
    CpuScaleFilter, PageTransitionStyle, ReadingDirection, WgpuDownscaleMethod, WgpuScaleDirection,
    WgpuScalePlan, WgpuUpscaleMethod,
};
use crate::core::worker::{DecodeBackend, PagePixels, PreparedPage, PreparedTargetIntent};
use egui::{TextureHandle, Vec2};
use std::time::Instant;

const SMART_WIDE_ASPECT: f32 = 1.20;
const SMART_TALL_ASPECT: f32 = 2.40;
const SMART_HEIGHT_MISMATCH: f32 = 0.25;
const SMART_ASPECT_MISMATCH: f32 = 0.30;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app) enum ViewMode {
    Single,
    DoubleLeftToRight,
    DoubleRightToLeft,
    SmartDoubleLeftToRight,
    SmartDoubleRightToLeft,
    VerticalStrip,
}

impl ViewMode {
    pub(in crate::app) fn step(self) -> usize {
        match self {
            Self::Single | Self::VerticalStrip => 1,
            Self::DoubleLeftToRight
            | Self::DoubleRightToLeft
            | Self::SmartDoubleLeftToRight
            | Self::SmartDoubleRightToLeft => 2,
        }
    }

    pub(in crate::app) fn is_smart(self) -> bool {
        matches!(
            self,
            Self::SmartDoubleLeftToRight | Self::SmartDoubleRightToLeft
        )
    }

    pub(in crate::app) fn reading_direction(self) -> Option<ReadingDirection> {
        match self {
            Self::Single | Self::VerticalStrip => None,
            Self::DoubleLeftToRight | Self::SmartDoubleLeftToRight => {
                Some(ReadingDirection::LeftToRight)
            }
            Self::DoubleRightToLeft | Self::SmartDoubleRightToLeft => {
                Some(ReadingDirection::RightToLeft)
            }
        }
    }

    pub(in crate::app) fn is_right_to_left(self, fallback: ReadingDirection) -> bool {
        self.reading_direction().unwrap_or(fallback) == ReadingDirection::RightToLeft
    }

    pub(in crate::app) fn with_reading_direction(self, direction: ReadingDirection) -> Self {
        match self {
            Self::Single => Self::Single,
            Self::VerticalStrip => Self::VerticalStrip,
            Self::DoubleLeftToRight | Self::DoubleRightToLeft => match direction {
                ReadingDirection::LeftToRight => Self::DoubleLeftToRight,
                ReadingDirection::RightToLeft => Self::DoubleRightToLeft,
            },
            Self::SmartDoubleLeftToRight | Self::SmartDoubleRightToLeft => match direction {
                ReadingDirection::LeftToRight => Self::SmartDoubleLeftToRight,
                ReadingDirection::RightToLeft => Self::SmartDoubleRightToLeft,
            },
        }
    }

    /// Stable, opaque token persisted in a book record so the view mode survives
    /// restarts. Core carries this as a plain string; only this layer knows the
    /// mapping. Keep in sync with [`ViewMode::from_token`].
    pub(in crate::app) fn token(self) -> &'static str {
        match self {
            Self::Single => "single",
            Self::DoubleLeftToRight => "double_ltr",
            Self::DoubleRightToLeft => "double_rtl",
            Self::SmartDoubleLeftToRight => "smart_ltr",
            Self::SmartDoubleRightToLeft => "smart_rtl",
            Self::VerticalStrip => "vertical_strip",
        }
    }

    /// Parse a persisted [`ViewMode::token`]. Unknown/legacy tokens yield `None`
    /// so the caller falls back to session behavior.
    pub(in crate::app) fn from_token(token: &str) -> Option<Self> {
        Some(match token {
            "single" => Self::Single,
            "double_ltr" => Self::DoubleLeftToRight,
            "double_rtl" => Self::DoubleRightToLeft,
            "smart_ltr" => Self::SmartDoubleLeftToRight,
            "smart_rtl" => Self::SmartDoubleRightToLeft,
            "vertical_strip" => Self::VerticalStrip,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(in crate::app) struct PageMetrics {
    pub(in crate::app) width: f32,
    pub(in crate::app) height: f32,
}

impl PageMetrics {
    pub(in crate::app) fn from_page(page: &PreparedPage) -> Self {
        Self {
            width: page.original_width.max(1) as f32,
            height: page.original_height.max(1) as f32,
        }
    }

    pub(in crate::app) fn aspect(self) -> f32 {
        self.width / self.height
    }

    pub(in crate::app) fn is_standalone(self) -> bool {
        self.aspect() >= SMART_WIDE_ASPECT || self.height / self.width >= SMART_TALL_ASPECT
    }

    pub(in crate::app) fn can_pair_with(self, other: Self) -> bool {
        if self.is_standalone() || other.is_standalone() {
            return false;
        }
        relative_difference(self.height, other.height) <= SMART_HEIGHT_MISMATCH
            && relative_difference(self.aspect(), other.aspect()) <= SMART_ASPECT_MISMATCH
    }
}

pub(in crate::app) fn ordered_spread_indices(
    mut indices: Vec<usize>,
    mode: ViewMode,
    fallback: ReadingDirection,
) -> Vec<usize> {
    if mode.is_right_to_left(fallback) {
        indices.reverse();
    }
    indices
}

pub(in crate::app) fn double_spread_indices(page: usize, page_count: usize) -> Vec<usize> {
    if page_count == 0 {
        return Vec::new();
    }
    let page = page.min(page_count - 1);
    let mut indices = vec![page];
    if let Some(next) = page.checked_add(1).filter(|next| *next < page_count) {
        indices.push(next);
    }
    indices
}

pub(in crate::app) fn smart_spread_indices_for_metrics(
    page: usize,
    page_count: usize,
    metrics_at: impl Fn(usize) -> Option<PageMetrics>,
) -> Vec<usize> {
    if page_count == 0 {
        return Vec::new();
    }
    let page = page.min(page_count - 1);
    let anchor = page - (page % 2);
    let Some(next) = anchor.checked_add(1).filter(|next| *next < page_count) else {
        return vec![page];
    };
    let Some(anchor_metrics) = metrics_at(anchor) else {
        return vec![page];
    };
    let Some(next_metrics) = metrics_at(next) else {
        return vec![page];
    };
    if anchor_metrics.can_pair_with(next_metrics) {
        vec![anchor, next]
    } else {
        vec![page]
    }
}

pub(in crate::app) fn relative_difference(left: f32, right: f32) -> f32 {
    let base = left.max(right).max(1.0);
    (left - right).abs() / base
}

pub(in crate::app) fn worker_center_page_for_mode(current_page: usize, mode: ViewMode) -> usize {
    if mode.is_smart() {
        current_page - (current_page % 2)
    } else {
        current_page
    }
}

pub(in crate::app) struct Transition {
    pub(in crate::app) from_indices: Vec<usize>,
    pub(in crate::app) target_long_edge: u32,
    pub(in crate::app) started_at: Instant,
    pub(in crate::app) screen_sign: f32,
    pub(in crate::app) style: PageTransitionStyle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app) enum PrepareScaleState {
    Native,
    CpuUpscale(CpuScaleFilter),
    CpuDownscale(CpuScaleFilter),
    FastSampledScaledDownscale(DecodeBackend),
}

impl PrepareScaleState {
    pub(in crate::app) fn label(self) -> String {
        match self {
            Self::Native => "no prepare resize".to_owned(),
            Self::CpuUpscale(filter) => format!("CPU prepare upscale ({})", filter.label()),
            Self::CpuDownscale(filter) => format!("CPU resize downscale ({})", filter.label()),
            Self::FastSampledScaledDownscale(backend) => {
                format!("sampled/scaled prepare ({})", backend.label())
            }
        }
    }

    fn from_page(key: PageCacheKey, page: &PreparedPage) -> Self {
        if page.display_width > page.original_width || page.display_height > page.original_height {
            Self::CpuUpscale(key.decode.cpu_upscale_filter)
        } else if page.display_width < page.original_width
            || page.display_height < page.original_height
        {
            if page.decode_backend.is_sampled_or_scaled_prepare() {
                Self::FastSampledScaledDownscale(page.decode_backend)
            } else {
                Self::CpuDownscale(key.decode.cpu_downscale_filter)
            }
        } else {
            Self::Native
        }
    }
}

/// Why the shown upscale method was chosen, for the toolbar scaler provenance suffix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app) enum UpscaleDecisionOrigin {
    /// The user picked a concrete (non-AUTO) method.
    User,
    /// AUTO routed to a method the book's round-trip probe decided.
    ProbeAuto,
    /// AUTO with no probe decision yet: the built-in default is in effect.
    AutoDefault,
}

// `substituted_below` holds an f32 threshold, so this state cannot derive `Eq`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(in crate::app) enum WgpuScaleState {
    Inactive,
    Native,
    Mixed,
    Upscale {
        method: WgpuUpscaleMethod,
        origin: UpscaleDecisionOrigin,
        /// `Some(threshold)` when the fixed-2x model was swapped for FSR because the
        /// needed scale fell below `threshold`; the shown method is then FSR.
        substituted_below: Option<f32>,
    },
    Downscale(WgpuDownscaleMethod),
}

impl WgpuScaleState {
    pub(in crate::app) fn label(self) -> String {
        match self {
            Self::Inactive => "no WGPU scaling".to_owned(),
            Self::Native => "WGPU native-size draw".to_owned(),
            Self::Mixed => "WGPU mixed-axis resize (bilinear)".to_owned(),
            Self::Upscale { method, .. } => format!("WGPU upscale ({})", method.label()),
            Self::Downscale(method) => format!("WGPU downscale ({})", method.label()),
        }
    }

    fn from_plan(
        active: bool,
        plan: WgpuScalePlan,
        origin: UpscaleDecisionOrigin,
        fixed_2x_sr_min_scale: f32,
    ) -> Self {
        if !active {
            return Self::Inactive;
        }
        match plan.direction {
            WgpuScaleDirection::Upscale => {
                if plan.effective_upscale_method == WgpuUpscaleMethod::None {
                    Self::Native
                } else {
                    Self::Upscale {
                        method: plan.effective_upscale_method,
                        origin,
                        substituted_below: plan
                            .upscale_substituted
                            .then_some(fixed_2x_sr_min_scale),
                    }
                }
            }
            WgpuScaleDirection::Downscale => Self::Downscale(plan.effective_downscale_method),
            WgpuScaleDirection::Mixed => Self::Mixed,
            WgpuScaleDirection::Native => Self::Native,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app) struct PageRenderInfo {
    pub(in crate::app) page_index: usize,
    pub(in crate::app) target_long_edge: u32,
    pub(in crate::app) decode_backend: DecodeBackend,
    pub(in crate::app) prepare_scale: PrepareScaleState,
}

impl PageRenderInfo {
    pub(in crate::app) fn from_page(
        page_index: usize,
        key: PageCacheKey,
        page: &PreparedPage,
    ) -> Self {
        Self {
            page_index,
            target_long_edge: key.target_long_edge,
            decode_backend: page.decode_backend,
            prepare_scale: PrepareScaleState::from_page(key, page),
        }
    }
}

// Holds a `WgpuScaleState`, whose `substituted_below: Option<f32>` blocks `Eq`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(in crate::app) struct CurrentViewState {
    pub(in crate::app) page_index: usize,
    pub(in crate::app) decode_backend: DecodeBackend,
    pub(in crate::app) prepare_scale: PrepareScaleState,
    pub(in crate::app) wgpu_scale: WgpuScaleState,
    /// The Glow kernel that drew this page at draw time, or `None` when the page
    /// was drawn by the plain sampler / a non-Glow backend. Set after paint so the
    /// top-bar chip can name the draw-time enlargement on native-prepared pages.
    pub(in crate::app) glow_kernel: Option<KernelChoice>,
    /// Debanding strength active for this page (WGPU display path only); `Off`
    /// for CPU/Glow pages and inspection views. Surfaced in the scaler tooltip.
    pub(in crate::app) deband: DebandStrength,
    pub(in crate::app) target_intent: PreparedTargetIntent,
}

impl CurrentViewState {
    pub(in crate::app) fn from_cpu(
        render: PageRenderInfo,
        target_intent: PreparedTargetIntent,
    ) -> Self {
        Self {
            page_index: render.page_index,
            decode_backend: render.decode_backend,
            prepare_scale: render.prepare_scale,
            wgpu_scale: WgpuScaleState::Inactive,
            glow_kernel: None,
            deband: DebandStrength::Off,
            target_intent,
        }
    }

    // established call surface; a params struct would be pure boilerplate
    #[allow(clippy::too_many_arguments)]
    pub(in crate::app) fn from_gpu(
        render: PageRenderInfo,
        image_size: [usize; 2],
        effects: ViewEffects,
        target_size: [u32; 2],
        wgpu_upscale_method: WgpuUpscaleMethod,
        wgpu_upscale_origin: UpscaleDecisionOrigin,
        wgpu_downscale_method: WgpuDownscaleMethod,
        fixed_2x_sr_min_scale: f32,
        active: bool,
        deband: DebandStrength,
        target_intent: PreparedTargetIntent,
    ) -> Self {
        let output_size = output_size_for_effects(image_size, effects);
        let scale_plan = WgpuScalePlan::resolve(
            output_size,
            target_size,
            wgpu_upscale_method,
            wgpu_downscale_method,
            fixed_2x_sr_min_scale,
        );
        Self {
            page_index: render.page_index,
            decode_backend: render.decode_backend,
            prepare_scale: render.prepare_scale,
            wgpu_scale: WgpuScaleState::from_plan(
                active,
                scale_plan,
                wgpu_upscale_origin,
                fixed_2x_sr_min_scale,
            ),
            glow_kernel: None,
            deband,
            target_intent,
        }
    }
}

pub(in crate::app) enum PageVisual {
    Ready {
        texture: TextureHandle,
        size: Vec2,
        render_info: Option<PageRenderInfo>,
    },
    ReadyGpu {
        source_key: GpuPaintSourceKey,
        image_size: [usize; 2],
        pixels: PagePixels,
        size: Vec2,
        effects: ViewEffects,
        wgpu_upscale_method: WgpuUpscaleMethod,
        wgpu_upscale_origin: UpscaleDecisionOrigin,
        wgpu_downscale_method: WgpuDownscaleMethod,
        render_info: PageRenderInfo,
    },
    Loading {
        index: usize,
    },
    Failed {
        index: usize,
        message: String,
    },
}

pub(in crate::app) fn page_visual_size(visual: &PageVisual) -> Vec2 {
    match visual {
        PageVisual::Ready { size, .. } => *size,
        PageVisual::ReadyGpu { size, .. } => *size,
        PageVisual::Loading { .. } | PageVisual::Failed { .. } => Vec2::new(900.0, 1300.0),
    }
}

#[cfg(test)]
mod tests {
    use super::ViewMode;
    use crate::core::state::ReadingDirection;

    #[test]
    fn vertical_strip_is_a_direction_free_single_step_mode() {
        assert_eq!(ViewMode::VerticalStrip.step(), 1);
        assert!(!ViewMode::VerticalStrip.is_smart());
        assert_eq!(ViewMode::VerticalStrip.reading_direction(), None);
        // Direction-free: applying either reading direction leaves the mode unchanged.
        assert_eq!(
            ViewMode::VerticalStrip.with_reading_direction(ReadingDirection::LeftToRight),
            ViewMode::VerticalStrip
        );
        assert_eq!(
            ViewMode::VerticalStrip.with_reading_direction(ReadingDirection::RightToLeft),
            ViewMode::VerticalStrip
        );
    }

    #[test]
    fn view_mode_tokens_round_trip_for_every_variant() {
        for mode in [
            ViewMode::Single,
            ViewMode::DoubleLeftToRight,
            ViewMode::DoubleRightToLeft,
            ViewMode::SmartDoubleLeftToRight,
            ViewMode::SmartDoubleRightToLeft,
            ViewMode::VerticalStrip,
        ] {
            assert_eq!(ViewMode::from_token(mode.token()), Some(mode));
        }
    }

    #[test]
    fn unknown_view_mode_token_is_none() {
        assert_eq!(ViewMode::from_token("webtoon"), None);
        assert_eq!(ViewMode::from_token(""), None);
    }
}
