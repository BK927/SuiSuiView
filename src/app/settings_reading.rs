use super::settings::{checkbox_with_help, grid_label_with_help, setting_group};
use super::SuiSuiViewApp;
use crate::core::i18n::I18n;
use crate::core::state::{
    AppSettings, EdgePageAction, LargeImageAnchor, PageTransitionStyle, WheelMode,
    STRIP_DRAG_SCROLL_PCT_MAX, STRIP_DRAG_SCROLL_PCT_MIN, STRIP_WHEEL_SCROLL_PCT_MAX,
    STRIP_WHEEL_SCROLL_PCT_MIN,
};

impl SuiSuiViewApp {
    pub(in crate::app) fn show_reading_settings(
        &mut self,
        ui: &mut egui::Ui,
        draft: &mut AppSettings,
        changed: &mut bool,
        i18n: I18n,
    ) {
        setting_group(
            ui,
            &i18n.text("settings.reading.turning.title"),
            &i18n.text("settings.reading.turning.desc"),
            |ui| {
                egui::Grid::new("settings_reading_turning_grid")
                    .num_columns(2)
                    .spacing([14.0, 8.0])
                    .show(ui, |ui| {
                        // Wheel navigation mode moved here from the Mouse section;
                        // the i18n key stays settings.mouse.wheel_mode by design.
                        grid_label_with_help(
                            ui,
                            &i18n.text("settings.mouse.wheel_mode"),
                            &i18n.text("settings.mouse.wheel_mode.help"),
                        );
                        egui::ComboBox::from_id_salt("wheel_mode")
                            .selected_text(draft.wheel_mode.label_i18n(i18n))
                            .show_ui(ui, |ui| {
                                for mode in WheelMode::ALL {
                                    *changed |= ui
                                        .selectable_value(
                                            &mut draft.wheel_mode,
                                            mode,
                                            mode.label_i18n(i18n),
                                        )
                                        .changed();
                                }
                            });
                        ui.end_row();

                        // Large-image anchor moved here from the Mouse section;
                        // the i18n key stays settings.mouse.large_anchor by design.
                        grid_label_with_help(
                            ui,
                            &i18n.text("settings.mouse.large_anchor"),
                            &i18n.text("settings.mouse.large_anchor.help"),
                        );
                        egui::ComboBox::from_id_salt("large_image_anchor")
                            .selected_text(draft.large_image_anchor.label_i18n(i18n))
                            .show_ui(ui, |ui| {
                                for anchor in LargeImageAnchor::ALL {
                                    *changed |= ui
                                        .selectable_value(
                                            &mut draft.large_image_anchor,
                                            anchor,
                                            anchor.label_i18n(i18n),
                                        )
                                        .changed();
                                }
                            });
                        ui.end_row();

                        // Page transition effect moved here from Rendering;
                        // the i18n key stays settings.rendering.transition by design.
                        grid_label_with_help(
                            ui,
                            &i18n.text("settings.rendering.transition"),
                            &i18n.text("settings.rendering.transition.help"),
                        );
                        let mut transition_style = draft.effective_page_transition_style();
                        egui::ComboBox::from_id_salt("page_transition_style")
                            .selected_text(transition_style.label_i18n(i18n))
                            .show_ui(ui, |ui| {
                                for style in PageTransitionStyle::ALL {
                                    *changed |= ui
                                        .selectable_value(
                                            &mut transition_style,
                                            style,
                                            style.label_i18n(i18n),
                                        )
                                        .changed();
                                }
                            });
                        draft.set_page_transition_style(transition_style);
                        ui.end_row();
                    });
            },
        );

        ui.add_space(8.0);
        // Page-edge behavior moved here from General; the group keeps its
        // settings.general.edge.* i18n keys by design.
        setting_group(
            ui,
            &i18n.text("settings.general.edge.title"),
            &i18n.text("settings.general.edge.desc"),
            |ui| {
                egui::Grid::new("settings_edge_page_grid")
                    .num_columns(2)
                    .spacing([14.0, 8.0])
                    .show(ui, |ui| {
                        grid_label_with_help(
                            ui,
                            &i18n.text("settings.general.edge.image"),
                            &i18n.text("settings.general.edge.image.help"),
                        );
                        egui::ComboBox::from_id_salt("image_edge_page_action")
                            .selected_text(draft.image_edge_page_action.label_i18n(i18n))
                            .show_ui(ui, |ui| {
                                for action in EdgePageAction::ALL {
                                    *changed |= ui
                                        .selectable_value(
                                            &mut draft.image_edge_page_action,
                                            action,
                                            action.label_i18n(i18n),
                                        )
                                        .changed();
                                }
                            });
                        ui.end_row();

                        grid_label_with_help(
                            ui,
                            &i18n.text("settings.general.edge.archive"),
                            &i18n.text("settings.general.edge.archive.help"),
                        );
                        egui::ComboBox::from_id_salt("archive_edge_page_action")
                            .selected_text(draft.archive_edge_page_action.label_i18n(i18n))
                            .show_ui(ui, |ui| {
                                for action in EdgePageAction::ALL {
                                    *changed |= ui
                                        .selectable_value(
                                            &mut draft.archive_edge_page_action,
                                            action,
                                            action.label_i18n(i18n),
                                        )
                                        .changed();
                                }
                            });
                        ui.end_row();
                    });
            },
        );

        ui.add_space(8.0);
        // Resume group: the former Bookmarks resume + archive controls, plus
        // remember_zoom relocated from the View viewer group. Item-level i18n
        // keys (settings.bookmarks.* / settings.view.remember_zoom) stay as-is.
        setting_group(
            ui,
            &i18n.text("settings.reading.resume.title"),
            &i18n.text("settings.reading.resume.desc"),
            |ui| {
                *changed |= checkbox_with_help(
                    ui,
                    &mut draft.auto_save_reading_position,
                    &i18n.text("settings.bookmarks.auto_save"),
                    &i18n.text("settings.bookmarks.auto_save.help"),
                );
                *changed |= checkbox_with_help(
                    ui,
                    &mut draft.resume_by_file_identity,
                    &i18n.text("settings.bookmarks.file_identity"),
                    &i18n.text("settings.bookmarks.file_identity.help"),
                );
                *changed |= checkbox_with_help(
                    ui,
                    &mut draft.remember_archive_page_name,
                    &i18n.text("settings.bookmarks.archive_page_name"),
                    &i18n.text("settings.bookmarks.archive_page_name.help"),
                );
                *changed |= checkbox_with_help(
                    ui,
                    &mut draft.remember_zoom_per_book,
                    &i18n.text("settings.view.remember_zoom"),
                    &i18n.text("settings.view.remember_zoom.help"),
                );
                if ui
                    .button(i18n.text("settings.bookmarks.clear_archive"))
                    .clicked()
                {
                    let cleared = self.store.clear_archive_page_names();
                    self.set_status(i18n.with_vars(
                        "settings.bookmarks.clear_archive.status",
                        &[("count", cleared.to_string())],
                    ));
                }
            },
        );
    }
}

