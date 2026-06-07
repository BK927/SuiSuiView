use super::super::SuiSuiViewApp;
use super::{icons, theme};
use crate::core::i18n::I18n;
use crate::core::state::{
    AppSettings, CpuScaleFilter, RendererMode, WgpuDownscaleMethod, WgpuUpscaleMethod,
};
use eframe::egui::{self, RichText};

impl SuiSuiViewApp {
    pub(in crate::app::ui) fn show_scale_group(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        let i18n = self.i18n();
        let summary = top_bar_scaler_summary(&self.settings, i18n);
        ui.menu_button(icons::icon_text(icons::IMAGE_SPARKLE, &summary), |ui| {
            self.hold_top_bar_open_for_menu();
            ui.set_min_width(320.0);
            self.show_cpu_filter_row(
                ctx,
                ui,
                i18n.text("topbar.scale.cpu_up"),
                self.settings.cpu_upscale_filter,
                |settings, filter| settings.cpu_upscale_filter = filter,
            );
            self.show_cpu_filter_row(
                ctx,
                ui,
                i18n.text("topbar.scale.cpu_down"),
                self.settings.cpu_downscale_filter,
                |settings, filter| settings.cpu_downscale_filter = filter,
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
                self.show_wgpu_downscale_row(ctx, ui, i18n);
            })
            .response
            .on_disabled_hover_text(i18n.text("topbar.scale.wgpu_disabled"));
        })
        .response
        .on_hover_text(top_bar_scaler_tooltip(&self.settings, i18n));
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

    fn show_wgpu_downscale_row(&mut self, ctx: &egui::Context, ui: &mut egui::Ui, i18n: I18n) {
        let current = self.settings.wgpu_downscale_method;
        ui.label(format!(
            "{}: {}",
            i18n.text("topbar.scale.wgpu_down"),
            current.label()
        ));
        let candidates =
            wgpu_downscale_candidates(&self.settings.top_bar_wgpu_downscale_methods, current);
        ui.horizontal_wrapped(|ui| {
            for candidate in candidates {
                if ui
                    .selectable_label(current == candidate, candidate.label())
                    .clicked()
                {
                    let mut settings = self.settings.clone();
                    settings.wgpu_downscale_method = candidate;
                    self.apply_settings(ctx, settings);
                }
            }
        });
    }
}

fn top_bar_scaler_summary(settings: &AppSettings, i18n: I18n) -> String {
    let mut summary = format!(
        "{}: C {}/{}",
        i18n.text("topbar.scale"),
        compact_cpu_filter_label(settings.cpu_upscale_filter),
        compact_cpu_filter_label(settings.cpu_downscale_filter)
    );
    if matches!(settings.renderer_mode, RendererMode::Wgpu) {
        summary.push_str(&format!(
            " | G {}/{}",
            compact_wgpu_upscale_label(wgpu_upscale_menu_current(settings)),
            compact_wgpu_downscale_label(settings.wgpu_downscale_method)
        ));
    }
    summary
}

