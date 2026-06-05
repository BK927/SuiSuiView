use std::time::Instant;

use super::handoff_image_report::ImageHandoffMetrics;
use super::image_first_page::PreparedImageReport;
use super::wgpu_worker::elapsed_ms;

pub(super) fn draw_image_ui(
    ctx: &egui::Context,
    started_at: Instant,
    phase: &str,
    image_report: Option<&PreparedImageReport>,
    texture: Option<&egui::TextureHandle>,
    metrics: &ImageHandoffMetrics,
) -> bool {
    let mut quit = false;
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.fill(egui::Color32::from_rgb(7, 10, 16)))
        .show(ctx, |ui| {
            ui.add_space(10.0);
            ui.heading("SuiSuiView image handoff probe");
            ui.label(phase);
            ui.label(format!(
                "elapsed: {:.1} ms",
                elapsed_ms(started_at.elapsed())
            ));
            if let Some(report) = image_report {
                if let Some(error) = report.error.as_ref() {
                    ui.colored_label(egui::Color32::LIGHT_RED, error);
                } else {
                    ui.label(format!(
                        "open/read/prepare: {} / {} / {}",
                        format_ms(report.open_source_ms),
                        format_ms(report.read_page_ms),
                        format_ms(report.prepare_ms)
                    ));
                }
            } else {
                ui.label("image worker: running...");
            }
            ui.label(format!(
                "Glow image visible: {}",
                format_ms(metrics.glow_image_visible_ms)
            ));
            ui.label(format!(
                "WGPU image visible: {}",
                format_ms(metrics.first_wgpu_image_present_ms)
            ));
            ui.label(format!(
                "handoff gap: {}",
                format_ms(metrics.handoff_gap_ms)
            ));
            ui.separator();
            if let Some(texture) = texture {
                let available = ui.available_size();
                let max_width = available.x.max(120.0).min(560.0);
                let max_height = (available.y - 48.0).max(120.0).min(320.0);
                ui.add(
                    egui::Image::new(texture)
                        .max_width(max_width)
                        .max_height(max_height)
                        .corner_radius(egui::CornerRadius::same(4)),
                );
            }
            ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                quit = ui.button("Quit").clicked();
            });
        });
    quit
}

fn format_ms(value: Option<f64>) -> String {
    value
        .map(|value| format!("{value:.1} ms"))
        .unwrap_or_else(|| "pending".to_owned())
}