pub(in crate::app) fn show_vertical_scroll_settings(
    ui: &mut egui::Ui,
    draft: &mut AppSettings,
    changed: &mut bool,
    i18n: I18n,
) {
    // Vertical-strip scroll amounts moved from the Mouse navigation group;
    // item-level i18n keys stay settings.mouse.strip_* by design.
    setting_group(
        ui,
        &i18n.text("settings.vertical_scroll.scroll.title"),
        &i18n.text("settings.vertical_scroll.scroll.desc"),
        |ui| {
            egui::Grid::new("settings_vertical_scroll_grid")
                .num_columns(2)
                .spacing([14.0, 8.0])
                .show(ui, |ui| {
                    grid_label_with_help(
                        ui,
                        &i18n.text("settings.mouse.strip_wheel"),
                        &i18n.text("settings.mouse.strip_wheel.help"),
                    );
                    *changed |= ui
                        .add(
                            egui::DragValue::new(&mut draft.strip_wheel_scroll_pct)
                                .range(STRIP_WHEEL_SCROLL_PCT_MIN..=STRIP_WHEEL_SCROLL_PCT_MAX)
                                .speed(10)
                                .suffix("%"),
                        )
                        .changed();
                    ui.end_row();

                    grid_label_with_help(
                        ui,
                        &i18n.text("settings.mouse.strip_drag"),
                        &i18n.text("settings.mouse.strip_drag.help"),
                    );
                    *changed |= ui
                        .add(
                            egui::DragValue::new(&mut draft.strip_drag_scroll_pct)
                                .range(STRIP_DRAG_SCROLL_PCT_MIN..=STRIP_DRAG_SCROLL_PCT_MAX)
                                .speed(5)
                                .suffix("%"),
                        )
                        .changed();
                    ui.end_row();
                });
        },
    );

    ui.add_space(8.0);
    // Cut stepping: strip_panel_snap moved from the Mouse navigation group;
    // the i18n key stays settings.mouse.strip_panel_snap by design.
    setting_group(
        ui,
        &i18n.text("settings.vertical_scroll.cut.title"),
        &i18n.text("settings.vertical_scroll.cut.desc"),
        |ui| {
            *changed |= checkbox_with_help(
                ui,
                &mut draft.strip_panel_snap,
                &i18n.text("settings.mouse.strip_panel_snap"),
                &i18n.text("settings.mouse.strip_panel_snap.help"),
            );
        },
    );
}
