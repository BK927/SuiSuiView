use super::SuiSuiViewApp;
use crate::core::deband::{CustomDeband, DebandStrength};
use crate::core::effects::{ImageFilter, ViewEffects};
use crate::core::i18n::I18n;
use crate::core::state::{
    AppSettings, CpuDownscaleChoice, ExpertDownscale, GpuDownscaleChoice, WgpuUpscaleMethod,
};
use std::time::{Duration, Instant};
mod cpu;

#[derive(Default)]
pub(super) struct AdjustmentWindow {
    pub open: bool,
    pub uncorrected: bool,
    pub save_at: Option<Instant>,
    cpu: Option<cpu::CpuAdjustmentWorker>,
    cpu_error: Option<String>,
}

/// Copy only this window's controls while painting. The full settings include
/// owned shortcut and history lists and are cloned only after an actual edit.
#[derive(Clone, Copy, PartialEq, Eq)]
struct AdjustmentOptions {
    expert_downscale: ExpertDownscale,
    linear_light_downscale: bool,
    deband: DebandStrength,
    custom_deband: CustomDeband,
    background_refine: bool,
    monitor_color_management: bool,
    wgpu_upscale_method: WgpuUpscaleMethod,
}

impl From<&AppSettings> for AdjustmentOptions {
    fn from(settings: &AppSettings) -> Self {
        Self {
            expert_downscale: settings.expert_downscale,
            linear_light_downscale: settings.linear_light_downscale,
            deband: settings.deband,
            custom_deband: settings.custom_deband,
            background_refine: settings.background_refine,
            monitor_color_management: settings.monitor_color_management,
            wgpu_upscale_method: settings.wgpu_upscale_method,
        }
    }
}

impl AdjustmentOptions {
    fn apply(self, settings: &mut AppSettings) {
        settings.expert_downscale = self.expert_downscale;
        settings.linear_light_downscale = self.linear_light_downscale;
        settings.deband = self.deband;
        settings.custom_deband = self.custom_deband;
        settings.background_refine = self.background_refine;
        settings.monitor_color_management = self.monitor_color_management;
    }
}

impl SuiSuiViewApp {
    pub(in crate::app) fn refinement_status_text(&self, ctx: &egui::Context) -> String {
        let status = super::gpu_paint::refine::status(ctx);
        let i18n = self.i18n();
        if let Some(reason) = status.skip_reason {
            let key = match reason {
                "memory_budget" => "adjust.skip_memory",
                "unsupported_method" => "adjust.skip_model",
                "texture_limit" | "invalid_size" | "size_overflow" => "adjust.skip_size",
                _ => "adjust.skip_other",
            };
            format!("{} ({reason})", i18n.text(key))
        } else if status.pending > 0 {
            i18n.with_vars(
                "adjust.refine_pending",
                &[
                    ("ready", status.ready.to_string()),
                    ("total", (status.ready + status.pending).to_string()),
                ],
            )
        } else if status.ready > 0 {
            i18n.text("adjust.refine_ready")
        } else {
            i18n.text("adjust.refine_inactive")
        }
    }

    pub(super) fn display_effects(&self) -> ViewEffects {
        if self.adjustments.uncorrected {
            self.effects.uncorrected()
        } else {
            self.effects.for_render()
        }
    }

    pub(super) fn schedule_adjustment_save(&mut self) {
        self.settings.view_adjustments = self.effects.saved();
        self.adjustments.save_at = Some(Instant::now() + Duration::from_millis(300));
        self.egui_ctx
            .request_repaint_after(Duration::from_millis(300));
    }

