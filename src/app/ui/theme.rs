use eframe::egui::{self, Color32, CornerRadius, Stroke};

pub(in crate::app) const TOP_BAR_HEIGHT: f32 = 56.0;
pub(in crate::app) const VIEWER_BG: Color32 = Color32::from_rgb(13, 15, 18);
pub(in crate::app) const TOOLBAR_FILL: Color32 = Color32::from_rgb(30, 32, 35);
pub(in crate::app) const CONTROL_FILL: Color32 = Color32::from_rgb(36, 38, 42);
pub(in crate::app) const CONTROL_FILL_HOVER: Color32 = Color32::from_rgb(48, 51, 56);
pub(in crate::app) const CONTROL_FILL_ACTIVE: Color32 = Color32::from_rgb(23, 95, 126);
pub(in crate::app) const POPOVER_FILL: Color32 = Color32::from_rgb(26, 28, 32);
pub(in crate::app) const ROW_FILL: Color32 = Color32::from_rgb(31, 34, 38);
pub(in crate::app) const ROW_FILL_SELECTED: Color32 = Color32::from_rgb(34, 47, 55);
pub(in crate::app) const INPUT_FILL: Color32 = Color32::from_rgb(20, 22, 26);
pub(in crate::app) const ACCENT: Color32 = Color32::from_rgb(190, 24, 70);
pub(in crate::app) const ACCENT_HOVER: Color32 = Color32::from_rgb(214, 38, 88);
pub(in crate::app) const SELECT_STROKE: Color32 = Color32::from_rgb(16, 151, 205);
pub(in crate::app) const SUBTLE_STROKE: Color32 = Color32::from_rgb(70, 74, 82);
pub(in crate::app) const TEXT_PRIMARY: Color32 = Color32::from_rgb(235, 238, 242);
pub(in crate::app) const TEXT_MUTED: Color32 = Color32::from_rgb(168, 172, 180);

pub(in crate::app) fn apply_app_theme(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    style.visuals = egui::Visuals::dark();
    style.visuals.panel_fill = VIEWER_BG;
    style.visuals.window_fill = POPOVER_FILL;
    style.visuals.extreme_bg_color = INPUT_FILL;
    style.visuals.faint_bg_color = Color32::from_rgb(28, 31, 35);
    style.visuals.selection.bg_fill = CONTROL_FILL_ACTIVE;
    style.visuals.selection.stroke = Stroke::new(1.0, Color32::WHITE);
    style.visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT_MUTED);
    style.visuals.widgets.inactive.bg_fill = CONTROL_FILL;
    style.visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, SUBTLE_STROKE);
    style.visuals.widgets.inactive.corner_radius = CornerRadius::same(6);
    style.visuals.widgets.hovered.bg_fill = CONTROL_FILL_HOVER;
    style.visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(92, 97, 106));
    style.visuals.widgets.hovered.corner_radius = CornerRadius::same(6);
    style.visuals.widgets.active.bg_fill = CONTROL_FILL_ACTIVE;
    style.visuals.widgets.active.bg_stroke = Stroke::new(1.0, SELECT_STROKE);
    style.visuals.widgets.active.corner_radius = CornerRadius::same(6);
    style.visuals.window_stroke = Stroke::new(1.0, SUBTLE_STROKE);
    style.spacing.button_padding = egui::vec2(12.0, 7.0);
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    ctx.set_style(style);
}

pub(in crate::app) fn subtle_stroke() -> Stroke {
    Stroke::new(1.0, SUBTLE_STROKE)
}

pub(in crate::app) fn selected_stroke() -> Stroke {
    Stroke::new(1.4, SELECT_STROKE)
}
