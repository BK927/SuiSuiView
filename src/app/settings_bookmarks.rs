use super::settings::{checkbox_with_help, setting_group};
use super::SuiSuiViewApp;
use crate::core::i18n::I18n;
use crate::core::state::{AppSettings, TopBarItems};
use eframe::egui;

pub(in crate::app) fn show_view_settings(
    ui: &mut egui::Ui,
    draft: &mut AppSettings,
    changed: &mut bool,
    i18n: I18n,
) {
    setting_group(
        ui,
        &i18n.text("settings.view.toolbar.title"),
        &i18n.text("settings.view.toolbar.desc"),
        |ui| {
            *changed |= checkbox_with_help(
                ui,
                &mut draft.show_status_bar,
                &i18n.text("settings.view.status_bar"),
                &i18n.text("settings.view.status_bar.help"),
            );
            *changed |= checkbox_with_help(
                ui,
                &mut draft.top_bar_pinned,
                &i18n.text("settings.view.top_bar_pinned"),
                &i18n.text("settings.view.top_bar_pinned.help"),
            );
            *changed |= checkbox_with_help(
                ui,
                &mut draft.show_filename_overlay,
                &i18n.text("settings.view.filename_overlay"),
                &i18n.text("settings.view.filename_overlay.help"),
            );
        },
    );

    ui.add_space(8.0);
    setting_group(
        ui,
        &i18n.text("settings.view.top_bar_items.title"),
        &i18n.text("settings.view.top_bar_items.desc"),
        |ui| {
            *changed |= top_bar_item_checkbox(
                ui,
                &mut draft.top_bar_items.open,
                "settings.view.top_bar_items.open",
                "settings.view.top_bar_items.open.help",
                i18n,
            );
            *changed |= top_bar_item_checkbox(
                ui,
                &mut draft.top_bar_items.page,
                "settings.view.top_bar_items.page",
                "settings.view.top_bar_items.page.help",
                i18n,
            );
            *changed |= top_bar_item_checkbox(
                ui,
                &mut draft.top_bar_items.view,
                "settings.view.top_bar_items.view",
                "settings.view.top_bar_items.view.help",
                i18n,
            );
            *changed |= top_bar_item_checkbox(
                ui,
                &mut draft.top_bar_items.adjust,
                "settings.view.top_bar_items.adjust",
                "settings.view.top_bar_items.adjust.help",
                i18n,
            );
            *changed |= top_bar_item_checkbox(
                ui,
                &mut draft.top_bar_items.compare,
                "settings.view.top_bar_items.compare",
                "settings.view.top_bar_items.compare.help",
                i18n,
            );
            *changed |= top_bar_item_checkbox(
                ui,
                &mut draft.top_bar_items.bookmarks,
                "settings.view.top_bar_items.bookmarks",
                "settings.view.top_bar_items.bookmarks.help",
                i18n,
            );
            ui.add_space(4.0);
            if ui
                .button(i18n.text("settings.view.top_bar_items.reset"))
                .clicked()
            {
                draft.top_bar_items = TopBarItems::default();
                *changed = true;
            }
        },
    );

    ui.add_space(8.0);
    setting_group(
        ui,
        &i18n.text("settings.view.viewer.title"),
        &i18n.text("settings.view.viewer.desc"),
        |ui| {
            *changed |= checkbox_with_help(
                ui,
                &mut draft.show_main_border,
                &i18n.text("settings.view.main_border"),
                &i18n.text("settings.view.main_border.help"),
            );
            *changed |= checkbox_with_help(
                ui,
                &mut draft.show_page_arrows,
                &i18n.text("settings.view.page_arrows"),
                &i18n.text("settings.view.page_arrows.help"),
            );
            *changed |= checkbox_with_help(
                ui,
                &mut draft.remember_zoom_per_book,
                &i18n.text("settings.view.remember_zoom"),
                &i18n.text("settings.view.remember_zoom.help"),
            );
        },
    );
}

fn top_bar_item_checkbox(
    ui: &mut egui::Ui,
    value: &mut bool,
    label_key: &str,
    help_key: &str,
    i18n: I18n,
) -> bool {
    checkbox_with_help(ui, value, &i18n.text(label_key), &i18n.text(help_key))
}

impl SuiSuiViewApp {
    pub(in crate::app) fn show_bookmark_settings(
        &mut self,
        ui: &mut egui::Ui,
        draft: &mut AppSettings,
        changed: &mut bool,
        i18n: I18n,
    ) {
        setting_group(
            ui,
            &i18n.text("settings.bookmarks.resume.title"),
            &i18n.text("settings.bookmarks.resume.desc"),
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
                    &mut draft.share_state_between_instances,
                    &i18n.text("settings.bookmarks.share_state"),
                    &i18n.text("settings.bookmarks.share_state.help"),
                );
                ui.horizontal_wrapped(|ui| {
                    ui.label(i18n.text("settings.bookmarks.max_books"));
                    super::settings::info_icon(ui, &i18n.text("settings.bookmarks.max_books.help"));
                    *changed |= ui
                        .add(
                            egui::DragValue::new(&mut draft.max_remembered_books)
                                .range(1..=500)
                                .speed(1),
                        )
                        .changed();
                });
            },
        );

        ui.add_space(8.0);
        setting_group(
            ui,
            &i18n.text("settings.bookmarks.archive.title"),
            &i18n.text("settings.bookmarks.archive.desc"),
            |ui| {
                *changed |= checkbox_with_help(
                    ui,
                    &mut draft.remember_archive_page_name,
                    &i18n.text("settings.bookmarks.archive_page_name"),
                    &i18n.text("settings.bookmarks.archive_page_name.help"),
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
