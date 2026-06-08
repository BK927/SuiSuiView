use super::{
    commands::DeleteMode,
    deletion::DeleteAfterPlan,
    ui::{dialog, theme},
    SuiSuiViewApp,
};
use eframe::egui::{self, Color32, CornerRadius, Frame, Margin, RichText, Stroke};

const DELETE_DIALOG_SIZE: egui::Vec2 = egui::vec2(430.0, 238.0);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::app) struct PendingDeleteDialog {
    mode: DeleteMode,
    plan: DeleteAfterPlan,
    skip_recycle_confirmation: bool,
}

impl PendingDeleteDialog {
    pub(in crate::app) fn new(mode: DeleteMode, plan: DeleteAfterPlan) -> Self {
        Self {
            mode,
            plan,
            skip_recycle_confirmation: false,
        }
    }
}

impl SuiSuiViewApp {
    pub(in crate::app) fn cancel_delete_confirmation(&mut self) {
        if self.pending_delete_dialog.take().is_some() {
            self.set_status("Delete cancelled.");
        }
    }

    pub(in crate::app) fn show_delete_confirmation_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut pending) = self.pending_delete_dialog.take() else {
            return;
        };
        let i18n = self.i18n();
        let screen = ctx.screen_rect();
        let dialog_size = dialog::bounded_dialog_size(
            ctx,
            DELETE_DIALOG_SIZE,
            egui::vec2(340.0, DELETE_DIALOG_SIZE.y),
        );
        let target = pending.plan.target.clone();
        let target_name = target
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| target.display().to_string());
        let target_path = target.display().to_string();
        let title = match pending.mode {
            DeleteMode::Recycle => i18n.text("delete.confirm.recycle.title"),
            DeleteMode::Permanent => i18n.text("delete.confirm.permanent.title"),
        };
        let detail = match pending.mode {
            DeleteMode::Recycle => i18n.text("delete.confirm.recycle.detail"),
            DeleteMode::Permanent => i18n.text("delete.confirm.permanent.detail"),
        };
        let confirm_label = match pending.mode {
            DeleteMode::Recycle => i18n.text("delete.confirm.recycle.action"),
            DeleteMode::Permanent => i18n.text("delete.confirm.permanent.action"),
        };
        let is_permanent = matches!(pending.mode, DeleteMode::Permanent);
        let mut cancel_clicked = false;
        let mut delete_clicked = false;

        egui::Area::new(egui::Id::new("delete_confirmation_dialog"))
            .fixed_pos(screen.min)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                let (overlay_rect, _) = ui.allocate_exact_size(screen.size(), egui::Sense::click());
                ui.painter().rect_filled(
                    overlay_rect,
                    CornerRadius::ZERO,
                    Color32::from_rgba_unmultiplied(0, 0, 0, 132),
                );

                let dialog_rect = egui::Rect::from_center_size(overlay_rect.center(), dialog_size);
                ui.scope_builder(egui::UiBuilder::new().max_rect(dialog_rect), |ui| {
                    Frame::new()
                        .fill(Color32::from_rgb(23, 25, 29))
                        .stroke(Stroke::new(
                            1.0,
                            if is_permanent {
                                theme::ACCENT_HOVER
                            } else {
                                Color32::from_rgb(76, 82, 92)
                            },
                        ))
                        .corner_radius(CornerRadius::same(8))
                        .inner_margin(Margin::symmetric(18, 16))
                        .show(ui, |ui| {
                            ui.set_min_size(dialog_size - egui::vec2(36.0, 32.0));
                            ui.label(
                                RichText::new(title)
                                    .size(19.0)
                                    .strong()
                                    .color(theme::TEXT_PRIMARY),
                            );
                            ui.add_space(8.0);
                            ui.label(RichText::new(detail).size(13.5).color(if is_permanent {
                                Color32::from_rgb(255, 184, 184)
                            } else {
                                theme::TEXT_PRIMARY
                            }));
                            ui.add_space(10.0);
                            Frame::new()
                                .fill(Color32::from_rgb(18, 20, 24))
                                .stroke(theme::subtle_stroke())
                                .corner_radius(CornerRadius::same(6))
                                .inner_margin(Margin::symmetric(10, 8))
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    ui.label(
                                        RichText::new(target_name.as_str())
                                            .size(14.0)
                                            .strong()
                                            .color(theme::TEXT_PRIMARY),
                                    );
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(target_path.as_str())
                                                .size(12.0)
                                                .color(theme::TEXT_MUTED),
                                        )
                                        .wrap(),
                                    );
                                });

                            if pending.mode == DeleteMode::Recycle {
                                ui.add_space(8.0);
                                ui.checkbox(
                                    &mut pending.skip_recycle_confirmation,
                                    i18n.text("delete.confirm.recycle.skip"),
                                );
                            } else {
                                ui.add_space(28.0);
                            }

                            ui.add_space(12.0);
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui
                                        .add_sized(
                                            [156.0, 36.0],
                                            egui::Button::new(confirm_label)
                                                .fill(if is_permanent {
                                                    theme::ACCENT
                                                } else {
                                                    Color32::from_rgb(46, 92, 67)
                                                })
                                                .stroke(Stroke::new(
                                                    1.0,
                                                    if is_permanent {
                                                        theme::ACCENT_HOVER
                                                    } else {
                                                        Color32::from_rgb(79, 139, 101)
                                                    },
                                                )),
                                        )
                                        .clicked()
                                    {
                                        delete_clicked = true;
                                    }
                                    if ui
                                        .add_sized(
                                            [86.0, 36.0],
                                            egui::Button::new(i18n.text("common.cancel"))
                                                .fill(Color32::from_rgb(38, 41, 47))
                                                .stroke(Stroke::new(
                                                    1.0,
                                                    Color32::from_rgb(58, 64, 73),
                                                )),
                                        )
                                        .clicked()
                                    {
                                        cancel_clicked = true;
                                    }
                                },
                            );
                        });
                });
            });

        if delete_clicked {
            if pending.skip_recycle_confirmation && pending.mode == DeleteMode::Recycle {
                self.settings.confirm_delete = false;
                self.store.update_settings(self.settings.clone());
            }
            self.execute_delete_plan(pending.mode, pending.plan);
        } else if cancel_clicked {
            self.set_status("Delete cancelled.");
        } else {
            self.pending_delete_dialog = Some(pending);
        }
    }
}
