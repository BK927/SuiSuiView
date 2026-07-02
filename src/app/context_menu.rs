use super::{
    commands::{AppCommand, DeleteMode},
    SuiSuiViewApp,
};
use crate::core::effects::ImageFilter;
use crate::core::state::{FitMode, ReadingDirection};

impl SuiSuiViewApp {
    pub(in crate::app) fn show_context_menu(
        &mut self,
        ctx: &egui::Context,
        response: &egui::Response,
    ) {
        response.context_menu(|ui| {
            ui.set_min_width(280.0);
            let has_book = self.source.is_some();
            let i18n = self.i18n();

            self.context_action(
                ui,
                ctx,
                &i18n.text("context.open"),
                "F2",
                AppCommand::OpenFile,
                true,
            );
            self.context_action(
                ui,
                ctx,
                &i18n.text("context.open_folder"),
                "F",
                AppCommand::OpenFolder,
                true,
            );
            self.context_action(
                ui,
                ctx,
                &i18n.text("context.close"),
                "F4",
                AppCommand::CloseBook,
                has_book,
            );

            ui.separator();
            self.context_filter(
                ui,
                ctx,
                &ImageFilter::None.label_i18n(i18n),
                "U",
                ImageFilter::None,
                has_book,
            );
            self.context_filter(
                ui,
                ctx,
                &ImageFilter::Smooth.label_i18n(i18n),
                "I",
                ImageFilter::Smooth,
                has_book,
            );
            self.context_filter(
                ui,
                ctx,
                &ImageFilter::SmoothSharpen.label_i18n(i18n),
                "S",
                ImageFilter::SmoothSharpen,
                has_book,
            );

            ui.separator();
            ui.menu_button(i18n.text("context.image_move"), |ui| {
                self.context_action(
                    ui,
                    ctx,
                    &i18n.text("context.next_image"),
                    "PgDn",
                    AppCommand::NextPage,
                    has_book,
                );
                self.context_action(
                    ui,
                    ctx,
                    &i18n.text("context.previous_image"),
                    "PgUp",
                    AppCommand::PreviousPage,
                    has_book,
                );
                self.context_action(
                    ui,
                    ctx,
                    &i18n.text("context.first_image"),
                    "Home",
                    AppCommand::Home,
                    has_book,
                );
                self.context_action(
                    ui,
                    ctx,
                    &i18n.text("context.last_image"),
                    "End",
                    AppCommand::End,
                    has_book,
                );
                ui.separator();
                self.context_action(
                    ui,
                    ctx,
                    &i18n.text("context.next_10"),
                    "Ctrl+PgDn",
                    AppCommand::MovePages(10),
                    has_book,
                );
                self.context_action(
                    ui,
                    ctx,
                    &i18n.text("context.previous_10"),
                    "Ctrl+PgUp",
                    AppCommand::MovePages(-10),
                    has_book,
                );
                self.context_action(
                    ui,
                    ctx,
                    &i18n.text("context.random_next"),
                    "Ctrl+Alt+PgDn",
                    AppCommand::RandomForward,
                    has_book,
                );
                self.context_action(
                    ui,
                    ctx,
                    &i18n.text("context.random_previous"),
                    "Ctrl+Alt+PgUp",
                    AppCommand::RandomBackward,
                    has_book,
                );
                ui.separator();
                self.context_action(
                    ui,
                    ctx,
                    &i18n.text("context.next_book"),
                    "]",
                    AppCommand::NextBook,
                    has_book,
                );
                self.context_action(
                    ui,
                    ctx,
                    &i18n.text("context.previous_book"),
                    "[",
                    AppCommand::PreviousBook,
                    has_book,
                );
            });

            ui.menu_button(i18n.text("context.view_mode"), |ui| {
                self.context_fit_mode(
                    ui,
                    ctx,
                    &i18n.text("context.original_size"),
                    "0",
                    FitMode::Original,
                    has_book,
                );
                self.context_fit_mode(
                    ui,
                    ctx,
                    &i18n.text("context.fit_page"),
                    "1 / 9 / Z",
                    FitMode::FitPage,
                    has_book,
                );
                self.context_fit_mode(
                    ui,
                    ctx,
                    &i18n.text("context.fit_width"),
                    "8",
                    FitMode::FitWidth,
                    has_book,
                );
                ui.separator();
                self.context_action(
                    ui,
                    ctx,
                    &i18n.text("context.double_ltr"),
                    "7",
                    AppCommand::SetDouble(ReadingDirection::LeftToRight),
                    has_book,
                );
                self.context_action(
                    ui,
                    ctx,
                    &i18n.text("context.double_rtl"),
                    "6",
                    AppCommand::SetDouble(ReadingDirection::RightToLeft),
                    has_book,
                );
                self.context_action(
                    ui,
                    ctx,
                    &i18n.text("context.toggle_double"),
                    "2",
                    AppCommand::ToggleDouble,
                    has_book,
                );
            });

            ui.menu_button(i18n.text("context.zoom_menu"), |ui| {
                self.context_action(
                    ui,
                    ctx,
                    &i18n.text("context.zoom_in"),
                    "+",
                    AppCommand::Zoom(1.1),
                    has_book,
                );
                self.context_action(
                    ui,
                    ctx,
                    &i18n.text("context.zoom_out"),
                    "-",
                    AppCommand::Zoom(0.9),
                    has_book,
                );
                self.context_action(
                    ui,
                    ctx,
                    &i18n.text("context.zoom_fine_in"),
                    "Ctrl++",
                    AppCommand::ZoomFine(0.01),
                    has_book,
                );
                self.context_action(
                    ui,
                    ctx,
                    &i18n.text("context.zoom_fine_out"),
                    "Ctrl+-",
                    AppCommand::ZoomFine(-0.01),
                    has_book,
                );
            });

            ui.menu_button(i18n.text("context.rotate_menu"), |ui| {
                self.context_action(
                    ui,
                    ctx,
                    &i18n.text("context.rotate_0"),
                    "Alt+↑",
                    AppCommand::SetRotation(0),
                    has_book,
                );
                self.context_action(
                    ui,
                    ctx,
                    &i18n.text("context.rotate_left"),
                    "Alt+←",
                    AppCommand::SetRotation(3),
                    has_book,
                );
                self.context_action(
                    ui,
                    ctx,
                    &i18n.text("context.rotate_right"),
                    "Alt+→",
                    AppCommand::SetRotation(1),
                    has_book,
                );
                self.context_action(
                    ui,
                    ctx,
                    &i18n.text("context.rotate_180"),
                    "Alt+↓",
                    AppCommand::SetRotation(2),
                    has_book,
                );
                ui.separator();
                self.context_action(
                    ui,
                    ctx,
                    &i18n.text("context.rotate_ccw"),
                    "Ctrl+L",
                    AppCommand::RotateCounterClockwise,
                    has_book,
                );
                self.context_action(
                    ui,
                    ctx,
                    &i18n.text("context.rotate_cw"),
                    "Ctrl+R",
                    AppCommand::RotateClockwise,
                    has_book,
                );
            });

            ui.menu_button(i18n.text("context.processing"), |ui| {
                self.context_toggle(
                    ui,
                    ctx,
                    &i18n.text("context.invert"),
                    "Ctrl+I",
                    self.effects.invert_colors,
                    AppCommand::ToggleInvert,
                );
                self.context_toggle(
                    ui,
                    ctx,
                    &i18n.text("context.gamma"),
                    "Ctrl+G",
                    self.effects.gamma,
                    AppCommand::ToggleGamma,
                );
                self.context_toggle(
                    ui,
                    ctx,
                    &i18n.text("context.flip_vertical"),
                    "Ctrl+F",
                    self.effects.transform.flip_vertical,
                    AppCommand::ToggleFlipVertical,
                );
                self.context_toggle(
                    ui,
                    ctx,
                    &i18n.text("context.flip_horizontal"),
                    "Ctrl+M",
                    self.effects.transform.flip_horizontal,
                    AppCommand::ToggleFlipHorizontal,
                );
            });

            ui.separator();
            self.context_action(
                ui,
                ctx,
                &i18n.text("context.open_explorer"),
                "Ctrl+Enter",
                AppCommand::OpenExplorer,
                has_book,
            );
            self.context_action(
                ui,
                ctx,
                &i18n.text("context.delete_file"),
                "Del",
                AppCommand::Delete(DeleteMode::Recycle),
                has_book,
            );
            self.context_action(
                ui,
                ctx,
                &i18n.text("context.delete_permanent"),
                "Shift+Del",
                AppCommand::Delete(DeleteMode::Permanent),
                has_book,
            );
            self.context_action(
                ui,
                ctx,
                &i18n.text("context.copy_page"),
                "Ctrl+C",
                AppCommand::CopyPageImage,
                has_book,
            );
            self.context_action(
                ui,
                ctx,
                &i18n.text("context.copy_display"),
                "Ctrl+Alt+C",
                AppCommand::CopyDisplayImage,
                has_book,
            );
            self.context_action(
                ui,
                ctx,
                &i18n.text("context.copy_path"),
                "Ctrl+Alt+Shift+C",
                AppCommand::CopyPath,
                has_book,
            );

            ui.separator();
            self.context_action(
                ui,
                ctx,
                &i18n.text("context.fullscreen"),
                "F11",
                AppCommand::ToggleFullscreen,
                true,
            );
            self.context_action(
                ui,
                ctx,
                &i18n.text("context.maximize"),
                "M",
                AppCommand::ToggleMaximized,
                true,
            );
            self.context_action(
                ui,
                ctx,
                &i18n.text("context.minimize"),
                "Q",
                AppCommand::Minimize,
                true,
            );
            if context_selectable(
                ui,
                self.settings.always_on_top,
                &i18n.text("context.always_on_top"),
                "Ctrl+A",
                true,
            )
            .clicked()
            {
                self.apply_command(ctx, AppCommand::ToggleAlwaysOnTop);
                ui.close();
            }
            self.context_action(
                ui,
                ctx,
                &i18n.text("topbar.settings"),
                "F5",
                AppCommand::OpenSettings,
                true,
            );
            self.context_action(
                ui,
                ctx,
                &i18n.text("topbar.info"),
                "F1",
                AppCommand::OpenAbout,
                true,
            );
            self.context_action(
                ui,
                ctx,
                &i18n.text("context.quit"),
                "X",
                AppCommand::Quit,
                true,
            );
        });
    }

