use super::super::commands::AppCommand;
use super::super::debug_compare::DebugCompareTarget;
use super::super::{SuiSuiViewApp, ViewMode};
use super::{icons, path_labels, theme};
use crate::core::effects::ImageFilter;
use crate::core::state::{AiUpscaleBackend, FitMode, ReadingDirection};
use eframe::egui::{self, Align2, Button, Color32, Frame, Margin, RichText, Stroke};
use std::path::PathBuf;

impl SuiSuiViewApp {
    pub(in crate::app) fn show_top_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("focus_bar")
            .exact_height(theme::TOP_BAR_HEIGHT)
            .frame(
                Frame::new()
                    .fill(theme::TOOLBAR_FILL)
                    .stroke(Stroke::new(1.0, theme::SUBTLE_STROKE))
                    .inner_margin(Margin::symmetric(14, 7)),
            )
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing = egui::vec2(8.0, 0.0);
                ui.horizontal_centered(|ui| {
                    self.show_open_group(ui);
                    toolbar_separator(ui);
                    self.show_page_group(ui);
                    toolbar_separator(ui);
                    self.show_view_group(ui);
                    self.show_correction_group(ctx, ui);
                    toolbar_separator(ui);
                    self.show_debug_compare_group(ui);
                    toolbar_separator(ui);
                    self.show_bookmark_group(ctx, ui);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add(icon_button(icons::INFO, icons::IconStyle::Regular, 19.0))
                            .on_hover_text("정보")
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
                            .on_hover_text("환경설정")
                            .clicked()
                        {
                            self.settings_open = true;
                        }
                    });
                });
            });
    }

    fn show_open_group(&mut self, ui: &mut egui::Ui) {
        ui.menu_button(icons::icon_text(icons::FOLDER_OPEN, "열기"), |ui| {
            ui.set_min_width(320.0);
            if ui
                .button(icons::icon_text(icons::DOCUMENT, "파일 열기"))
                .clicked()
            {
                self.open_file_dialog();
                ui.close();
            }
            if ui
                .button(icons::icon_text(icons::FOLDER_OPEN, "폴더 열기"))
                .clicked()
            {
                self.open_folder_dialog();
                ui.close();
            }

            ui.separator();
            ui.label(RichText::new("최근").color(theme::TEXT_MUTED));
            let recent_books = self.store.recent_books(8);
            if recent_books.is_empty() {
                ui.add_enabled(false, egui::Label::new("최근 책 없음"));
                return;
            }

            for book in &recent_books {
                if let Some(path) = book.known_paths.last() {
                    let label =
                        path_labels::compact_start(path, path_labels::RECENT_PATH_LABEL_CHARS);
                    if ui.button(label).on_hover_text(path).clicked() {
                        self.open_path(PathBuf::from(path));
                        ui.close();
                    }
                } else {
                    ui.add_enabled(false, egui::Label::new(&book.title))
                        .on_hover_text("최근 책 경로를 찾을 수 없습니다.");
                }
            }
        });
    }

    fn show_page_group(&mut self, ui: &mut egui::Ui) {
        let has_book = self.source.is_some();
        let previous = ui
            .add_enabled(
                has_book,
                icon_button(icons::CHEVRON_LEFT, icons::IconStyle::Regular, 22.0),
            )
            .on_hover_text("이전 페이지");
        if previous.clicked() {
            self.previous_page();
        }

        let total_pages = self.source.as_ref().map_or(0, |source| source.page_count());
        let mut page = self.current_page.saturating_add(1) as i64;
        let response = ui.add_enabled(
            has_book,
            egui::DragValue::new(&mut page)
                .range(1..=total_pages.max(1) as i64)
                .speed(1)
                .suffix(format!(" / {}", total_pages)),
        );
        if response.changed() {
            let target = page.saturating_sub(1) as usize;
            let direction = if target >= self.current_page {
                super::super::NavigationDirection::Forward
            } else {
                super::super::NavigationDirection::Backward
            };
            self.set_page(target, direction);
        }

        let next = ui
            .add_enabled(
                has_book,
                icon_button(icons::CHEVRON_RIGHT, icons::IconStyle::Regular, 22.0),
            )
            .on_hover_text("다음 페이지");
        if next.clicked() {
            self.next_page();
        }
    }

    fn show_view_group(&mut self, ui: &mut egui::Ui) {
        let label = format!("보기: {}", self.fit_mode.label());
        ui.menu_button(icons::icon_text(icons::EYE, &label), |ui| {
            ui.set_min_width(260.0);
            ui.label("레이아웃");
            ui.horizontal(|ui| {
                for mode in [ViewMode::Single, ViewMode::Double] {
                    if ui
                        .selectable_label(self.view_mode == mode, view_mode_label(mode))
                        .clicked()
                    {
                        self.set_view_mode(mode);
                    }
                }
            });

            ui.separator();
            ui.label("읽기 방향");
            ui.horizontal(|ui| {
                for direction in [ReadingDirection::LeftToRight, ReadingDirection::RightToLeft] {
                    if ui
                        .selectable_label(self.reading_direction == direction, direction.label())
                        .clicked()
                    {
                        self.reading_direction = direction;
                        self.persist_current_bookmark();
                    }
                }
            });

            ui.separator();
            ui.label("맞춤");
            ui.horizontal_wrapped(|ui| {
                for mode in FitMode::ALL {
                    if ui
                        .selectable_label(self.fit_mode == mode, mode.label())
                        .clicked()
                    {
                        self.set_fit_mode(mode);
                    }
                }
            });

            ui.separator();
            ui.horizontal(|ui| {
                ui.label("줌");
                if ui.button("-").clicked() {
                    self.adjust_zoom(0.9);
                }
                ui.label(format!("{:.0}%", self.manual_zoom * 100.0));
                if ui.button("+").clicked() {
                    self.adjust_zoom(1.1);
                }
            });
        });
    }

    fn show_correction_group(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        ui.menu_button(icons::icon_text(icons::WAND, "보정"), |ui| {
            ui.set_min_width(220.0);
            let ai_enabled = self.settings.ai_upscale.backend == AiUpscaleBackend::RealEsrganNcnn;
            if ui
                .add_enabled(
                    ai_enabled && self.source.is_some(),
                    egui::Button::new("AI x4"),
                )
                .clicked()
            {
                self.upscale_current_page();
                ui.close();
            }
            ui.add_enabled_ui(self.source.is_some(), |ui| {
                let mut use_ai = self.use_ai_upscaled_pages;
                if ui.checkbox(&mut use_ai, "AI 결과 우선 표시").changed() {
                    self.set_use_ai_upscaled_pages(use_ai);
                }
            });

            let mut transition_effect = self.settings.transition_effect;
            if ui
                .checkbox(&mut transition_effect, "페이지 전환 효과")
                .changed()
            {
                let mut settings = self.settings.clone();
                settings.transition_effect = transition_effect;
                self.apply_settings(ctx, settings);
            }

            ui.separator();
            ui.label("필터");
            for filter in [
                ImageFilter::None,
                ImageFilter::Smooth,
                ImageFilter::SmoothSharpen,
                ImageFilter::RcasSharpen,
            ] {
                if ui
                    .selectable_label(self.effects.filter == filter, filter.label())
                    .clicked()
                {
                    self.apply_command(ctx, AppCommand::SetFilter(filter));
                }
            }
        });
    }

    fn show_debug_compare_group(&mut self, ui: &mut egui::Ui) {
        let has_book = self.source.is_some();
        let response = ui.add_enabled(
            has_book,
            egui::Button::new("비교")
                .selected(self.debug_compare.enabled)
                .min_size(egui::vec2(52.0, 34.0)),
        );
        if response.on_hover_text("디버그 좌우 비교 모드").clicked() {
            self.set_debug_compare_enabled(!self.debug_compare.enabled);
        }

        if self.debug_compare.enabled {
            compare_target_combo(ui, "compare_left", "A", &mut self.debug_compare.left);
            compare_target_combo(ui, "compare_right", "B", &mut self.debug_compare.right);
        }
    }

    fn show_bookmark_group(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        let bookmarked = self.current_page_is_bookmarked();
        let response = bookmark_toolbar_button(
            ui,
            self.source.is_some(),
            bookmarked,
            self.bookmark_popover_open,
        );
        if response.bookmark_clicked {
            self.toggle_current_page_bookmark();
        }
        if response.menu_clicked {
            self.toggle_bookmark_popover_below(response.rect);
        }
        if self.bookmark_popover_open && ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
            self.close_bookmark_popover();
        }
    }

    fn set_view_mode(&mut self, mode: ViewMode) {
        if self.view_mode == mode {
            return;
        }
        self.view_mode = mode;
        if self.source.is_some() {
            self.worker.set_page(
                self.current_page,
                self.last_nav_direction,
                self.target_long_edge,
                self.visible_page_count(),
                self.worker_options(),
            );
        }
    }
}

