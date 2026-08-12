use super::ui::{dialog, icons, theme};
use super::{
    gpu_paint, platform, settings_input, settings_performance, settings_rendering, settings_view,
    SuiSuiViewApp,
};
use crate::core::i18n::I18n;
use crate::core::state::{AppSettings, GpuEffectMode, Language, RendererMode, WgpuUpscaleMethod};
use egui::{self, RichText};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum SettingsSection {
    #[default]
    General,
    View,
    Reading,
    Rendering,
    Performance,
    Keyboard,
    Mouse,
    // File associations and Windows default-app registration only exist on
    // Windows, so the whole section is compiled out elsewhere.
    #[cfg(target_os = "windows")]
    FileAssociations,
}

impl SettingsSection {
    #[cfg(target_os = "windows")]
    const ALL: [Self; 8] = [
        Self::General,
        Self::View,
        Self::Reading,
        Self::Rendering,
        Self::Performance,
        Self::Keyboard,
        Self::Mouse,
        Self::FileAssociations,
    ];
    #[cfg(not(target_os = "windows"))]
    const ALL: [Self; 7] = [
        Self::General,
        Self::View,
        Self::Reading,
        Self::Rendering,
        Self::Performance,
        Self::Keyboard,
        Self::Mouse,
    ];

    fn label(self, i18n: I18n) -> String {
        match self {
            Self::General => i18n.text("settings.section.general"),
            Self::View => i18n.text("settings.section.view"),
            Self::Reading => i18n.text("settings.section.reading"),
            Self::Rendering => i18n.text("settings.section.rendering"),
            Self::Performance => i18n.text("settings.section.performance"),
            Self::Keyboard => i18n.text("settings.section.keyboard"),
            Self::Mouse => i18n.text("settings.section.mouse"),
            #[cfg(target_os = "windows")]
            Self::FileAssociations => i18n.text("settings.section.file_associations"),
        }
    }

    fn description(self, i18n: I18n) -> String {
        match self {
            Self::General => i18n.text("settings.section.general.desc"),
            Self::View => i18n.text("settings.section.view.desc"),
            Self::Reading => i18n.text("settings.section.reading.desc"),
            Self::Rendering => i18n.text("settings.section.rendering.desc"),
            Self::Performance => i18n.text("settings.section.performance.desc"),
            Self::Keyboard => i18n.text("settings.section.keyboard.desc"),
            Self::Mouse => i18n.text("settings.section.mouse.desc"),
            #[cfg(target_os = "windows")]
            Self::FileAssociations => i18n.text("settings.section.file_associations.desc"),
        }
    }

    fn icon(self) -> (char, icons::IconStyle) {
        match self {
            Self::General => (icons::SETTINGS, icons::IconStyle::Regular),
            Self::View => (icons::EYE, icons::IconStyle::Regular),
            Self::Reading => (icons::BOOKMARK, icons::IconStyle::Regular),
            Self::Rendering => (icons::IMAGE_SPARKLE, icons::IconStyle::Regular),
            Self::Performance => (icons::LOCK_OPEN, icons::IconStyle::Regular),
            Self::Keyboard => (icons::KEYBOARD, icons::IconStyle::Regular),
            Self::Mouse => (icons::CURSOR_CLICK, icons::IconStyle::Regular),
            #[cfg(target_os = "windows")]
            Self::FileAssociations => (icons::DOCUMENT, icons::IconStyle::Regular),
        }
    }
}

impl SuiSuiViewApp {
    pub(in crate::app) fn i18n(&self) -> I18n {
        I18n::from_language(self.settings.language)
    }

