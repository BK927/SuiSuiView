use super::settings::{checkbox_with_help, grid_label_with_help, setting_group};
use crate::core::i18n::I18n;
use crate::core::state::{
    AppSettings, CpuScaleFilter, TopBarItems, WgpuUpscaleMethod, DEFAULT_TOP_BAR_CPU_SCALE_FILTERS,
    DEFAULT_TOP_BAR_WGPU_UPSCALE_METHODS,
};

pub(in crate::app) fn show_view_settings(
    ui: &mut egui::Ui,
    draft: &mut AppSettings,
    changed: &mut bool,
    i18n: I18n,
) {
    // Toolbar group: pin toggle plus the top-bar item toggles, merged from the
    // former separate "top bar items" group. Item-level i18n keys stay as-is.
    setting_group(
        ui,
        &i18n.text("settings.view.toolbar.title"),
        &i18n.text("settings.view.toolbar.desc"),
        |ui| {
            *changed |= checkbox_with_help(
                ui,
                &mut draft.top_bar_pinned,
                &i18n.text("settings.view.top_bar_pinned"),
                &i18n.text("settings.view.top_bar_pinned.help"),
            );
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
        },
    );

    ui.add_space(8.0);
    // "화면 표시" group: status bar + filename overlay relocated from the toolbar
    // group, joined with the former viewer-display controls. The group reuses the
    // settings.view.viewer.* keys (retitled in the catalog); item keys stay as-is.
    setting_group(
        ui,
        &i18n.text("settings.view.viewer.title"),
        &i18n.text("settings.view.viewer.desc"),
        |ui| {
            *changed |= checkbox_with_help(
                ui,
                &mut draft.show_status_bar,
                &i18n.text("settings.view.status_bar"),
                &i18n.text("settings.view.status_bar.help"),
            );
            *changed |= checkbox_with_help(
                ui,
                &mut draft.show_filename_overlay,
                &i18n.text("settings.view.filename_overlay"),
                &i18n.text("settings.view.filename_overlay.help"),
            );
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
                &mut draft.pixel_grid_enabled,
                &i18n.text("settings.view.pixel_grid"),
                &i18n.text("settings.view.pixel_grid.help"),
            );
            ui.add_enabled_ui(draft.pixel_grid_enabled, |ui| {
                ui.horizontal(|ui| {
                    grid_label_with_help(
                        ui,
                        &i18n.text("settings.view.pixel_grid_threshold"),
                        &i18n.text("settings.view.pixel_grid_threshold.help"),
                    );
                    *changed |= ui
                        .add(
                            egui::DragValue::new(&mut draft.pixel_grid_min_zoom_pct)
                                .range(200..=6400)
                                .speed(10)
                                .suffix("%"),
                        )
                        .changed();
                });
            });
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

// established call surface; a params struct would be pure boilerplate
#[allow(clippy::too_many_arguments)]
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
