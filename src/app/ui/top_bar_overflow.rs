use super::super::SuiSuiViewApp;
use super::icons;
use super::top_bar::{icon_button, toolbar_separator};
use super::top_bar_groups::{responsive_top_bar_layout, TopBarGroup};

const TOP_BAR_ACTION_BUTTON_COUNT: f32 = 3.0;

impl SuiSuiViewApp {
    pub(in crate::app::ui) fn show_top_bar_contents(
        &mut self,
        ctx: &egui::Context,
        ui: &mut egui::Ui,
    ) {
        ui.spacing_mut().item_spacing = egui::vec2(8.0, 0.0);
        ui.horizontal_centered(|ui| {
            let action_width = top_bar_action_width(ui.spacing().item_spacing.x);
            let left_width = (ui.available_width() - action_width).max(0.0);
            let total_pages = self.source.as_ref().map_or(0, |source| source.page_count());
            let layout = responsive_top_bar_layout(
                self.settings.top_bar_items,
                left_width,
                self.debug_compare.enabled,
                total_pages,
            );

            ui.allocate_ui_with_layout(
                egui::vec2(left_width, ui.available_height()),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.set_clip_rect(ui.clip_rect().intersect(ui.max_rect()));
                    self.show_top_bar_group_row(ctx, ui, &layout.inline_groups);
                    if !layout.overflow_groups.is_empty() {
                        if !layout.inline_groups.is_empty() {
                            toolbar_separator(ui);
                        }
                        self.show_top_bar_more_menu(ctx, ui, &layout.overflow_groups);
                    }
                },
            );
            self.show_top_bar_actions(ui);
        });
    }

    fn show_top_bar_group_row(
        &mut self,
        ctx: &egui::Context,
        ui: &mut egui::Ui,
        groups: &[TopBarGroup],
    ) {
        let mut first_group = true;
        for group in groups {
            if !first_group {
                toolbar_separator(ui);
            }
            self.show_top_bar_group(ctx, ui, *group);
            first_group = false;
        }
    }

    fn show_top_bar_group(&mut self, ctx: &egui::Context, ui: &mut egui::Ui, group: TopBarGroup) {
        match group {
            TopBarGroup::Open => self.show_open_group(ui),
            TopBarGroup::Page => self.show_page_group(ui),
            TopBarGroup::View => self.show_view_group(ctx, ui),
            TopBarGroup::Adjust => self.show_correction_group(ctx, ui),
            TopBarGroup::Compare => self.show_debug_compare_group(ui),
            TopBarGroup::Bookmarks => self.show_bookmark_group(ctx, ui),
        }
    }

    fn show_top_bar_more_menu(
        &mut self,
        ctx: &egui::Context,
        ui: &mut egui::Ui,
        groups: &[TopBarGroup],
    ) {
        let tooltip = self.i18n().text("topbar.more");
        let button = icon_button(icons::MORE_HORIZONTAL, icons::IconStyle::Regular, 20.0);
        let (response, _) = egui::containers::menu::MenuButton::from_button(button).ui(ui, |ui| {
            self.hold_top_bar_open_for_menu();
            ui.set_min_width(320.0);
            let mut first_group = true;
            for group in groups {
                if !first_group {
                    ui.separator();
                }
                ui.horizontal(|ui| {
                    self.show_top_bar_group(ctx, ui, *group);
                });
                first_group = false;
            }
        });
        response.on_hover_text(tooltip);
    }

    fn show_top_bar_actions(&mut self, ui: &mut egui::Ui) {
        let i18n = self.i18n();
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add(icon_button(icons::INFO, icons::IconStyle::Regular, 19.0))
                .on_hover_text(i18n.text("topbar.info"))
                .clicked()
            {
                self.open_about_window();
            }
            if ui
                .add(icon_button(
                    icons::SETTINGS,
                    icons::IconStyle::Regular,
                    20.0,
                ))
                .on_hover_text(i18n.text("topbar.settings"))
                .clicked()
            {
                self.settings_open = true;
            }
            self.show_top_bar_pin_button(ui);
        });
    }
}

fn top_bar_action_width(item_spacing: f32) -> f32 {
    36.0 * TOP_BAR_ACTION_BUTTON_COUNT + item_spacing * (TOP_BAR_ACTION_BUTTON_COUNT - 1.0)
}