fn compare_target_combo(
    ui: &mut egui::Ui,
    id: &'static str,
    label: &'static str,
    target: &mut DebugCompareTarget,
) {
    egui::ComboBox::from_id_salt(id)
        .width(210.0)
        .selected_text(format!("{label}: {}", target.label()))
        .show_ui(ui, |ui| {
            for candidate in DebugCompareTarget::ALL {
                ui.selectable_value(target, candidate, candidate.label());
            }
        });
}

fn toolbar_separator(ui: &mut egui::Ui) {
    ui.add_space(4.0);
    ui.separator();
    ui.add_space(4.0);
}

fn icon_button(code: char, style: icons::IconStyle, size: f32) -> Button<'static> {
    Button::new(icons::icon(code, style, size, theme::TEXT_PRIMARY))
        .min_size(egui::vec2(36.0, 34.0))
        .fill(theme::CONTROL_FILL)
        .stroke(theme::subtle_stroke())
}

struct BookmarkToolbarButtonResponse {
    bookmark_clicked: bool,
    menu_clicked: bool,
    rect: egui::Rect,
}

fn bookmark_toolbar_button(
    ui: &mut egui::Ui,
    can_bookmark_page: bool,
    bookmarked: bool,
    open: bool,
) -> BookmarkToolbarButtonResponse {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(70.0, 38.0), egui::Sense::click());
    let icon_rect =
        egui::Rect::from_min_max(rect.min, egui::pos2(rect.left() + 44.0, rect.bottom()));
    let menu_rect = egui::Rect::from_min_max(egui::pos2(icon_rect.right(), rect.top()), rect.max);
    let pointer_pos = ui.input(|input| input.pointer.hover_pos());
    let icon_hovered = can_bookmark_page && pointer_pos.is_some_and(|pos| icon_rect.contains(pos));
    let menu_hovered = pointer_pos.is_some_and(|pos| menu_rect.contains(pos));

    let fill = Color32::from_rgb(27, 29, 33);
    let hover_fill = Color32::from_rgb(38, 41, 47);
    let stroke = Stroke::new(1.0, Color32::from_rgb(172, 31, 80));
    let icon_color = if can_bookmark_page {
        theme::ACCENT_HOVER
    } else {
        Color32::from_rgb(92, 98, 108)
    };
    let menu_color = theme::ACCENT_HOVER;

    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, egui::CornerRadius::same(7), fill);
    if icon_hovered {
        painter.rect_filled(
            icon_rect.shrink(2.0),
            egui::CornerRadius::same(6),
            hover_fill,
        );
    }
    if menu_hovered || open {
        painter.rect_filled(
            menu_rect.shrink(2.0),
            egui::CornerRadius::same(6),
            hover_fill,
        );
    }
    painter.rect_stroke(
        rect,
        egui::CornerRadius::same(7),
        stroke,
        egui::StrokeKind::Inside,
    );

    let icon_style = if bookmarked {
        icons::IconStyle::Filled
    } else {
        icons::IconStyle::Regular
    };
    painter.text(
        icon_rect.center(),
        Align2::CENTER_CENTER,
        icons::BOOKMARK.to_string(),
        icons::icon_font(icon_style, 21.0),
        icon_color,
    );

    let center = menu_rect.center();
    let left = egui::pos2(center.x - 5.0, center.y - 2.5);
    let mid = egui::pos2(center.x, center.y + 3.0);
    let right = egui::pos2(center.x + 5.0, center.y - 2.5);
    painter.line_segment([left, mid], Stroke::new(1.7, menu_color));
    painter.line_segment([mid, right], Stroke::new(1.7, menu_color));

    BookmarkToolbarButtonResponse {
        bookmark_clicked: response.clicked() && icon_hovered,
        menu_clicked: response.clicked() && menu_hovered,
        rect,
    }
}

fn view_mode_label(mode: ViewMode) -> &'static str {
    match mode {
        ViewMode::Single => "1장",
        ViewMode::Double => "2장",
    }
}
