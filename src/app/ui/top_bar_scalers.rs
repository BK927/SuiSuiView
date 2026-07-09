use super::super::viewer::{
    CurrentViewState, PrepareScaleState, UpscaleDecisionOrigin, WgpuScaleState,
};
use super::super::{KernelChoice, SuiSuiViewApp};
use super::{icons, theme};
use crate::core::i18n::I18n;
use crate::core::state::{AppSettings, CpuScaleFilter, RendererMode, WgpuUpscaleMethod};
use egui::{self, RichText};

impl SuiSuiViewApp {
    pub(in crate::app::ui) fn show_scale_group(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        let i18n = self.i18n();
        let current_view = self.current_view_state;
        let summary = top_bar_scaler_summary(current_view.as_ref(), i18n);
        ui.menu_button(icons::icon_text(icons::RESIZE_SMALL, &summary), |ui| {
            self.hold_top_bar_open_for_menu();
            ui.set_min_width(320.0);
            self.show_cpu_filter_row(
                ctx,
                ui,
                i18n.text("topbar.scale.cpu_up"),
                self.settings.cpu_upscale_filter,
                |settings, filter| settings.cpu_upscale_filter = filter,
            );

            ui.separator();
            let gpu_enabled = matches!(self.settings.renderer_mode, RendererMode::Wgpu);
            if !gpu_enabled {
                ui.label(
                    RichText::new(i18n.text("topbar.scale.wgpu_disabled"))
                        .size(12.0)
                        .color(theme::TEXT_MUTED),
                );
                ui.add_space(4.0);
            }
            ui.add_enabled_ui(gpu_enabled, |ui| {
                self.show_wgpu_upscale_row(ctx, ui, i18n);
            })
            .response
            .on_disabled_hover_text(i18n.text("topbar.scale.wgpu_disabled"));
        })
        .response
        .on_hover_text(top_bar_scaler_tooltip(current_view.as_ref(), i18n));
    }

    fn show_cpu_filter_row(
        &mut self,
        ctx: &egui::Context,
        ui: &mut egui::Ui,
        title: String,
        current: CpuScaleFilter,
        apply: impl Fn(&mut AppSettings, CpuScaleFilter),
    ) {
        ui.label(format!("{title}: {}", current.label()));
        let candidates = cpu_filter_candidates(&self.settings.top_bar_cpu_scale_filters, current);
        ui.horizontal_wrapped(|ui| {
            for candidate in candidates {
                if ui
                    .selectable_label(current == candidate, candidate.label())
                    .clicked()
                {
                    let mut settings = self.settings.clone();
                    apply(&mut settings, candidate);
                    self.apply_settings(ctx, settings);
                }
            }
        });
    }

    fn show_wgpu_upscale_row(&mut self, ctx: &egui::Context, ui: &mut egui::Ui, i18n: I18n) {
        let current = wgpu_upscale_menu_current(&self.settings);
        ui.label(format!(
            "{}: {}",
            i18n.text("topbar.scale.wgpu_up"),
            current.settings_label_i18n(i18n)
        ));
        let candidates =
            wgpu_upscale_candidates(&self.settings.top_bar_wgpu_upscale_methods, current);
        ui.horizontal_wrapped(|ui| {
            for candidate in candidates {
                if ui
                    .selectable_label(current == candidate, candidate.settings_label_i18n(i18n))
                    .clicked()
                {
                    let mut settings = self.settings.clone();
                    settings.wgpu_upscale_method = candidate;
                    self.apply_settings(ctx, settings);
                }
            }
        });
    }
}

fn top_bar_scaler_summary(current_view: Option<&CurrentViewState>, i18n: I18n) -> String {
    let Some(current_view) = current_view else {
        return i18n.text("topbar.scale");
    };
    let mut parts = Vec::new();
    if let Some(prepare_label) = compact_prepare_scale_state_label(current_view.prepare_scale) {
        parts.push(prepare_label);
    }
    // A native-prepared page that the Glow kernel enlarged at draw time reads as
    // the filter's technical name (not the confusing "Native" prepare label).
    if let Some(kernel_label) = compact_glow_kernel_label(current_view) {
        parts.push(kernel_label);
    }
    if let Some(wgpu_label) = compact_wgpu_scale_state_label(current_view.wgpu_scale, i18n) {
        parts.push(wgpu_label);
    }
    if parts.is_empty() {
        return "Native".to_owned();
    }
    parts.join(" | ")
}