    pub(super) fn show_settings_window(&mut self, ctx: &egui::Context) {
        if !self.settings_open {
            return;
        }

        let mut open = self.settings_open;
        let mut draft = self.settings.clone();
        let mut changed = false;
        let fast_start_failure_notice = self.fast_start_failure_notice.clone();
        let mut fast_start_action = None;
        if draft.gpu_effect_mode != GpuEffectMode::Auto {
            draft.gpu_effect_mode = GpuEffectMode::Auto;
            changed = true;
        }
        let mut active_section = self.settings_section;
        let i18n = self.i18n();
        // Read-only live memory usage for the performance section summary. Captured here while
        // `self` is accessible; GPU figures mirror the render thread's pool byte counters.
        let live_ram_bytes = self.decoded_bytes + self.texture_cache_bytes();
        let (gpu_source_bytes, gpu_intermediate_bytes, gpu_draw_state_bytes) =
            gpu_paint::gpu_pool_bytes_live();
        let live_gpu_bytes = gpu_source_bytes + gpu_intermediate_bytes + gpu_draw_state_bytes;
        let dialog_size = dialog::bounded_dialog_size(
            ctx,
            dialog::SPLIT_DIALOG_SIZE,
            dialog::MIN_SPLIT_DIALOG_SIZE,
        );

        egui::Window::new(i18n.text("settings.window"))
            .open(&mut open)
            .fixed_size(dialog_size)
            .collapsible(false)
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing = egui::vec2(10.0, 8.0);
                let body_size = ui.available_size();
                let spacing_x = ui.spacing().item_spacing.x;
                let nav_size = egui::vec2(dialog::NAV_WIDTH, body_size.y);
                let content_size = egui::vec2(
                    (body_size.x - dialog::NAV_WIDTH - spacing_x).max(0.0),
                    body_size.y,
                );

                ui.horizontal(|ui| {
                    dialog::show_sized_frame(ui, nav_size, dialog::rail_frame(), |ui| {
                        ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                            ui.label(
                                RichText::new(i18n.text("settings.nav_title"))
                                    .strong()
                                    .size(15.0)
                                    .color(theme::TEXT_PRIMARY),
                            );
                            ui.add_space(8.0);
                            for section in SettingsSection::ALL {
                                let (icon, icon_style) = section.icon();
                                if dialog::nav_button(
                                    ui,
                                    active_section == section,
                                    icon,
                                    icon_style,
                                    &section.label(i18n),
                                )
                                .clicked()
                                {
                                    active_section = section;
                                }
                                ui.add_space(4.0);
                            }
                        });
                    });

                    dialog::show_sized_frame(ui, content_size, dialog::content_frame(), |ui| {
                        ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                            dialog::section_heading(
                                ui,
                                &active_section.label(i18n),
                                &active_section.description(i18n),
                            );

                            let content_height = ui.available_height();
                            egui::ScrollArea::vertical()
                                .id_salt(("settings_section", active_section.label(i18n)))
                                .max_height(content_height)
                                .auto_shrink([false, false])
                                .show(ui, |ui| match active_section {
                                    SettingsSection::General => {
                                        show_general_settings(ui, &mut draft, &mut changed, i18n);
                                    }
                                    SettingsSection::View => {
                                        settings_view::show_view_settings(
                                            ui,
                                            &mut draft,
                                            &mut changed,
                                            i18n,
                                        );
                                    }
                                    SettingsSection::Reading => {
                                        self.show_reading_settings(
                                            ui,
                                            &mut draft,
                                            &mut changed,
                                            i18n,
                                        );
                                    }
                                    SettingsSection::Rendering => {
                                        settings_rendering::show_rendering_settings(
                                            ui,
                                            &mut draft,
                                            &mut self.pending_gpu_acceleration,
                                            fast_start_failure_notice.as_ref(),
                                            &mut fast_start_action,
                                            &mut changed,
                                            i18n,
                                        );
                                    }
                                    SettingsSection::Performance => {
                                        settings_performance::show_decoder_settings(
                                            ui,
                                            &mut draft,
                                            &mut changed,
                                            i18n,
                                        );
                                        ui.add_space(8.0);
                                        settings_performance::show_performance_settings(
                                            ui,
                                            &mut draft,
                                            (live_ram_bytes, live_gpu_bytes),
                                            &mut changed,
                                            i18n,
                                        );
                                    }
                                    SettingsSection::Keyboard => {
                                        self.show_keyboard_settings(
                                            ctx,
                                            ui,
                                            &mut draft,
                                            &mut changed,
                                            i18n,
                                        );
                                    }
                                    SettingsSection::Mouse => {
                                        settings_input::show_mouse_settings(
                                            ui,
                                            &mut draft,
                                            &mut changed,
                                            i18n,
                                        );
                                    }
                                    #[cfg(target_os = "windows")]
                                    SettingsSection::FileAssociations => {
                                        self.show_file_association_settings(ui, i18n);
                                    }
                                });
                        });
                    });
                });
            });

        if self.settings_section == SettingsSection::Keyboard
            && active_section != SettingsSection::Keyboard
        {
            self.shortcut_capture = None;
            self.shortcut_conflict = None;
        }
        self.settings_section = active_section;
        self.settings_open = open;
        if !self.settings_open {
            self.shortcut_capture = None;
            self.shortcut_conflict = None;
            self.pending_gpu_acceleration = None;
        }
        self.show_gpu_acceleration_confirm_dialog(ctx, &mut draft, &mut changed);
        if let (Some(notice), Some(action)) =
            (fast_start_failure_notice.as_ref(), fast_start_action)
        {
            self.handle_fast_start_report_action(notice, action);
        }
        if changed {
            self.apply_settings(ctx, draft);
        }
    }

    fn show_gpu_acceleration_confirm_dialog(
        &mut self,
        ctx: &egui::Context,
        draft: &mut AppSettings,
        changed: &mut bool,
    ) {
        let Some(enable_gpu) = self.pending_gpu_acceleration else {
            return;
        };

        let viewport_rect = ctx.screen_rect();
        let dialog_size = egui::vec2(360.0, 154.0);
        let mut cancel_clicked = false;
        let mut restart_clicked = false;
        let i18n = self.i18n();

        egui::Area::new(egui::Id::new("gpu_acceleration_confirm_dialog"))
            .fixed_pos(viewport_rect.min)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                let (overlay_rect, _) =
                    ui.allocate_exact_size(viewport_rect.size(), egui::Sense::click());
                ui.painter().rect_filled(
                    overlay_rect,
                    egui::CornerRadius::ZERO,
                    egui::Color32::from_rgba_unmultiplied(0, 0, 0, 112),
                );

                let dialog_rect = egui::Rect::from_center_size(overlay_rect.center(), dialog_size);
                ui.scope_builder(egui::UiBuilder::new().max_rect(dialog_rect), |ui| {
                    egui::Frame::new()
                        .fill(egui::Color32::from_rgb(23, 25, 29))
                        .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(76, 82, 92)))
                        .corner_radius(egui::CornerRadius::same(8))
                        .inner_margin(egui::Margin::symmetric(16, 14))
                        .show(ui, |ui| {
                            ui.set_min_size(dialog_size - egui::vec2(32.0, 28.0));
                            ui.label(
                                RichText::new(i18n.text("settings.gpu_confirm.title"))
                                    .size(18.0)
                                    .strong()
                                    .color(theme::TEXT_PRIMARY),
                            );
                            ui.add_space(8.0);
                            ui.label(
                                RichText::new(i18n.text("settings.gpu_confirm.restart_required"))
                                    .size(14.0)
                                    .color(theme::TEXT_PRIMARY),
                            );
                            let detail = if enable_gpu {
                                i18n.text("settings.gpu_confirm.enable_detail")
                            } else {
                                i18n.text("settings.gpu_confirm.disable_detail")
                            };
                            ui.label(RichText::new(detail).size(12.5).color(theme::TEXT_MUTED));
                            ui.add_space(16.0);
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui
                                        .add_sized(
                                            [112.0, 34.0],
                                            egui::Button::new(
                                                i18n.text("settings.gpu_confirm.restart_now"),
                                            )
                                            .fill(theme::ACCENT)
                                            .stroke(egui::Stroke::new(1.0, theme::ACCENT_HOVER)),
                                        )
                                        .clicked()
                                    {
                                        restart_clicked = true;
                                    }
                                    if ui
                                        .add_sized(
                                            [72.0, 34.0],
                                            egui::Button::new(i18n.text("common.cancel"))
                                                .fill(egui::Color32::from_rgb(38, 41, 47))
                                                .stroke(egui::Stroke::new(
                                                    1.0,
                                                    egui::Color32::from_rgb(58, 64, 73),
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

        if restart_clicked {
            draft.renderer_mode = if enable_gpu {
                RendererMode::Wgpu
            } else {
                RendererMode::LowMemoryGlow
            };
            if enable_gpu && draft.wgpu_upscale_method == WgpuUpscaleMethod::None {
                draft.wgpu_upscale_method = WgpuUpscaleMethod::Auto;
            }
            self.pending_gpu_acceleration = None;
            *changed = true;
        } else if cancel_clicked {
            self.pending_gpu_acceleration = None;
        }
    }

    pub(super) fn apply_settings(&mut self, ctx: &egui::Context, settings: AppSettings) {
        match self.store.update_settings(settings) {
            Ok(settings) => self.install_persisted_settings(ctx, settings, true),
            Err(error) => {
                let latest = self.store.settings().clone();
                self.install_persisted_settings(ctx, latest, false);
                self.notify_state_save_failed(&error);
            }
        }
    }

    fn install_persisted_settings(
        &mut self,
        ctx: &egui::Context,
        settings: AppSettings,
        show_saved_status: bool,
    ) {
        let previous_decode = self.decode_options();
        let previous_preview = self.settings.progressive_preview_enabled;
        let previous_prefetch = self.settings.prefetch_enabled;
        let previous_cache_budget = self.cpu_cache_budget_bytes();
        let previous_gpu_effect_mode = self.settings.gpu_effect_mode;
        let previous_wgpu_upscale_method = self.settings.wgpu_upscale_method;
        let previous_renderer_mode = self.settings.renderer_mode;
        let mut textures_invalidated = false;

        self.settings = settings;
        if !self.settings.top_bar_items.compare && self.debug_compare.enabled {
            self.set_debug_compare_enabled(false);
        }
        self.refresh_single_instance_listener();
        // `update_settings` writes `state.json` itself, but a book record may still
        // be buffered behind the shared debounce timer; flush rather than discard.
        self.flush_deferred_state_save();
        platform::apply_window_level(ctx, self.settings.always_on_top);

        if previous_renderer_mode != self.settings.renderer_mode {
            match platform::restart_current_process() {
                Ok(()) => {
                    self.set_status(self.i18n().text("status.gpu_restart"));
                    self.close_requested = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    return;
                }
                Err(error) => {
                    self.set_status(
                        self.i18n().with_vars(
                            "status.gpu_restart_failed",
                            &[("error", error.to_string())],
                        ),
                    );
                }
            }
        }

        let decode_changed = previous_decode != self.decode_options();
        let preview_changed = previous_preview != self.settings.progressive_preview_enabled;
        if decode_changed || preview_changed {
            self.decoded_pages.clear();
            self.decoded_bytes = 0;
            if decode_changed {
                self.page_metrics.clear();
                self.note_strip_dims_changed();
            }
            self.textures.clear();
            textures_invalidated = true;
            self.page_errors.clear();
        } else if previous_cache_budget != self.cpu_cache_budget_bytes() {
            self.prune_decoded_cache();
        }
        if previous_gpu_effect_mode != self.settings.gpu_effect_mode
            || previous_wgpu_upscale_method != self.settings.wgpu_upscale_method
        {
            self.textures.clear();
            textures_invalidated = true;
            ctx.request_repaint();
        }

        if self.source.is_some()
            && (decode_changed
                || preview_changed
                || previous_prefetch != self.settings.prefetch_enabled
                || previous_cache_budget != self.cpu_cache_budget_bytes())
        {
            self.worker.set_page(
                self.worker_center_page(),
                self.last_nav_direction,
                self.target_long_edge,
                self.visible_page_count(),
                self.worker_options(),
            );
        }
        if textures_invalidated {
            self.request_original_texture_only_decode_if_needed();
        }
        if show_saved_status {
            self.set_status(self.i18n().text("status.settings_saved"));
        }
    }
}

fn show_general_settings(
    ui: &mut egui::Ui,
    draft: &mut AppSettings,
    changed: &mut bool,
    i18n: I18n,
) {
    setting_group(
        ui,
        &i18n.text("settings.general.behavior.title"),
        &i18n.text("settings.general.behavior.desc"),
        |ui| {
            egui::Grid::new("settings_language_grid")
                .num_columns(2)
                .spacing([14.0, 8.0])
                .show(ui, |ui| {
                    grid_label_with_help(
                        ui,
                        &i18n.text("settings.general.language"),
                        &i18n.text("settings.general.language.help"),
                    );
                    egui::ComboBox::from_id_salt("language")
                        .selected_text(draft.language.label(i18n))
                        .show_ui(ui, |ui| {
                            for language in Language::ALL {
                                *changed |= ui
                                    .selectable_value(
                                        &mut draft.language,
                                        language,
                                        language.label(i18n),
                                    )
                                    .changed();
                            }
                        });
                    ui.end_row();
                });
            ui.add_space(8.0);
            *changed |= checkbox_with_help(
                ui,
                &mut draft.confirm_delete,
                &i18n.text("settings.general.confirm_delete"),
                &i18n.text("settings.general.confirm_delete.help"),
            );
            *changed |= checkbox_with_help(
                ui,
                &mut draft.esc_to_quit,
                &i18n.text("settings.general.esc_to_quit"),
                &i18n.text("settings.general.esc_to_quit.help"),
            );
            *changed |= checkbox_with_help(
                ui,
                &mut draft.always_on_top,
                &i18n.text("settings.general.always_on_top"),
                &i18n.text("settings.general.always_on_top.help"),
            );
            *changed |= checkbox_with_help(
                ui,
                &mut draft.remember_recent_locations,
                &i18n.text("settings.general.remember_recent"),
                &i18n.text("settings.general.remember_recent.help"),
            );
            #[cfg(target_os = "windows")]
            {
                *changed |= checkbox_with_help(
                    ui,
                    &mut draft.single_instance,
                    &i18n.text("settings.general.single_instance"),
                    &i18n.text("settings.general.single_instance.help"),
                );
            }
            #[cfg(not(target_os = "windows"))]
            {
                ui.add_enabled(
                    false,
                    egui::Checkbox::new(
                        &mut draft.single_instance,
                        i18n.text("settings.general.single_instance"),
                    ),
                )
                .on_hover_text(i18n.text("settings.general.single_instance.unavailable"));
            }
            *changed |= checkbox_with_help(
                ui,
                &mut draft.show_toasts,
                &i18n.text("settings.general.show_toasts"),
                &i18n.text("settings.general.show_toasts.help"),
            );
        },
    );
}

pub(in crate::app) fn setting_group(
    ui: &mut egui::Ui,
    title: &str,
    description: &str,
    add_contents: impl FnOnce(&mut egui::Ui),
) {
    dialog::setting_card(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(
            RichText::new(title)
                .size(13.5)
                .strong()
                .color(theme::TEXT_PRIMARY),
        );
        ui.label(
            RichText::new(description)
                .size(12.0)
                .color(theme::TEXT_MUTED),
        );
        ui.add_space(8.0);
        add_contents(ui);
    });
}

pub(in crate::app) fn checkbox_with_help(
    ui: &mut egui::Ui,
    value: &mut bool,
    label: &str,
    help: &str,
) -> bool {
    let mut changed = false;
    ui.horizontal_wrapped(|ui| {
        changed |= ui.checkbox(value, label).on_hover_text(help).changed();
        info_icon(ui, help);
    });
    changed
}

pub(in crate::app) fn grid_label_with_help(ui: &mut egui::Ui, label: &str, help: &str) {
    ui.horizontal(|ui| {
        ui.add(egui::Label::new(label).sense(egui::Sense::hover()))
            .on_hover_text(help)
            .on_hover_cursor(egui::CursorIcon::Help);
        info_icon(ui, help);
    });
}

pub(in crate::app) fn info_icon(ui: &mut egui::Ui, help: &str) {
    ui.add(
        egui::Label::new(icons::icon(
            icons::INFO,
            icons::IconStyle::Regular,
            13.0,
            theme::TEXT_MUTED,
        ))
        .sense(egui::Sense::hover()),
    )
    .on_hover_text(help)
    .on_hover_cursor(egui::CursorIcon::Help);
}
