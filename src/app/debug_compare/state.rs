use crate::app::{gpu_paint::GpuPaintSourceKey, SuiSuiViewApp};
use crate::core::deband::{CustomDeband, DebandStrength, ResolvedDeband};
use crate::core::effects::ViewEffects;
use crate::core::i18n::I18n;
use crate::core::state::{GpuDownscaleChoice, WgpuUpscaleMethod};
use std::hash::{Hash, Hasher};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app) enum DebugCompareTarget {
    Current,
    Model(WgpuUpscaleMethod),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(in crate::app) struct CompareSnapshot {
    pub current: WgpuUpscaleMethod,
    pub automatic: WgpuUpscaleMethod,
}

impl DebugCompareTarget {
    pub(in crate::app) fn label(self, snapshot: CompareSnapshot, i18n: I18n) -> String {
        match self {
            Self::Current => format!(
                "{} · {}",
                i18n.text("compare.current"),
                snapshot.current.label_i18n(i18n)
            ),
            Self::Model(WgpuUpscaleMethod::Auto) => {
                format!("Auto · {}", snapshot.automatic.label_i18n(i18n))
            }
            Self::Model(method) => method.label_i18n(i18n),
        }
    }

    pub(super) fn resolved(self, snapshot: CompareSnapshot) -> WgpuUpscaleMethod {
        match self {
            Self::Current => snapshot.current,
            Self::Model(WgpuUpscaleMethod::Auto) => snapshot.automatic,
            Self::Model(method) => method,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app) struct CompareSide {
    pub target: DebugCompareTarget,
    pub effects: ViewEffects,
    pub downscale: GpuDownscaleChoice,
    pub linear: bool,
    pub deband: DebandStrength,
    pub custom_deband: CustomDeband,
}

impl Default for CompareSide {
    fn default() -> Self {
        Self {
            target: DebugCompareTarget::Current,
            effects: ViewEffects::default(),
            downscale: GpuDownscaleChoice::default(),
            linear: false,
            deband: DebandStrength::Off,
            custom_deband: CustomDeband::default(),
        }
    }
}

impl CompareSide {
    pub(super) fn resolved_deband(self) -> ResolvedDeband {
        ResolvedDeband::resolve(self.deband, self.custom_deband)
    }

    pub(super) fn reference_id(
        self,
        source: GpuPaintSourceKey,
        method: WgpuUpscaleMethod,
        threshold: u32,
    ) -> u64 {
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        source.hash(&mut hash);
        self.effects.for_render().hash(&mut hash);
        self.downscale.hash(&mut hash);
        self.linear.hash(&mut hash);
        self.resolved_deband().hash(&mut hash);
        method.hash(&mut hash);
        threshold.hash(&mut hash);
        hash.finish()
    }
}

pub(in crate::app) struct DebugCompareState {
    pub enabled: bool,
    pub left: CompareSide,
    pub right: CompareSide,
    pub snapshot: CompareSnapshot,
    pub wipe: f32,
    pub side_by_side: bool,
    pub loupe: bool,
    pub loupe_zoom: f32,
    pub(super) loupe_page: Option<super::paint::LoupePage>,
}

impl Default for DebugCompareState {
    fn default() -> Self {
        Self {
            enabled: false,
            left: CompareSide::default(),
            right: CompareSide::default(),
            snapshot: CompareSnapshot::default(),
            wipe: 0.5,
            side_by_side: false,
            loupe: false,
            loupe_zoom: 4.0,
            loupe_page: None,
        }
    }
}

impl SuiSuiViewApp {
    pub(in crate::app) fn set_debug_compare_enabled(&mut self, enabled: bool) {
        if self.debug_compare.enabled == enabled {
            return;
        }
        self.debug_compare.enabled = enabled;
        self.debug_compare.loupe_page = None;
        self.transition = None;
        if enabled {
            let current = self
                .book_aware_wgpu_upscale_method(self.active_wgpu_upscale_method())
                .0;
            let automatic = self
                .book_aware_wgpu_upscale_method(WgpuUpscaleMethod::Auto)
                .0;
            self.debug_compare.snapshot = CompareSnapshot {
                current: current.resolve_for_upscale().unwrap_or(current),
                automatic: automatic.resolve_for_upscale().unwrap_or(automatic),
            };
            let side = CompareSide {
                target: DebugCompareTarget::Current,
                effects: self.effects,
                downscale: self.settings.expert_downscale.gpu,
                linear: self.settings.linear_light_downscale,
                deband: self.active_deband().strength(),
                custom_deband: self.settings.custom_deband,
            };
            self.debug_compare.left = side;
            self.debug_compare.right = side;
            for key in self.debug_compare_pin_keys() {
                self.request_debug_compare_page(key);
            }
        } else {
            self.debug_compare_inflight.clear();
        }
        self.egui_ctx.request_repaint();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comparison_auto_uses_the_captured_method() {
        let captured = CompareSnapshot {
            current: WgpuUpscaleMethod::Cunny4x24Ds,
            automatic: WgpuUpscaleMethod::WgslAnime4kV32CnnX2M,
        };
        assert_eq!(
            DebugCompareTarget::Current.resolved(captured),
            captured.current
        );
        assert_eq!(
            DebugCompareTarget::Model(WgpuUpscaleMethod::Auto).resolved(captured),
            captured.automatic
        );
        assert_eq!(
            DebugCompareTarget::Model(WgpuUpscaleMethod::Cunny4x24Ds).resolved(captured),
            WgpuUpscaleMethod::Cunny4x24Ds
        );
    }

    #[test]
    fn comparison_sides_are_independent_values() {
        let original = CompareSide::default();
        let mut edited = original;
        edited.linear = true;
        edited.effects.gamma = true;
        edited.custom_deband.radius += 1;
        assert!(!original.linear && !original.effects.gamma);
        assert_ne!(edited, original);
    }
}
