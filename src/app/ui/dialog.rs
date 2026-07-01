use super::{icons, theme};
use egui::{
    self, text::LayoutJob, Button, Color32, CornerRadius, FontId, Frame, Margin, Response,
    RichText, Stroke, TextFormat,
};

pub(in crate::app) const SPLIT_DIALOG_SIZE: egui::Vec2 = egui::vec2(700.0, 500.0);
pub(in crate::app) const ABOUT_DIALOG_SIZE: egui::Vec2 = egui::vec2(640.0, 440.0);
pub(in crate::app) const MIN_SPLIT_DIALOG_SIZE: egui::Vec2 = egui::vec2(560.0, 340.0);
pub(in crate::app) const MIN_ABOUT_DIALOG_SIZE: egui::Vec2 = egui::vec2(520.0, 320.0);
pub(in crate::app) const NAV_WIDTH: f32 = 136.0;
pub(in crate::app) const DIALOG_SCREEN_MARGIN: f32 = 48.0;

pub(in crate::app) fn rail_frame() -> Frame {
    Frame::new()
        .fill(Color32::from_rgb(22, 24, 28))
        .stroke(theme::subtle_stroke())
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::same(10))
}

pub(in crate::app) fn content_frame() -> Frame {
    Frame::new()
        .fill(Color32::from_rgb(24, 27, 31))
        .stroke(theme::subtle_stroke())
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::same(14))
}

pub(in crate::app) fn bounded_dialog_size(
    ctx: &egui::Context,
    preferred: egui::Vec2,
    minimum: egui::Vec2,
) -> egui::Vec2 {
    let screen = ctx.screen_rect().size();
    let available = (screen - egui::vec2(DIALOG_SCREEN_MARGIN, DIALOG_SCREEN_MARGIN))
        .max(egui::vec2(320.0, 240.0));
    egui::vec2(
        preferred.x.min(available.x).max(minimum.x.min(available.x)),
        preferred.y.min(available.y).max(minimum.y.min(available.y)),
    )
}

pub(in crate::app) fn frame_inner_size(frame: &Frame, outer_size: egui::Vec2) -> egui::Vec2 {
    let margin = frame.total_margin().sum();
    egui::vec2(
        (outer_size.x - margin.x).max(0.0),
        (outer_size.y - margin.y).max(0.0),
    )
}

pub(in crate::app) fn show_sized_frame(
    ui: &mut egui::Ui,
    outer_size: egui::Vec2,
    frame: Frame,
    add_contents: impl FnOnce(&mut egui::Ui),
) {
    let inner_size = frame_inner_size(&frame, outer_size);
    ui.allocate_ui_with_layout(outer_size, egui::Layout::top_down(egui::Align::Min), |ui| {
        frame.show(ui, |ui| {
            ui.set_min_size(inner_size);
            add_contents(ui);
        });
    });
}

pub(in crate::app) fn setting_card(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui)) {
    Frame::new()
        .fill(Color32::from_rgb(29, 32, 36))
        .stroke(Stroke::new(1.0, Color32::from_rgb(58, 63, 70)))
        .corner_radius(CornerRadius::same(7))
        .inner_margin(Margin::symmetric(12, 10))
        .show(ui, add_contents);
}

pub(in crate::app) fn section_heading(ui: &mut egui::Ui, title: &str, description: &str) {
    ui.label(
        RichText::new(title)
            .size(18.0)
            .strong()
            .color(theme::TEXT_PRIMARY),
    );
    ui.label(
        RichText::new(description)
            .size(12.5)
            .color(theme::TEXT_MUTED),
    );
    ui.add_space(8.0);
}

pub(in crate::app) fn nav_button(
    ui: &mut egui::Ui,
    selected: bool,
    icon_code: char,
    icon_style: icons::IconStyle,
    label: &str,
) -> Response {
    let fill = if selected {
        theme::ROW_FILL_SELECTED
    } else {
        Color32::TRANSPARENT
    };
    let stroke = if selected {
        theme::selected_stroke()
    } else {
        Stroke::new(1.0, Color32::TRANSPARENT)
    };
    let color = if selected {
        theme::TEXT_PRIMARY
    } else {
        theme::TEXT_MUTED
    };

    ui.add_sized(
        [ui.available_width(), 36.0],
        Button::new(nav_label(icon_code, icon_style, label, color))
            .fill(fill)
            .stroke(stroke)
            .corner_radius(CornerRadius::same(6)),
    )
}

fn nav_label(
    icon_code: char,
    icon_style: icons::IconStyle,
    label: &str,
    color: Color32,
) -> egui::WidgetText {
    let mut job = LayoutJob::default();
    job.append(
        &icon_code.to_string(),
        0.0,
        TextFormat {
            font_id: icons::icon_font(icon_style, 17.0),
            color,
            ..Default::default()
        },
    );
    job.append(
        "  ",
        0.0,
        TextFormat {
            font_id: FontId::proportional(13.5),
            color,
            ..Default::default()
        },
    );
    job.append(
        label,
        0.0,
        TextFormat {
            font_id: FontId::proportional(13.5),
            color,
            ..Default::default()
        },
    );
    job.into()
}