    pub(super) fn show_adjustment_window(&mut self, ctx: &egui::Context) {
        let i18n = self.i18n();
        let was_uncorrected = self.adjustments.uncorrected;
        self.adjustments.uncorrected = false;
        if let Some(at) = self.adjustments.save_at {
            let now = Instant::now();
            if now >= at {
                self.adjustments.save_at = None;
                if let Err(error) = self.store.update_settings(self.settings.clone()) {
                    self.notify_state_save_failed(&error);
                }
            } else {
                // An earlier repaint may have won egui's deadline coalescing.
                ctx.request_repaint_after(at.saturating_duration_since(now));
            }
        }
        if !self.adjustments.open {
            if was_uncorrected {
                ctx.request_repaint();
            }
            return;
        }

        let mut open = self.adjustments.open;
        let mut effects = self.effects;
        let before = AdjustmentOptions::from(&self.settings);
        let mut settings = before;
        if open {
            egui::Window::new(i18n.text("adjust.title"))
                .id(egui::Id::new("view-adjustments"))
                .open(&mut open)
                .resizable(false)
                .default_width(320.0)
                .vscroll(true)
                .max_height((ctx.available_rect().height() - 24.0).max(160.0))
                .show(ctx, |ui| {
                    edit_effects(ui, &mut effects, i18n);
                    ui.horizontal(|ui| {
                        if ui.button(i18n.text("adjust.reset_all")).clicked() {
                            effects = effects.uncorrected();
                            settings.expert_downscale = Default::default();
                            settings.linear_light_downscale = false;
                            settings.deband = DebandStrength::Off;
                            settings.custom_deband = Default::default();
                        }
                        self.adjustments.uncorrected = ui
                            .button(i18n.text("adjust.hold_original"))
                            .is_pointer_button_down_on();
                    });
                    ui.collapsing(i18n.text("adjust.downscale"), |ui| {
                        egui::ComboBox::from_id_salt("gpu-downscale")
                            .selected_text(format!("GPU · {:?}", settings.expert_downscale.gpu))
                            .show_ui(ui, |ui| {
                                for value in GpuDownscaleChoice::ALL {
                                    ui.selectable_value(
                                        &mut settings.expert_downscale.gpu,
                                        value,
                                        format!("Pyramid {value:?}"),
                                    );
                                }
                            });
                        egui::ComboBox::from_id_salt("cpu-downscale")
                            .selected_text(format!("CPU · {:?}", settings.expert_downscale.cpu))
                            .show_ui(ui, |ui| {
                                for value in CpuDownscaleChoice::ALL {
                                    ui.selectable_value(
                                        &mut settings.expert_downscale.cpu,
                                        value,
                                        format!("{value:?}"),
                                    );
                                }
                            });
                        ui.checkbox(
                            &mut settings.linear_light_downscale,
                            i18n.text("adjust.linear"),
                        );
                        ui.small(i18n.text("adjust.downscale_help"));
                    });
                    ui.collapsing(i18n.text("adjust.deband"), |ui| {
                        egui::ComboBox::from_id_salt("deband-control")
                            .selected_text(settings.deband.label_i18n(i18n))
                            .show_ui(ui, |ui| {
                                for value in DebandStrength::ALL {
                                    ui.selectable_value(
                                        &mut settings.deband,
                                        value,
                                        value.label_i18n(i18n),
                                    );
                                }
                            });
                        if settings.deband == DebandStrength::Custom {
                            let d = &mut settings.custom_deband;
                            ui.add(
                                egui::Slider::new(&mut d.threshold_hundredths, 0..=800)
                                    .text(i18n.text("adjust.deband_strength"))
                                    .custom_formatter(|v, _| format!("{:.2}", v / 100.0))
                                    .custom_parser(parse_hundredths),
                            );
                            ui.add(
                                egui::Slider::new(&mut d.radius, 1..=32)
                                    .text(i18n.text("adjust.radius")),
                            );
                            ui.add(
                                egui::Slider::new(&mut d.grain_hundredths, 0..=200)
                                    .text(i18n.text("adjust.grain"))
                                    .custom_formatter(|v, _| format!("{:.2}", v / 100.0))
                                    .custom_parser(parse_hundredths),
                            );
                            ui.collapsing(i18n.text("adjust.advanced"), |ui| {
                                ui.add(
                                    egui::Slider::new(&mut d.iterations, 1..=4)
                                        .text(i18n.text("adjust.iterations")),
                                );
                            });
                        }
                    });
                    let supported = self.gpu_effects_available
                        && settings.wgpu_upscale_method
                            != crate::core::state::WgpuUpscaleMethod::Auto
                        && super::realtime_sr::background::BackgroundRefiner::supports(
                            settings.wgpu_upscale_method,
                        );
                    ui.add_enabled(
                        supported || settings.background_refine,
                        egui::Checkbox::new(
                            &mut settings.background_refine,
                            i18n.text("adjust.background"),
                        ),
                    )
                    .on_hover_text(i18n.text("adjust.background_help"));
                    ui.add_enabled(
                        self.gpu_effects_available || settings.monitor_color_management,
                        egui::Checkbox::new(
                            &mut settings.monitor_color_management,
                            i18n.text("adjust.monitor"),
                        ),
                    )
                    .on_disabled_hover_text(i18n.text("settings.rendering.gpu_upscaler.disabled"));
                    if settings.monitor_color_management {
                        let status = super::monitor_color::status(ctx);
                        let response = ui.small(status.label(i18n));
                        if let Some(detail) = status.detail() {
                            response.on_hover_text(detail);
                        }
                    }
                    if settings.background_refine {
                        ui.small(self.refinement_status_text(ctx));
                    }
                });
        }
        self.adjustments.open = open;
        if effects != self.effects {
            self.update_effects(|value| *value = effects);
        }
        let decode_changed = self.settings.expert_downscale.cpu != settings.expert_downscale.cpu;
        let render_changed = before != settings;
        if render_changed {
            let controls = settings;
            let mut settings = self.settings.clone();
            controls.apply(&mut settings);
            settings.view_adjustments = self.effects.saved();
            if decode_changed {
                self.apply_settings(ctx, settings);
            } else {
                self.settings = settings;
                self.schedule_adjustment_save();
            }
            ctx.request_repaint();
        }
        if self.adjustments.uncorrected != was_uncorrected {
            ctx.request_repaint();
        }
    }
}

