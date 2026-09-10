//! On-demand observations of the branches that actually prepared a display.
//! No timers, allocations or publication run while the scale tooltip is closed.
use super::{GpuDisplayRect, GpuPaintSourceKey};
use crate::core::i18n::{I18n, ResolvedLanguage};
use crate::core::state::{
    WgpuDownscaleMethod, WgpuScaleDirection, WgpuScalePlan, WgpuUpscaleMethod,
};

const CAPACITY: usize = 8;
const REQUEST: &str = "display-scale-capture-request";
const SNAPSHOT: &str = "display-scale-capture-snapshot";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum DisplayScaleReason {
    Downscale,
    SrPostDownscale,
    SrPresentation,
    ZoomMipmap,
    NearNative,
    Native,
    MixedAxes,
    Upscale,
    BackgroundFallback,
    SrPendingFallback,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) struct ActualDisplayScale {
    pub downscale: Option<WgpuDownscaleMethod>,
    pub upscaler: WgpuUpscaleMethod,
    pub reason: DisplayScaleReason,
}

#[derive(Clone, Copy)]
pub(in crate::app) struct DisplayScaleEntry {
    pub slot: u64,
    pub source_key: GpuPaintSourceKey,
    pub actual: ActualDisplayScale,
    sr_upscaler: WgpuUpscaleMethod,
}

impl DisplayScaleEntry {
    pub(in crate::app) fn label(self, i18n: I18n) -> String {
        self.actual.label(i18n)
    }
}

impl ActualDisplayScale {
    pub(in crate::app) fn label(self, i18n: I18n) -> String {
        let ko = i18n.language() == ResolvedLanguage::KoKr;
        let reason = match (self.reason, ko) {
            (DisplayScaleReason::Downscale, true) => "화면 축소",
            (DisplayScaleReason::Downscale, false) => "display downscale",
            (DisplayScaleReason::SrPostDownscale, true) => "SR 처리 후 화면 크기로 재축소",
            (DisplayScaleReason::SrPostDownscale, false) => "downscale after SR",
            (DisplayScaleReason::SrPresentation, true) => "SR 결과 표시 · 추가 축소 없음",
            (DisplayScaleReason::SrPresentation, false) => "SR presentation; no further downscale",
            (DisplayScaleReason::ZoomMipmap, true) => "확대·축소 조작 중 임시 밉맵",
            (DisplayScaleReason::ZoomMipmap, false) => "temporary mipmap during zoom",
            (DisplayScaleReason::NearNative, true) => "원본에 가까운 크기 · 고급 축소 생략",
            (DisplayScaleReason::NearNative, false) => "near native; quality downscale bypassed",
            (DisplayScaleReason::Native, true) => "원본 크기 · 축소 없음",
            (DisplayScaleReason::Native, false) => "native size; no downscale",
            (DisplayScaleReason::MixedAxes, true) => "축마다 확대·축소가 달라 기본 보간 사용",
            (DisplayScaleReason::MixedAxes, false) => "mixed axis scaling; basic interpolation",
            (DisplayScaleReason::Upscale, true) => "화면 확대 · 축소 없음",
            (DisplayScaleReason::Upscale, false) => "display upscale; no downscale",
            (DisplayScaleReason::BackgroundFallback, true) => "배경 보정 대기 중 빠른 표시",
            (DisplayScaleReason::BackgroundFallback, false) => {
                "fast display while background refinement is pending"
            }
            (DisplayScaleReason::SrPendingFallback, true) => "SR 준비 중 기본 보간 표시",
            (DisplayScaleReason::SrPendingFallback, false) => {
                "basic interpolation while SR is pending"
            }
        };
        let mut path = Vec::with_capacity(2);
        if self.upscaler != WgpuUpscaleMethod::None {
            path.push(self.upscaler.label());
        }
        if let Some(downscale) = self.downscale {
            path.push(downscale.label());
        }
        if path.is_empty() {
            reason.to_owned()
        } else {
            format!("{} · {reason}", path.join(" → "))
        }
    }
}

#[derive(Clone)]
pub(in crate::app) struct DisplayScaleSnapshot {
    pub pass: u64,
    pub entries: Vec<DisplayScaleEntry>,
}

/// Call from the hover UI. A fresh request schedules one frame to collect it;
/// subsequent hover frames renew the lease without continuous repaint requests.
pub(in crate::app) fn request_capture(ctx: &egui::Context) {
    let pass = ctx.cumulative_pass_nr();
    let fresh = ctx.data_mut(|data| {
        let old = data.get_temp::<u64>(egui::Id::new(REQUEST));
        data.insert_temp(egui::Id::new(REQUEST), pass.saturating_add(1));
        old.is_none_or(|until| until < pass)
    });
    if fresh {
        ctx.request_repaint();
    }
}

