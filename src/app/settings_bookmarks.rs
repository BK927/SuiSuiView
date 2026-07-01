use super::settings::{checkbox_with_help, setting_group};
use super::SuiSuiViewApp;
use crate::core::i18n::I18n;
use crate::core::state::{
    AppSettings, CpuScaleFilter, TopBarItems, WgpuDownscaleMethod, WgpuUpscaleMethod,
    DEFAULT_TOP_BAR_CPU_SCALE_FILTERS, DEFAULT_TOP_BAR_WGPU_DOWNSCALE_METHODS,
    DEFAULT_TOP_BAR_WGPU_UPSCALE_METHODS,
};
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
        &i18n.text("settings.view.top_bar_scalers.title"),
        &i18n.text("settings.view.top_bar_scalers.desc"),
        |ui| {
            show_quick_scaler_candidates(
                ui,
                &i18n.text("settings.view.top_bar_scalers.cpu"),
                &mut draft.top_bar_cpu_scale_filters,
                CpuScaleFilter::ALL,
                &DEFAULT_TOP_BAR_CPU_SCALE_FILTERS,
                |filter| filter.label().to_owned(),
                changed,
                i18n,
            );
            ui.separator();
            show_quick_scaler_candidates(
                ui,
                &i18n.text("settings.view.top_bar_scalers.wgpu_up"),
                &mut draft.top_bar_wgpu_upscale_methods,
                WgpuUpscaleMethod::SETTINGS_CHOICES
                    .into_iter()
                    .filter(|method| *method != WgpuUpscaleMethod::None),
                &DEFAULT_TOP_BAR_WGPU_UPSCALE_METHODS,
                |method| method.settings_label_i18n(i18n),
                changed,
                i18n,
            );
            ui.separator();
            show_quick_scaler_candidates(
                ui,
                &i18n.text("settings.view.top_bar_scalers.wgpu_down"),
                &mut draft.top_bar_wgpu_downscale_methods,
                WgpuDownscaleMethod::ALL,
                &DEFAULT_TOP_BAR_WGPU_DOWNSCALE_METHODS,
                |method| method.label().to_owned(),
                changed,
                i18n,
            );
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

fn show_quick_scaler_candidates<T, I>(
    ui: &mut egui::Ui,
    title: &str,
    selected: &mut Vec<T>,
    choices: I,
    defaults: &[T],
    label: impl Fn(T) -> String,
    changed: &mut bool,
    i18n: I18n,
) where
    T: Copy + Eq,
    I: IntoIterator<Item = T>,
{
    ui.horizontal_wrapped(|ui| {
        ui.label(title);
        if ui
            .small_button(i18n.text("settings.view.top_bar_scalers.defaults"))
            .clicked()
        {
            *selected = defaults.to_vec();
            *changed = true;
        }
        if ui
            .small_button(i18n.text("settings.view.top_bar_scalers.clear"))
            .clicked()
        {
            selected.clear();
            *changed = true;
        }
    });
    ui.horizontal_wrapped(|ui| {
        for choice in choices {
            let mut enabled = selected.contains(&choice);
            if ui.checkbox(&mut enabled, label(choice)).changed() {
                if enabled {
                    if !selected.contains(&choice) {
                        selected.push(choice);
                    }
                } else {
                    selected.retain(|candidate| *candidate != choice);
                }
                *changed = true;
            }
        }
    });
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