    fn context_action(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        label: &str,
        shortcut: &str,
        command: AppCommand,
        enabled: bool,
    ) {
        if context_button(ui, label, shortcut, enabled).clicked() {
            self.apply_command(ctx, command);
            ui.close();
        }
    }

    fn context_filter(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        label: &str,
        shortcut: &str,
        filter: ImageFilter,
        enabled: bool,
    ) {
        if context_selectable(ui, self.effects.filter == filter, label, shortcut, enabled).clicked()
        {
            self.apply_command(ctx, AppCommand::SetFilter(filter));
            ui.close();
        }
    }

    fn context_fit_mode(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        label: &str,
        shortcut: &str,
        mode: FitMode,
        enabled: bool,
    ) {
        if context_selectable(ui, self.fit_mode == mode, label, shortcut, enabled).clicked() {
            self.apply_command(ctx, AppCommand::SetFitMode(mode));
            ui.close();
        }
    }

    fn context_toggle(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        label: &str,
        shortcut: &str,
        selected: bool,
        command: AppCommand,
    ) {
        let enabled = self.source.is_some();
        if context_selectable(ui, selected, label, shortcut, enabled).clicked() {
            self.apply_command(ctx, command);
            ui.close();
        }
    }
}

fn context_button(ui: &mut egui::Ui, label: &str, shortcut: &str, enabled: bool) -> egui::Response {
    ui.add_enabled(
        enabled,
        egui::Button::new(label).shortcut_text(shortcut.to_owned()),
    )
}

fn context_selectable(
    ui: &mut egui::Ui,
    selected: bool,
    label: &str,
    shortcut: &str,
    enabled: bool,
) -> egui::Response {
    ui.add_enabled(
        enabled,
        egui::Button::new(label)
            .selected(selected)
            .shortcut_text(shortcut.to_owned()),
    )
}