fn top_bar_scaler_tooltip(current_view: Option<&CurrentViewState>, i18n: I18n) -> String {
    let Some(current_view) = current_view else {
        return i18n.text("topbar.scale.current_unknown");
    };
    let mut lines = vec![
        format!(
            "{}: {}",
            i18n.text("topbar.scale.current_prepare"),
            current_view.prepare_scale.label()
        ),
        format!(
            "{}: {}",
            i18n.text("topbar.scale.current_display"),
            current_view.wgpu_scale.label()
        ),
    ];
    if let Some(kernel) = glow_kernel_for_chip(current_view) {
        lines.push(i18n.with_vars(
            "topbar.scale.glow_kernel",
            &[("kernel", kernel.label().to_owned())],
        ));
    }
    if current_view.deband.is_active() {
        lines.push(i18n.with_vars(
            "topbar.scale.deband",
            &[("level", current_view.deband.label_i18n(i18n))],
        ));
    }
    if let Some(provenance) = wgpu_scale_provenance_sentence(current_view.wgpu_scale, i18n) {
        lines.push(provenance);
    }
    lines.join("\n")
}

/// The Glow draw-time kernel to surface on the chip: only when it drew this page
/// and the prepared texture was native (so the chip does not shadow a real CPU
/// prepare-resize label).
fn glow_kernel_for_chip(current_view: &CurrentViewState) -> Option<KernelChoice> {
    (current_view.prepare_scale == PrepareScaleState::Native)
        .then_some(current_view.glow_kernel)
        .flatten()
}

fn compact_glow_kernel_label(current_view: &CurrentViewState) -> Option<String> {
    glow_kernel_for_chip(current_view).map(|kernel| kernel.label().to_owned())
}

fn cpu_filter_candidates(
    configured: &[CpuScaleFilter],
    current: CpuScaleFilter,
) -> Vec<CpuScaleFilter> {
    unique_candidates(
        configured
            .iter()
            .copied()
            .filter(|filter| CpuScaleFilter::ALL.contains(filter)),
        current,
    )
}

fn wgpu_upscale_candidates(
    configured: &[WgpuUpscaleMethod],
    current: WgpuUpscaleMethod,
) -> Vec<WgpuUpscaleMethod> {
    unique_candidates(
        configured.iter().copied().filter(|method| {
            *method != WgpuUpscaleMethod::None
                && WgpuUpscaleMethod::SETTINGS_CHOICES.contains(method)
        }),
        current,
    )
}

fn unique_candidates<T: Copy + Eq>(configured: impl IntoIterator<Item = T>, current: T) -> Vec<T> {
    let mut candidates = Vec::new();
    for candidate in configured {
        if !candidates.contains(&candidate) {
            candidates.push(candidate);
        }
    }
    if !candidates.contains(&current) {
        candidates.push(current);
    }
    candidates
}

fn wgpu_upscale_menu_current(settings: &AppSettings) -> WgpuUpscaleMethod {
    if matches!(settings.renderer_mode, RendererMode::Wgpu)
        && settings.wgpu_upscale_method == WgpuUpscaleMethod::None
    {
        WgpuUpscaleMethod::Auto
    } else {
        settings.wgpu_upscale_method
    }
}

fn compact_prepare_scale_state_label(state: PrepareScaleState) -> Option<String> {
    match state {
        PrepareScaleState::Native => None,
        PrepareScaleState::CpuUpscale(filter) | PrepareScaleState::CpuDownscale(filter) => {
            Some(scale_filter_label(filter))
        }
        PrepareScaleState::FastSampledScaledDownscale(backend) => Some(backend.label().to_owned()),
    }
}

fn compact_wgpu_scale_state_label(state: WgpuScaleState, i18n: I18n) -> Option<String> {
    match state {
        WgpuScaleState::Inactive | WgpuScaleState::Native => None,
        WgpuScaleState::Mixed => Some("Bilinear".to_owned()),
        WgpuScaleState::Upscale {
            method,
            origin,
            substituted_below,
        } => Some(compact_wgpu_upscale_provenance_label(
            method,
            origin,
            substituted_below,
        )),
        // The display downscaler is a fixed internal constant (C2), not a user
        // choice: the chip says WHAT is happening in plain words; the tooltip
        // still names the algorithm for the curious.
        WgpuScaleState::Downscale(_method) => Some(i18n.text("topbar.scale.downscale")),
    }
}

/// The compact upscale method name plus a provenance suffix. Substitution wins over the
/// origin suffix because it is the more specific story (and implies the shown method is FSR).
fn compact_wgpu_upscale_provenance_label(
    method: WgpuUpscaleMethod,
    origin: UpscaleDecisionOrigin,
    substituted_below: Option<f32>,
) -> String {
    let base = compact_wgpu_upscale_label(method);
    match substituted_below {
        Some(threshold) => format!("{base}·<{threshold:.2}x"),
        None => match origin {
            UpscaleDecisionOrigin::User => base,
            UpscaleDecisionOrigin::ProbeAuto => format!("{base}·probe"),
            UpscaleDecisionOrigin::AutoDefault => format!("{base}·auto"),
        },
    }
}