pub(super) fn edit_effects(ui: &mut egui::Ui, effects: &mut ViewEffects, i18n: I18n) -> bool {
    let before = *effects;
    egui::ComboBox::from_id_salt("adjust-filter")
        .selected_text(effects.filter.label_i18n(i18n))
        .show_ui(ui, |ui| {
            for filter in [
                ImageFilter::None,
                ImageFilter::Smooth,
                ImageFilter::SmoothSharpen,
                ImageFilter::RcasSharpen,
            ] {
                ui.selectable_value(&mut effects.filter, filter, filter.label_i18n(i18n));
            }
        });
    ui.horizontal(|ui| {
        ui.checkbox(&mut effects.gamma, i18n.text("adjust.gamma"));
        ui.add(
            egui::Slider::new(&mut effects.tone.gamma_pct, 10..=400)
                .custom_formatter(|v, _| format!("{:.2}", v / 100.0))
                .custom_parser(parse_hundredths),
        );
        if ui.small_button("↺").clicked() {
            effects.gamma = false;
            effects.tone.gamma_pct = 120;
        }
    });
    macro_rules! row {
        ($field:ident, $range:expr, $default:expr, $key:literal) => {
            ui.horizontal(|ui| {
                ui.add(egui::Slider::new(&mut effects.tone.$field, $range).text(i18n.text($key)));
                if ui.small_button("↺").clicked() {
                    effects.tone.$field = $default;
                }
            });
        };
    }
    row!(brightness_pct, -100..=100, 0, "adjust.brightness");
    row!(contrast_pct, 0..=200, 100, "adjust.contrast");
    row!(
        black,
        0..=effects.tone.white.saturating_sub(1),
        0,
        "adjust.black"
    );
    row!(
        white,
        effects.tone.black.saturating_add(1)..=255,
        255,
        "adjust.white"
    );
    row!(filter_pct, 0..=200, 100, "adjust.filter_strength");
    ui.checkbox(&mut effects.invert_colors, i18n.text("adjust.invert"));
    effects.tone.normalize();
    before != *effects
}

pub(super) fn parse_hundredths(text: &str) -> Option<f64> {
    text.trim()
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
        .map(|value| value * 100.0)
}
