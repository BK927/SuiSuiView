use super::state::CompareSide;
use super::DebugCompareTarget;
use crate::app::{adjustments::edit_effects, SuiSuiViewApp};
use crate::core::deband::DebandStrength;
use crate::core::i18n::I18n;
use crate::core::state::{GpuDownscaleChoice, WgpuUpscaleMethod};

impl SuiSuiViewApp {
    pub(in crate::app) fn show_compare_controls(&mut self, ui: &mut egui::Ui) {
        let i18n = self.i18n();
        let snapshot = self.debug_compare.snapshot;
        for (left, name) in [(true, "A"), (false, "B")] {
            let mut side = if left {
                self.debug_compare.left
            } else {
                self.debug_compare.right
            };
            ui.menu_button(
                format!("{name}: {}", side.target.label(snapshot, i18n)),
                |ui| {
                    ui.set_min_width(300.0);
                    egui::ComboBox::from_id_salt(("compare-model", name))
                        .selected_text(side.target.label(snapshot, i18n))
                        .show_ui(ui, |ui| {
                            let choices = std::iter::once(DebugCompareTarget::Current)
                                .chain(
                                    WgpuUpscaleMethod::SETTINGS_CHOICES
                                        .into_iter()
                                        .map(DebugCompareTarget::Model),
                                )
                                .chain(std::iter::once(DebugCompareTarget::Model(
                                    WgpuUpscaleMethod::NvidiaNis,
                                )));
                            for candidate in choices {
                                let unavailable = self.compare_unavailable_reason(candidate);
                                let response = ui.add_enabled(
                                    unavailable.is_none(),
                                    egui::Button::new(candidate.label(snapshot, i18n))
                                        .selected(side.target == candidate),
                                );
                                if response.clicked() {
                                    side.target = candidate;
                                }
                                if let Some(reason) = unavailable {
                                    response.on_disabled_hover_text(reason);
                                }
                            }
                        });
                    if let Some(reason) = self.compare_unavailable_reason(side.target) {
                        ui.label(reason);
                    }
                    edit_effects(ui, &mut side.effects, i18n);
                    edit_side_quality(ui, &mut side, i18n);
                    if ui.button(i18n.text("compare.reset")).clicked() {
                        side.effects = self.effects;
                        side.target = DebugCompareTarget::Current;
                        side.downscale = self.settings.expert_downscale.gpu;
                        side.linear = self.settings.linear_light_downscale;
                        side.deband = self.active_deband().strength();
                        side.custom_deband = self.settings.custom_deband;
                    }
                },
            );
            if left {
                self.debug_compare.left = side;
            } else {
                self.debug_compare.right = side;
            }
        }
        ui.menu_button(i18n.text("compare.options"), |ui| {
            ui.set_max_width(330.0);
            ui.checkbox(
                &mut self.debug_compare.side_by_side,
                i18n.text("compare.side_by_side"),
            );
            ui.add_enabled(
                !self.debug_compare.side_by_side,
                egui::Slider::new(&mut self.debug_compare.wipe, 0.0..=1.0)
                    .text(i18n.text("compare.wipe")),
            );
            ui.checkbox(&mut self.debug_compare.loupe, i18n.text("compare.loupe"));
            ui.add_enabled(
                self.debug_compare.loupe,
                egui::Slider::new(&mut self.debug_compare.loupe_zoom, 2.0..=8.0)
                    .text(i18n.text("compare.magnification")),
            );
            ui.small(i18n.text("compare.same_input"));
            ui.small(i18n.text("compare.snapshot"));
        });
    }

    pub(in crate::app) fn compare_unavailable_reason(
        &self,
        target: DebugCompareTarget,
    ) -> Option<String> {
        let i18n = self.i18n();
        if !self.can_paint_wgsl_effects() {
            return Some(i18n.text("compare.gpu_unavailable"));
        }
        let method = target.resolved(self.debug_compare.snapshot);
        if method == WgpuUpscaleMethod::NvidiaNis {
            return Some(i18n.text("compare.vendor_unavailable"));
        }
        if method == WgpuUpscaleMethod::WgslSrLabSpanX2
            && ![
                "SUISUIVIEW_EXPERIMENT_SPAN_MANIFEST",
                "SUISUIVIEW_SR_LAB_SPAN_MANIFEST",
            ]
            .into_iter()
            .any(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()))
        {
            return Some(i18n.text("compare.span_unavailable"));
        }
        None
    }
}

fn edit_side_quality(ui: &mut egui::Ui, side: &mut CompareSide, i18n: I18n) {
    ui.collapsing(i18n.text("adjust.downscale"), |ui| {
        egui::ComboBox::from_id_salt("comparison-downscale")
            .selected_text(side.downscale.method().label())
            .show_ui(ui, |ui| {
                for value in GpuDownscaleChoice::ALL {
                    ui.selectable_value(&mut side.downscale, value, value.method().label());
                }
            });
        ui.checkbox(&mut side.linear, i18n.text("adjust.linear"));
    });
    ui.collapsing(i18n.text("adjust.deband"), |ui| {
        egui::ComboBox::from_id_salt("comparison-deband")
            .selected_text(side.deband.label_i18n(i18n))
            .show_ui(ui, |ui| {
                for value in DebandStrength::ALL {
                    ui.selectable_value(&mut side.deband, value, value.label_i18n(i18n));
                }
            });
        if side.deband == DebandStrength::Custom {
            let d = &mut side.custom_deband;
            ui.add(
                egui::Slider::new(&mut d.threshold_hundredths, 0..=800)
                    .text(i18n.text("adjust.deband_strength"))
                    .custom_formatter(|v, _| format!("{:.2}", v / 100.0))
                    .custom_parser(crate::app::adjustments::parse_hundredths),
            );
            ui.add(egui::Slider::new(&mut d.radius, 1..=32).text(i18n.text("adjust.radius")));
            ui.add(
                egui::Slider::new(&mut d.grain_hundredths, 0..=200)
                    .text(i18n.text("adjust.grain"))
                    .custom_formatter(|v, _| format!("{:.2}", v / 100.0))
                    .custom_parser(crate::app::adjustments::parse_hundredths),
            );
            ui.add(
                egui::Slider::new(&mut d.iterations, 1..=4).text(i18n.text("adjust.iterations")),
            );
        }
    });
}