/// One-sentence provenance note for the scaler button hover, or `None` for a plain
/// user-picked upscale (and any non-upscale state).
fn wgpu_scale_provenance_sentence(state: WgpuScaleState, i18n: I18n) -> Option<String> {
    let WgpuScaleState::Upscale {
        origin,
        substituted_below,
        ..
    } = state
    else {
        return None;
    };
    if let Some(threshold) = substituted_below {
        return Some(i18n.with_vars(
            "top_bar.scaler.origin.substituted",
            &[("threshold", format!("{threshold:.2}"))],
        ));
    }
    match origin {
        UpscaleDecisionOrigin::User => None,
        UpscaleDecisionOrigin::ProbeAuto => Some(i18n.text("top_bar.scaler.origin.probe")),
        UpscaleDecisionOrigin::AutoDefault => Some(i18n.text("top_bar.scaler.origin.auto")),
    }
}

fn scale_filter_label(filter: CpuScaleFilter) -> String {
    filter.label().replace(" / Area", "")
}

fn compact_wgpu_upscale_label(method: WgpuUpscaleMethod) -> String {
    match method {
        WgpuUpscaleMethod::Auto => "Auto".to_owned(),
        WgpuUpscaleMethod::None => "None".to_owned(),
        WgpuUpscaleMethod::WgslBilinear => "Linear".to_owned(),
        WgpuUpscaleMethod::WgslFsr1Style | WgpuUpscaleMethod::WgslFsr1EasuRcas => "FSR".to_owned(),
        WgpuUpscaleMethod::WgslNisStyle | WgpuUpscaleMethod::NvidiaNis => "NIS".to_owned(),
        WgpuUpscaleMethod::WgslAnime4kV32CnnX2S => "Anime4K S".to_owned(),
        WgpuUpscaleMethod::WgslAnime4kV32CnnX2M => "Anime4K M".to_owned(),
        WgpuUpscaleMethod::WgslSrLabSpanX2 => "SPAN".to_owned(),
        _ => abbreviate(method.label(), 14),
    }
}

fn abbreviate(label: &str, max_chars: usize) -> String {
    if label.chars().count() <= max_chars {
        return label.to_owned();
    }
    let kept = max_chars.saturating_sub(3);
    let mut text = label.chars().take(kept).collect::<String>();
    text.push_str("...");
    text
}

#[cfg(test)]
mod tests {
    use super::{
        cpu_filter_candidates, top_bar_scaler_summary, unique_candidates, wgpu_upscale_menu_current,
    };
    use crate::app::viewer::{
        CurrentViewState, PrepareScaleState, UpscaleDecisionOrigin, WgpuScaleState,
    };
    use crate::app::KernelChoice;
    use crate::core::i18n::{I18n, ResolvedLanguage};
    use crate::core::state::{
        AppSettings, CpuScaleFilter, DebandStrength, RendererMode, WgpuDownscaleMethod,
        WgpuUpscaleMethod,
    };
    use crate::core::worker::{DecodeBackend, PreparedTargetIntent};

    #[test]
    fn quick_candidates_deduplicate_and_append_current() {
        let candidates = unique_candidates(
            [CpuScaleFilter::Nearest, CpuScaleFilter::Nearest],
            CpuScaleFilter::Lanczos3,
        );

        assert_eq!(
            candidates,
            vec![CpuScaleFilter::Nearest, CpuScaleFilter::Lanczos3]
        );
    }

    #[test]
    fn cpu_candidates_include_current_even_when_configured_empty() {
        assert_eq!(
            cpu_filter_candidates(&[], CpuScaleFilter::CatmullRom),
            vec![CpuScaleFilter::CatmullRom]
        );
    }

    #[test]
    fn scaler_summary_uses_current_view_state() {
        let i18n = I18n::resolved(ResolvedLanguage::EnUs);
        let state = test_view_state(
            PrepareScaleState::CpuDownscale(CpuScaleFilter::Hamming),
            WgpuScaleState::Downscale(WgpuDownscaleMethod::PyramidLanczos3),
        );

        // The fixed display downscaler shows as plain "Downscale" on the chip
        // (the technical name lives in the tooltip).
        assert_eq!(
            top_bar_scaler_summary(Some(&state), i18n),
            "Hamming | Downscale"
        );
    }