fn top_bar_scaler_tooltip(settings: &AppSettings, i18n: I18n) -> String {
    let mut lines = vec![
        format!(
            "{}: {}",
            i18n.text("topbar.scale.cpu_up"),
            settings.cpu_upscale_filter.label()
        ),
        format!(
            "{}: {}",
            i18n.text("topbar.scale.cpu_down"),
            settings.cpu_downscale_filter.label()
        ),
    ];
    if matches!(settings.renderer_mode, RendererMode::Wgpu) {
        lines.push(format!(
            "{}: {}",
            i18n.text("topbar.scale.wgpu_up"),
            wgpu_upscale_menu_current(settings).settings_label_i18n(i18n)
        ));
        lines.push(format!(
            "{}: {}",
            i18n.text("topbar.scale.wgpu_down"),
            settings.wgpu_downscale_method.label()
        ));
    } else {
        lines.push(i18n.text("topbar.scale.wgpu_disabled"));
        lines.push(format!(
            "{}: {}",
            i18n.text("topbar.scale.wgpu_up_saved"),
            settings.wgpu_upscale_method.label_i18n(i18n)
        ));
        lines.push(format!(
            "{}: {}",
            i18n.text("topbar.scale.wgpu_down_saved"),
            settings.wgpu_downscale_method.label()
        ));
    }
    lines.join("\n")
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

fn wgpu_downscale_candidates(
    configured: &[WgpuDownscaleMethod],
    current: WgpuDownscaleMethod,
) -> Vec<WgpuDownscaleMethod> {
    unique_candidates(
        configured
            .iter()
            .copied()
            .filter(|method| WgpuDownscaleMethod::ALL.contains(method)),
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

fn compact_cpu_filter_label(filter: CpuScaleFilter) -> &'static str {
    match filter {
        CpuScaleFilter::Nearest => "Near",
        CpuScaleFilter::Box => "Box",
        CpuScaleFilter::Bilinear => "Linear",
        CpuScaleFilter::Hamming => "Hamming",
        CpuScaleFilter::CatmullRom => "Catmull",
        CpuScaleFilter::Mitchell => "Mitchell",
        CpuScaleFilter::Gaussian => "Gauss",
        CpuScaleFilter::Lanczos2 => "Lz2",
        CpuScaleFilter::Lanczos3 => "Lz3",
    }
}

fn compact_wgpu_downscale_label(method: WgpuDownscaleMethod) -> &'static str {
    match method {
        WgpuDownscaleMethod::Nearest => "Near",
        WgpuDownscaleMethod::Bilinear => "Linear",
        WgpuDownscaleMethod::Box => "Box",
        WgpuDownscaleMethod::Hamming => "Hamming",
        WgpuDownscaleMethod::CatmullRom => "Catmull",
        WgpuDownscaleMethod::Mitchell => "Mitchell",
        WgpuDownscaleMethod::Lanczos2 => "Lz2",
        WgpuDownscaleMethod::Lanczos3 => "Lz3",
        WgpuDownscaleMethod::HardwareMipmapLinear => "Mipmap",
        WgpuDownscaleMethod::PyramidBoxTent => "Py+Box",
        WgpuDownscaleMethod::PyramidHamming => "Py+Ham",
        WgpuDownscaleMethod::PyramidMitchell => "Py+Mit",
        WgpuDownscaleMethod::PyramidLanczos2 => "Py+Lz2",
        WgpuDownscaleMethod::PyramidLanczos3 => "Py+Lz3",
    }
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
    use crate::core::i18n::{I18n, ResolvedLanguage};
    use crate::core::state::{
        AppSettings, CpuScaleFilter, RendererMode, WgpuDownscaleMethod, WgpuUpscaleMethod,
    };

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
    fn scaler_summary_differs_between_wgpu_on_and_off() {
        let i18n = I18n::resolved(ResolvedLanguage::EnUs);
        let off = AppSettings::default();
        let on = AppSettings {
            renderer_mode: RendererMode::Wgpu,
            wgpu_upscale_method: WgpuUpscaleMethod::Auto,
            wgpu_downscale_method: WgpuDownscaleMethod::PyramidLanczos3,
            ..AppSettings::default()
        };

        assert_eq!(
            top_bar_scaler_summary(&off, i18n),
            "Scale: C Catmull/Hamming"
        );
        assert_eq!(
            top_bar_scaler_summary(&on, i18n),
            "Scale: C Catmull/Hamming | G Auto/Py+Lz3"
        );
    }

    #[test]
    fn wgpu_off_summary_does_not_imply_gpu_scaler_is_active() {
        let i18n = I18n::resolved(ResolvedLanguage::EnUs);
        let settings = AppSettings {
            renderer_mode: RendererMode::LowMemoryGlow,
            wgpu_upscale_method: WgpuUpscaleMethod::Auto,
            wgpu_downscale_method: WgpuDownscaleMethod::PyramidLanczos3,
            ..AppSettings::default()
        };

        assert_eq!(
            top_bar_scaler_summary(&settings, i18n),
            "Scale: C Catmull/Hamming"
        );
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
}
