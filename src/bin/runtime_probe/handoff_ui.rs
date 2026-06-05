use std::time::Instant;

use super::handoff::HandoffMetrics;
use super::wgpu_worker::elapsed_ms;

pub(super) fn draw_handoff_ui(
    ctx: &egui::Context,
    started_at: Instant,
    phase: &str,
    probe_text: &mut String,
    focus_id: egui::Id,
    focus_requested: &mut bool,
    metrics: &HandoffMetrics,
) -> bool {
    let mut quit = false;
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.fill(egui::Color32::from_rgb(7, 10, 16)))
        .show(ctx, |ui| {
            ui.add_space(14.0);
            ui.heading("SuiSuiView renderer handoff probe");
            ui.label(phase);
            ui.label(format!(
                "elapsed: {:.1} ms",
                elapsed_ms(started_at.elapsed())
            ));
            ui.separator();

            let response = ui.add(
                egui::TextEdit::singleline(probe_text)
                    .id(focus_id)
                    .desired_width(320.0),
            );
            if !*focus_requested {
                response.request_focus();
                *focus_requested = true;
            }
            let focus_has = ui.memory(|memory| memory.has_focus(response.id));
            ui.label(format!(
                "field focus: {}",
                if focus_has { "present" } else { "missing" }
            ));
            ui.separator();

            ui.label(format!(
                "Glow first visible: {}",
                format_ms(metrics.first_glow_visible_ms)
            ));
            ui.label(format!(
                "Glow last present: {}",
                format_ms(metrics.last_glow_present_ms)
            ));
            ui.label(format!(
                "WGPU first present: {}",
                format_ms(metrics.first_wgpu_present_ms)
            ));
            ui.label(format!(
                "handoff gap: {}",
                format_ms(metrics.handoff_gap_ms)
            ));
            ui.label(format!(
                "WGPU attach: painter {} / surface {}",
                format_ms(metrics.painter_new_ms),
                format_ms(metrics.set_window_ms)
            ));

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