pub(in crate::app) fn latest(ctx: &egui::Context) -> Option<DisplayScaleSnapshot> {
    ctx.data(|data| data.get_temp::<DisplayScaleSnapshot>(egui::Id::new(SNAPSHOT)))
        .filter(|snapshot| snapshot.pass.saturating_add(1) >= ctx.cumulative_pass_nr())
}

#[derive(Default)]
pub(super) struct DisplayScaleCapture {
    pass: Option<u64>,
    enabled: bool,
    entries: [Option<DisplayScaleEntry>; CAPACITY],
}

impl DisplayScaleCapture {
    pub(super) fn begin_draw(
        &mut self,
        ctx: &egui::Context,
        slot: u64,
        source_key: GpuPaintSourceKey,
    ) {
        let pass = ctx.cumulative_pass_nr();
        if self.pass != Some(pass) {
            self.pass = Some(pass);
            self.enabled = ctx
                .data(|data| data.get_temp::<u64>(egui::Id::new(REQUEST)))
                .is_some_and(|until| until >= pass);
            if self.enabled {
                self.entries.fill(None);
            }
        }
        if !self.enabled {
            return;
        }
        if let Some(index) = self
            .entries
            .iter()
            .position(|entry| entry.is_some_and(|e| e.slot == slot))
            .or_else(|| self.entries.iter().position(Option::is_none))
        {
            self.entries[index] = Some(DisplayScaleEntry {
                slot,
                source_key,
                actual: ActualDisplayScale {
                    downscale: None,
                    upscaler: WgpuUpscaleMethod::None,
                    reason: DisplayScaleReason::Native,
                },
                sr_upscaler: WgpuUpscaleMethod::None,
            });
        }
    }

    pub(super) fn resolve(
        &mut self,
        slot: u64,
        plan: WgpuScalePlan,
        size: [usize; 2],
        display: GpuDisplayRect,
    ) {
        let Some(entry) = self.entry(slot) else {
            return;
        };
        entry.sr_upscaler = plan.effective_upscale_method;
        entry.actual = resolved_scale(plan, size, display);
    }

    pub(super) fn fallback(&mut self, slot: u64, background: bool) {
        if let Some(entry) = self.entry(slot) {
            entry.actual = ActualDisplayScale {
                downscale: None,
                upscaler: if background {
                    WgpuUpscaleMethod::WgslFsr1EasuRcas
                } else {
                    WgpuUpscaleMethod::None
                },
                reason: if background {
                    DisplayScaleReason::BackgroundFallback
                } else {
                    DisplayScaleReason::SrPendingFallback
                },
            };
        }
    }

    pub(super) fn zoom_mipmap(&mut self, slot: u64) {
        if let Some(entry) = self.entry(slot) {
            entry.actual.downscale = Some(WgpuDownscaleMethod::HardwareMipmapLinear);
            entry.actual.reason = DisplayScaleReason::ZoomMipmap;
        }
    }

    pub(super) fn post_sr(
        &mut self,
        slot: u64,
        size: [usize; 2],
        display: GpuDisplayRect,
        method: WgpuDownscaleMethod,
    ) {
        if let Some(entry) = self.entry(slot) {
            let shrinks = display.full_size[0] as usize <= size[0]
                && display.full_size[1] as usize <= size[1]
                && display.full_size.map(|v| v as usize) != size;
            entry.actual = ActualDisplayScale {
                upscaler: entry.sr_upscaler,
                downscale: shrinks.then(|| actual_filter(method, size, display)),
                reason: if shrinks {
                    DisplayScaleReason::SrPostDownscale
                } else {
                    DisplayScaleReason::SrPresentation
                },
            };
        }
    }

    pub(super) fn publish(&self, ctx: &egui::Context) {
        if !self.enabled || self.pass != Some(ctx.cumulative_pass_nr()) {
            return;
        }
        let snapshot = DisplayScaleSnapshot {
            pass: self.pass.unwrap_or_default(),
            entries: self.entries.iter().flatten().copied().collect(),
        };
        ctx.data_mut(|data| data.insert_temp(egui::Id::new(SNAPSHOT), snapshot));
    }

    fn entry(&mut self, slot: u64) -> Option<&mut DisplayScaleEntry> {
        if !self.enabled {
            return None;
        }
        self.entries
            .iter_mut()
            .flatten()
            .find(|entry| entry.slot == slot)
    }
}

fn actual_filter(
    method: WgpuDownscaleMethod,
    size: [usize; 2],
    display: GpuDisplayRect,
) -> WgpuDownscaleMethod {
    if method.is_pyramid()
        && (display.is_clipped()
            || !super::passes::needs_multi_pass_downscale(size, display.full_size))
    {
        method.base_filter()
    } else {
        method
    }
}