    #[test]
    fn scaler_summary_marks_upscale_provenance() {
        let i18n = I18n::resolved(ResolvedLanguage::EnUs);

        // User-picked method: no suffix.
        let user = test_view_state(
            PrepareScaleState::Native,
            WgpuScaleState::Upscale {
                method: WgpuUpscaleMethod::WgslAnime4kV32CnnX2M,
                origin: UpscaleDecisionOrigin::User,
                substituted_below: None,
            },
        );
        assert_eq!(top_bar_scaler_summary(Some(&user), i18n), "Anime4K M");

        // AUTO routed by the probe.
        let probe = test_view_state(
            PrepareScaleState::Native,
            WgpuScaleState::Upscale {
                method: WgpuUpscaleMethod::WgslAnime4kV32CnnX2M,
                origin: UpscaleDecisionOrigin::ProbeAuto,
                substituted_below: None,
            },
        );
        assert_eq!(
            top_bar_scaler_summary(Some(&probe), i18n),
            "Anime4K M·probe"
        );

        // AUTO with no decision yet: the built-in default.
        let auto = test_view_state(
            PrepareScaleState::Native,
            WgpuScaleState::Upscale {
                method: WgpuUpscaleMethod::WgslFsr1EasuRcas,
                origin: UpscaleDecisionOrigin::AutoDefault,
                substituted_below: None,
            },
        );
        assert_eq!(top_bar_scaler_summary(Some(&auto), i18n), "FSR·auto");

        // Substitution wins over the origin suffix.
        let substituted = test_view_state(
            PrepareScaleState::Native,
            WgpuScaleState::Upscale {
                method: WgpuUpscaleMethod::WgslFsr1EasuRcas,
                origin: UpscaleDecisionOrigin::ProbeAuto,
                substituted_below: Some(1.10),
            },
        );
        assert_eq!(
            top_bar_scaler_summary(Some(&substituted), i18n),
            "FSR·<1.10x"
        );
    }

    #[test]
    fn scaler_summary_names_fast_prepare_backend() {
        let i18n = I18n::resolved(ResolvedLanguage::EnUs);
        let state = test_view_state(
            PrepareScaleState::FastSampledScaledDownscale(DecodeBackend::PngSampled),
            WgpuScaleState::Inactive,
        );

        assert_eq!(top_bar_scaler_summary(Some(&state), i18n), "PNG sampled");
    }

    #[test]
    fn scaler_summary_falls_back_to_plain_label_when_current_state_is_unknown() {
        let i18n = I18n::resolved(ResolvedLanguage::EnUs);

        assert_eq!(top_bar_scaler_summary(None, i18n), "Scale");
    }

    #[test]
    fn native_prepare_with_glow_kernel_shows_the_filter_label() {
        let i18n = I18n::resolved(ResolvedLanguage::EnUs);
        // Native prepare + a Glow draw-time kernel: the chip names the kernel
        // instead of reading a confusing "Native".
        let mut state = test_view_state(PrepareScaleState::Native, WgpuScaleState::Inactive);
        state.glow_kernel = Some(KernelChoice::CatmullRom);
        assert_eq!(top_bar_scaler_summary(Some(&state), i18n), "CatmullRom");

        // A real CPU prepare-upscale label is not shadowed by the kernel field.
        let mut cpu = test_view_state(
            PrepareScaleState::CpuUpscale(CpuScaleFilter::Lanczos3),
            WgpuScaleState::Inactive,
        );
        cpu.glow_kernel = Some(KernelChoice::Lanczos3);
        assert_eq!(top_bar_scaler_summary(Some(&cpu), i18n), "Lanczos3");
    }

    #[test]
    fn wgpu_inactive_summary_does_not_imply_gpu_scaler_is_active() {
        let i18n = I18n::resolved(ResolvedLanguage::EnUs);
        let state = test_view_state(PrepareScaleState::Native, WgpuScaleState::Inactive);

        assert_eq!(top_bar_scaler_summary(Some(&state), i18n), "Native");
    }

    #[test]
    fn wgpu_enabled_saved_none_is_shown_as_auto() {
        let settings = AppSettings {
            renderer_mode: RendererMode::Wgpu,
            wgpu_upscale_method: WgpuUpscaleMethod::None,
            ..AppSettings::default()
        };

        assert_eq!(
            wgpu_upscale_menu_current(&settings),
            WgpuUpscaleMethod::Auto
        );
    }

    fn test_view_state(
        prepare_scale: PrepareScaleState,
        wgpu_scale: WgpuScaleState,
    ) -> CurrentViewState {
        CurrentViewState {
            page_index: 0,
            decode_backend: DecodeBackend::ImageCrate,
            prepare_scale,
            wgpu_scale,
            glow_kernel: None,
            deband: DebandStrength::Off,
            target_intent: PreparedTargetIntent::NormalNavigation,
        }
    }
}
