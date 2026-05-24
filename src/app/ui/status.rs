use super::super::SuiSuiViewApp;
use eframe::egui::{self, Align2, Color32, CornerRadius, Frame, Margin, Stroke};
use std::time::Duration;

const TOAST_VISIBLE_FOR: Duration = Duration::from_secs(4);

impl SuiSuiViewApp {
    pub(in crate::app) fn show_status_surfaces(&self, ctx: &egui::Context) {
        if self.settings.show_status_bar {
            self.show_status_bar(ctx);
        } else {
            self.show_status_toast(ctx);
        }
    }

    fn show_status_bar(&self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(&self.status);
                if self.source.is_some() {
                    ui.separator();
                    ui.label(format!(
                        "CPU cache: {:.1}/{:.0} MB",
                        self.decoded_bytes as f32 / (1024.0 * 1024.0),
                        self.cpu_cache_budget_bytes() as f32 / (1024.0 * 1024.0)
                    ));
                    ui.separator();
                    ui.label(if self.gpu_effects_available {
                        "GPU effects: available"
                    } else {
                        "GPU effects: CPU fallback"
                    });
                }
            });
        });
    }

    fn show_status_toast(&self, ctx: &egui::Context) {
        let elapsed = self.toast_updated_at.elapsed();
        if self.toast.is_empty() || elapsed > TOAST_VISIBLE_FOR {
            return;
        }
        ctx.request_repaint_after(TOAST_VISIBLE_FOR - elapsed);

        let top_offset = if self.top_bar_is_visible(ctx) {
            58.0
        } else {
            16.0
        };

        egui::Area::new(egui::Id::new("status_toast"))
            .anchor(Align2::RIGHT_TOP, egui::vec2(-16.0, top_offset))
            .order(egui::Order::Foreground)
            .interactable(false)
            .show(ctx, |ui| {
                Frame::new()
                    .fill(Color32::from_rgb(31, 33, 36))
                    .stroke(Stroke::new(1.0, Color32::from_rgb(74, 78, 84)))
                    .corner_radius(CornerRadius::same(6))
                    .inner_margin(Margin::symmetric(10, 7))
                    .show(ui, |ui| {
                        ui.label(&self.toast);
                    });
            });
    }
}