fn resolved_scale(
    plan: WgpuScalePlan,
    size: [usize; 2],
    display: GpuDisplayRect,
) -> ActualDisplayScale {
    let reason = match plan.direction {
        WgpuScaleDirection::Downscale => DisplayScaleReason::Downscale,
        WgpuScaleDirection::Upscale => DisplayScaleReason::Upscale,
        WgpuScaleDirection::Mixed => DisplayScaleReason::MixedAxes,
        WgpuScaleDirection::Native if display.full_size.map(|v| v as usize) != size => {
            DisplayScaleReason::NearNative
        }
        WgpuScaleDirection::Native => DisplayScaleReason::Native,
    };
    ActualDisplayScale {
        upscaler: plan.effective_upscale_method,
        reason,
        downscale: (plan.direction == WgpuScaleDirection::Downscale)
            .then(|| actual_filter(plan.effective_downscale_method, size, display)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn actual_display_scale_distinguishes_native_clipped_and_multistage() {
        let display = |size| GpuDisplayRect {
            origin: [0, 0],
            visible_size: size,
            sample_offset: [0, 0],
            full_size: size,
        };
        let resolve = |size, rect: GpuDisplayRect| {
            resolved_scale(
                WgpuScalePlan::resolve(
                    size,
                    rect.full_size,
                    WgpuUpscaleMethod::WgslAnime4kV32CnnX2M,
                    WgpuDownscaleMethod::PyramidLanczos3,
                    1.1,
                ),
                size,
                rect,
            )
        };
        assert_eq!(
            resolve([1000, 1000], display([990, 990])).reason,
            DisplayScaleReason::NearNative
        );
        assert_eq!(
            resolve([1000, 1000], display([1000, 1000])).reason,
            DisplayScaleReason::Native
        );
        assert_eq!(
            resolve([1000, 1000], display([250, 250])).downscale,
            Some(WgpuDownscaleMethod::PyramidLanczos3)
        );
        let mut clipped = display([250, 250]);
        clipped.visible_size = [100, 100];
        assert_eq!(
            resolve([1000, 1000], clipped).downscale,
            Some(WgpuDownscaleMethod::Lanczos3)
        );
        assert_eq!(
            resolve([1000, 1000], display([700, 700])).downscale,
            Some(WgpuDownscaleMethod::Lanczos3)
        );
    }

    #[test]
    fn actual_display_scale_replaces_fallback_with_ready_sr_and_bounds_slots() {
        use crate::app::PageCacheKey;
        use crate::core::source::PageId;
        use crate::core::worker::DecodeOptions;
        let ctx = egui::Context::default();
        let key = GpuPaintSourceKey {
            book: 1,
            page: PageCacheKey {
                page_id: PageId(1),
                target_long_edge: 64,
                decode: DecodeOptions::default(),
            },
        };
        let display = GpuDisplayRect {
            origin: [0, 0],
            visible_size: [48, 48],
            sample_offset: [0, 0],
            full_size: [48, 48],
        };
        let mut capture = DisplayScaleCapture::default();
        capture.begin_draw(&ctx, 1, key);
        assert!(!capture.enabled);
        request_capture(&ctx);
        // A real callback starts collecting once in the next prepare pass.
        capture.pass = None;
        capture.begin_draw(&ctx, 1, key);
        let model = WgpuUpscaleMethod::WgslAnime4kV32CnnX2M;
        capture.resolve(
            1,
            WgpuScalePlan::resolve(
                [32, 32],
                [48, 48],
                model,
                WgpuDownscaleMethod::PyramidLanczos3,
                1.1,
            ),
            [32, 32],
            display,
        );
        capture.fallback(1, true);
        assert_eq!(
            capture.entry(1).unwrap().actual.upscaler,
            WgpuUpscaleMethod::WgslFsr1EasuRcas
        );
        capture.post_sr(1, [64, 64], display, WgpuDownscaleMethod::PyramidLanczos3);
        let actual = capture.entry(1).unwrap().actual;
        assert_eq!(actual.upscaler, model);
        assert_eq!(actual.reason, DisplayScaleReason::SrPostDownscale);
        assert_eq!(actual.downscale, Some(WgpuDownscaleMethod::Lanczos3));
        capture.fallback(1, false);
        assert_eq!(
            capture.entry(1).unwrap().actual.upscaler,
            WgpuUpscaleMethod::None
        );
        assert_eq!(
            capture.entry(1).unwrap().actual.reason,
            DisplayScaleReason::SrPendingFallback
        );
        for slot in 2..20 {
            capture.begin_draw(&ctx, slot, key);
        }
        assert_eq!(capture.entries.iter().flatten().count(), CAPACITY);
    }
}
