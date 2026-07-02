use super::super::commands::AppCommand;
use super::super::debug_compare::DebugCompareTarget;
use super::super::{SuiSuiViewApp, ViewMode};
use super::{icons, path_labels, theme};
use crate::core::effects::ImageFilter;
use crate::core::i18n::I18n;
use crate::core::state::{FitMode, PageTransitionStyle};
use egui::{
    self, Align2, Button, Color32, FontId, Frame, Margin, RichText, Sense, Stroke, Vec2,
};
use std::path::PathBuf;
use std::time::{Duration, Instant};

const OPEN_MENU_MIN_WIDTH: f32 = 560.0;
const OPEN_MENU_MAX_VIEWPORT_FRACTION: f32 = 0.8;
const OPEN_MENU_OUTER_MARGIN: f32 = 24.0;
const RECENT_ROW_HORIZONTAL_PADDING: f32 = 10.0;
const RECENT_ROW_VERTICAL_PADDING: f32 = 3.0;
const RECENT_ROW_LINE_GAP: f32 = 3.0;
const RECENT_ROW_CORNER_RADIUS: u8 = 5;
const RECENT_ROW_HOVER_FILL: Color32 = Color32::from_rgb(38, 41, 47);
const TOP_BAR_ANIMATION: f32 = 0.10;
const TOP_BAR_HIDE_DELAY: Duration = Duration::from_millis(80);
const TOP_BAR_MENU_HOLD_DELAY: Duration = Duration::from_secs(2);
const TOP_BAR_HOVER_ZONE_EXTRA: f32 = 8.0;
const TOP_BAR_MIN_INTERACTIVE_ALPHA: f32 = 0.35;
const TOP_BAR_SLIDE_DISTANCE: f32 = 10.0;

impl SuiSuiViewApp {
    pub(in crate::app) fn show_top_bar(&mut self, ctx: &egui::Context) {
        if self.settings.top_bar_pinned {
            self.show_pinned_top_bar(ctx);
            return;
        }

        self.show_overlay_top_bar(ctx);
    }

    fn show_pinned_top_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("focus_bar")
            .exact_height(theme::TOP_BAR_HEIGHT)
            .frame(top_bar_frame(1.0))
            .show(ctx, |ui| {
                self.show_top_bar_contents(ctx, ui);
            });
    }

    fn show_overlay_top_bar(&mut self, ctx: &egui::Context) {
        let target_visible = self.update_top_bar_visibility(ctx);
        let progress = ctx.animate_bool_with_time(
            egui::Id::new("top_bar_overlay_visible"),
            target_visible,
            TOP_BAR_ANIMATION,
        );
        if progress <= 0.001 {
            return;
        }
        if progress < 0.999 {
            ctx.request_repaint_after(Duration::from_millis(16));
        }

        let alpha = ease_out_cubic(progress);
        let y_offset = top_bar_slide_offset(alpha);
        let screen = ctx.screen_rect();
        egui::Area::new(egui::Id::new("focus_bar_overlay"))
            .fixed_pos(egui::pos2(screen.left(), screen.top() + y_offset))
            .order(egui::Order::Foreground)
            .interactable(top_bar_overlay_is_interactive(target_visible, alpha))
            .show(ctx, |ui| {
                ui.set_min_width(screen.width());
                ui.set_max_width(screen.width());
                top_bar_frame(alpha).show(ui, |ui| {
                    ui.set_min_height(theme::TOP_BAR_HEIGHT - 14.0);
                    ui.set_opacity(alpha);
                    self.show_top_bar_contents(ctx, ui);
                });
            });
    }

    pub(in crate::app) fn top_bar_is_visible(&self, ctx: &egui::Context) -> bool {
        if self.top_bar_is_forced_visible(ctx) {
            return true;
        }

        if pointer_is_near_top_bar(ctx) {
            return true;
        }

        self.top_bar_auto_hide_until
            .is_some_and(|hide_at| Instant::now() < hide_at)
    }

    fn update_top_bar_visibility(&mut self, ctx: &egui::Context) -> bool {
        if self.top_bar_is_forced_visible(ctx) {
            self.top_bar_auto_hide_until = None;
            return true;
        }

        if pointer_is_near_top_bar(ctx) {
            self.top_bar_auto_hide_until = Some(Instant::now() + TOP_BAR_HIDE_DELAY);
            return true;
        }

        let Some(hide_at) = self.top_bar_auto_hide_until else {
            return false;
        };
        let now = Instant::now();
        if now >= hide_at {
            self.top_bar_auto_hide_until = None;
            return false;
        }

        ctx.request_repaint_after(hide_at - now);
        true
    }

    fn top_bar_is_forced_visible(&self, ctx: &egui::Context) -> bool {
        self.settings.top_bar_pinned
            || self.bookmark_popover_open
            || self.settings_open
            || self.about_open
            || ctx.is_popup_open()
    }

    pub(in crate::app::ui) fn show_top_bar_pin_button(&mut self, ui: &mut egui::Ui) {
        let (icon, style, color) = if self.settings.top_bar_pinned {
            (
                icons::PIN_FILLED,
                icons::IconStyle::Filled,
                theme::ACCENT_HOVER,
            )
        } else {
            (icons::PIN, icons::IconStyle::Regular, theme::TEXT_PRIMARY)
        };
        let tooltip = if self.settings.top_bar_pinned {
            self.i18n().text("topbar.unpin")
        } else {
            self.i18n().text("topbar.pin")
        };
        if ui
            .add(icon_button_colored(icon, style, 18.0, color))
            .on_hover_text(tooltip)
            .clicked()
        {
            self.settings.top_bar_pinned = !self.settings.top_bar_pinned;
            self.store.update_settings(self.settings.clone());
        }
    }

    pub(in crate::app::ui) fn hold_top_bar_open_for_menu(&mut self) {
        self.top_bar_auto_hide_until = Some(Instant::now() + TOP_BAR_MENU_HOLD_DELAY);
    }

    pub(in crate::app::ui) fn show_open_group(&mut self, ui: &mut egui::Ui) {
        let i18n = self.i18n();
        ui.menu_button(
            icons::icon_text(icons::FOLDER_OPEN, &i18n.text("topbar.open")),
            |ui| {
                self.hold_top_bar_open_for_menu();
                let recent_books = if self.settings.remember_recent_locations {
                    self.store.recent_books(8)
                } else {
                    Vec::new()
                };
                let menu_width = recent_open_menu_width(ui, &recent_books);
                ui.set_min_width(menu_width);
                ui.set_max_width(menu_width);

                if ui
                    .button(icons::icon_text(
                        icons::DOCUMENT,
                        &i18n.text("topbar.open_file"),
                    ))
                    .clicked()
                {
                    self.open_file_dialog();
                    ui.close();
                }
                if ui
                    .button(icons::icon_text(
                        icons::FOLDER_OPEN,
                        &i18n.text("topbar.open_folder"),
                    ))
                    .clicked()
                {
                    self.open_folder_dialog();
                    ui.close();
                }

                ui.separator();
                ui.label(RichText::new(i18n.text("topbar.recent")).color(theme::TEXT_MUTED));
                if !self.settings.remember_recent_locations {
                    ui.add_enabled(false, egui::Label::new(i18n.text("topbar.recent_disabled")));
                    return;
                }
                if recent_books.is_empty() {
                    ui.add_enabled(false, egui::Label::new(i18n.text("topbar.no_recent")));
                    return;
                }

                for book in &recent_books {
                    if let Some(path) = book.known_paths.last() {
                        if recent_path_row(ui, path, menu_width)
                            .on_hover_text(path)
                            .clicked()
                        {
                            self.open_path(PathBuf::from(path));
                            ui.close();
                        }
                    } else {
                        ui.add_enabled(false, egui::Label::new(&book.title))
                            .on_hover_text(i18n.text("topbar.recent_missing"));
                    }
                }
            },
        );
    }

    pub(in crate::app::ui) fn show_page_group(&mut self, ui: &mut egui::Ui) {
        let i18n = self.i18n();
        let has_book = self.source.is_some();
        let previous = ui
            .add_enabled(
                has_book,
                icon_button(icons::CHEVRON_LEFT, icons::IconStyle::Regular, 22.0),
            )
            .on_hover_text(i18n.text("topbar.previous_page"));
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
            .on_hover_text(i18n.text("topbar.next_page"));
        if next.clicked() {
            self.next_page();
        }
    }

    pub(in crate::app::ui) fn show_view_group(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        let i18n = self.i18n();
        let label = format!(
            "{}: {}",
            i18n.text("topbar.view"),
            self.fit_mode.label_i18n(i18n)
        );
        ui.menu_button(icons::icon_text(icons::EYE, &label), |ui| {
            self.hold_top_bar_open_for_menu();
            ui.set_min_width(260.0);
            ui.label(i18n.text("topbar.layout"));
            ui.horizontal_wrapped(|ui| {
                for mode in [
                    ViewMode::Single,
                    ViewMode::DoubleLeftToRight,
                    ViewMode::DoubleRightToLeft,
                    ViewMode::SmartDoubleLeftToRight,
                    ViewMode::SmartDoubleRightToLeft,
                ] {
                    if ui
                        .selectable_label(self.view_mode == mode, view_mode_label(mode, i18n))
                        .clicked()
                    {
                        self.set_view_mode(mode);
                    }
                }
            });

            ui.separator();
            ui.label(i18n.text("topbar.fit"));
            ui.horizontal_wrapped(|ui| {
                for mode in FitMode::ALL {
                    if ui
                        .selectable_label(self.fit_mode == mode, mode.label_i18n(i18n))
                        .clicked()
                    {
                        self.set_fit_mode(mode);
                    }
                }
            });

            ui.separator();
            ui.horizontal(|ui| {
                ui.label(i18n.text("topbar.zoom"));
                if ui.button("-").clicked() {
                    self.adjust_zoom(0.9);
                }
                ui.label(format!("{:.0}%", self.manual_zoom * 100.0));
                if ui.button("+").clicked() {
                    self.adjust_zoom(1.1);
                }
            });
        });
        self.show_scale_group(ctx, ui);
    }

    pub(in crate::app::ui) fn show_correction_group(
        &mut self,
        ctx: &egui::Context,
        ui: &mut egui::Ui,
    ) {
        let i18n = self.i18n();
        ui.menu_button(
            icons::icon_text(icons::WAND, &i18n.text("topbar.correction")),
            |ui| {
                self.hold_top_bar_open_for_menu();
                ui.set_min_width(220.0);
                ui.label(i18n.text("topbar.transition"));
                let active_transition = self.settings.effective_page_transition_style();
                for style in PageTransitionStyle::ALL {
                    if ui
                        .selectable_label(active_transition == style, style.label_i18n(i18n))
                        .clicked()
                    {
                        let mut settings = self.settings.clone();
                        settings.set_page_transition_style(style);
                        self.apply_settings(ctx, settings);
                        ui.close();
                    }
                }

                ui.separator();
                ui.label(i18n.text("topbar.filter"));
                for filter in [
                    ImageFilter::None,
                    ImageFilter::Smooth,
                    ImageFilter::SmoothSharpen,
                    ImageFilter::RcasSharpen,
                ] {
                    if ui
                        .selectable_label(self.effects.filter == filter, filter.label_i18n(i18n))
                        .clicked()
                    {
                        self.apply_command(ctx, AppCommand::SetFilter(filter));
                    }
                }
            },
        );
    }

    pub(in crate::app::ui) fn show_debug_compare_group(&mut self, ui: &mut egui::Ui) {
        let has_book = self.source.is_some();
        let response = ui.add_enabled(
            has_book,
            egui::Button::new(self.i18n().text("topbar.compare"))
                .selected(self.debug_compare.enabled)
                .min_size(egui::vec2(52.0, 34.0)),
        );
        if response
            .on_hover_text(self.i18n().text("topbar.compare_tooltip"))
            .clicked()
        {
            self.set_debug_compare_enabled(!self.debug_compare.enabled);
        }

        if self.debug_compare.enabled {
            compare_target_combo(ui, "compare_left", "A", &mut self.debug_compare.left);
            compare_target_combo(ui, "compare_right", "B", &mut self.debug_compare.right);
        }
    }

    pub(in crate::app::ui) fn show_bookmark_group(
        &mut self,
        ctx: &egui::Context,
        ui: &mut egui::Ui,
    ) {
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
        if let Some(direction) = mode.reading_direction() {
            self.reading_direction = direction;
        }
        if self.source.is_some() {
            self.worker.set_page(
                self.worker_center_page(),
                self.last_nav_direction,
                self.target_long_edge,
                self.visible_page_count(),
                self.worker_options(),
            );
            self.persist_reading_position();
        }
    }
}

fn recent_open_menu_width(ui: &egui::Ui, recent_books: &[crate::core::state::BookRecord]) -> f32 {
    let viewport_width = ui.ctx().input(|input| {
        input
            .viewport()
            .inner_rect
            .map_or(OPEN_MENU_MIN_WIDTH, |rect| rect.width())
    });
    let font_id = egui::TextStyle::Button.resolve(ui.style());
    let longest_path_width = recent_books
        .iter()
        .filter_map(|book| book.known_paths.last())
        .map(|path| text_width(ui, path, &font_id))
        .fold(0.0_f32, f32::max);
    recent_menu_width_for(longest_path_width, viewport_width)
}

fn top_bar_frame(alpha: f32) -> Frame {
    Frame::new()
        .fill(theme::TOOLBAR_FILL.linear_multiply(alpha))
        .stroke(Stroke::new(
            1.0,
            theme::SUBTLE_STROKE.linear_multiply(alpha),
        ))
        .inner_margin(Margin::symmetric(14, 7))
}

fn ease_out_cubic(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}

fn top_bar_slide_offset(alpha: f32) -> f32 {
    -TOP_BAR_SLIDE_DISTANCE * (1.0 - alpha.clamp(0.0, 1.0))
}

fn top_bar_overlay_is_interactive(target_visible: bool, alpha: f32) -> bool {
    target_visible || alpha >= TOP_BAR_MIN_INTERACTIVE_ALPHA
}

fn top_bar_pointer_limit() -> f32 {
    theme::TOP_BAR_HEIGHT + TOP_BAR_HOVER_ZONE_EXTRA
}

fn pointer_is_near_top_bar(ctx: &egui::Context) -> bool {
    let limit = top_bar_pointer_limit();
    ctx.input(|input| input.pointer.hover_pos().is_some_and(|pos| pos.y <= limit))
}

fn recent_menu_width_for(longest_path_width: f32, viewport_width: f32) -> f32 {
    let max_width = (viewport_width * OPEN_MENU_MAX_VIEWPORT_FRACTION).max(OPEN_MENU_MIN_WIDTH);
    let desired_width =
        (longest_path_width + RECENT_ROW_HORIZONTAL_PADDING * 2.0 + OPEN_MENU_OUTER_MARGIN).ceil();

    desired_width.clamp(OPEN_MENU_MIN_WIDTH, max_width)
}

fn recent_path_row(ui: &mut egui::Ui, path: &str, menu_width: f32) -> egui::Response {
    let font_id = egui::TextStyle::Button.resolve(ui.style());
    let line_height = recent_row_line_height(font_id.size);
    let row_width = ui.available_width().max(menu_width);
    let text_width = (row_width - RECENT_ROW_HORIZONTAL_PADDING * 2.0).max(1.0);
    let lines = recent_path_lines(ui, path, text_width, &font_id);
    let row_height = recent_row_height(lines.len().max(1), font_id.size);
    let (rect, response) = ui.allocate_exact_size(Vec2::new(row_width, row_height), Sense::click());

    if ui.is_rect_visible(rect) {
        if response.hovered() {
            ui.painter().rect_filled(
                rect,
                egui::CornerRadius::same(RECENT_ROW_CORNER_RADIUS),
                RECENT_ROW_HOVER_FILL,
            );
        }

        let text_rect = rect.shrink2(Vec2::new(
            RECENT_ROW_HORIZONTAL_PADDING,
            RECENT_ROW_VERTICAL_PADDING,
        ));
        for (index, line) in lines.iter().enumerate() {
            let y = text_rect.top() + line_height * (index as f32 + 0.5);
            ui.painter().text(
                egui::pos2(text_rect.left(), y),
                Align2::LEFT_CENTER,
                line,
                font_id.clone(),
                theme::TEXT_PRIMARY,
            );
        }
    }

    response
}

fn recent_row_line_height(font_size: f32) -> f32 {
    font_size + RECENT_ROW_LINE_GAP
}

fn recent_row_height(line_count: usize, font_size: f32) -> f32 {
    recent_row_line_height(font_size) * line_count.max(1) as f32 + RECENT_ROW_VERTICAL_PADDING * 2.0
}

fn recent_path_lines(ui: &egui::Ui, path: &str, width: f32, font_id: &FontId) -> Vec<String> {
    let (lines, fits) = wrap_recent_path(ui, path, width, font_id, 2);
    if fits {
        return lines;
    }

    let char_count = path
        .chars()
        .count()
        .min(path_labels::RECENT_PATH_LABEL_CHARS);
    let mut best = 4_usize;
    let mut low = 4_usize;
    let mut high = char_count;
    while low <= high {
        let mid = low + (high - low) / 2;
        let candidate = path_labels::compact_start_for_two_lines(path, mid);
        let (_, candidate_fits) = wrap_recent_path(ui, &candidate, width, font_id, 2);
        if candidate_fits {
            best = mid;
            low = mid + 1;
        } else {
            high = mid.saturating_sub(1);
        }
    }

    let compact = path_labels::compact_start_for_two_lines(path, best);
    wrap_recent_path(ui, &compact, width, font_id, 2).0
}

fn wrap_recent_path(
    ui: &egui::Ui,
    text: &str,
    width: f32,
    font_id: &FontId,
    max_lines: usize,
) -> (Vec<String>, bool) {
    let mut lines = Vec::new();
    let mut rest = text.trim();
    while !rest.is_empty() && lines.len() < max_lines {
        let (line, next) = take_wrapped_line(ui, rest, width, font_id);
        if line.is_empty() {
            break;
        }
        lines.push(line.to_owned());
        rest = next.trim_start();
    }
    (lines, rest.is_empty())
}

fn take_wrapped_line<'a>(
    ui: &egui::Ui,
    text: &'a str,
    width: f32,
    font_id: &FontId,
) -> (&'a str, &'a str) {
    if text_width(ui, text, font_id) <= width {
        return (text.trim_end(), "");
    }

    let char_ends: Vec<usize> = text
        .char_indices()
        .map(|(index, ch)| index + ch.len_utf8())
        .collect();
    let last_fit = largest_fitting_prefix(ui, text, width, font_id, &char_ends);

    if last_fit == 0 {
        return ("", text);
    }

    let last_break = text[..last_fit]
        .char_indices()
        .filter_map(|(index, ch)| {
            matches!(ch, '\\' | '/' | ' ' | '|').then_some(index + ch.len_utf8())
        })
        .next_back();
    let end = last_break.unwrap_or(last_fit);

    (text[..end].trim_end(), &text[end..])
}

fn largest_fitting_prefix(
    ui: &egui::Ui,
    text: &str,
    width: f32,
    font_id: &FontId,
    char_ends: &[usize],
) -> usize {
    let mut best = 0;
    let mut low = 0;
    let mut high = char_ends.len();

    while low < high {
        let mid = low + (high - low) / 2;
        let end = char_ends[mid];
        if text_width(ui, &text[..end], font_id) <= width {
            best = end;
            low = mid + 1;
        } else {
            high = mid;
        }
    }

    best
}

fn text_width(ui: &egui::Ui, text: &str, font_id: &FontId) -> f32 {
    ui.painter()
        .layout_no_wrap(text.to_owned(), font_id.clone(), theme::TEXT_PRIMARY)
        .size()
        .x
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

pub(in crate::app::ui) fn toolbar_separator(ui: &mut egui::Ui) {
    ui.add_space(4.0);
    ui.separator();
    ui.add_space(4.0);
}

pub(in crate::app::ui) fn icon_button(
    code: char,
    style: icons::IconStyle,
    size: f32,
) -> Button<'static> {
    icon_button_colored(code, style, size, theme::TEXT_PRIMARY)
}

fn icon_button_colored(
    code: char,
    style: icons::IconStyle,
    size: f32,
    color: Color32,
) -> Button<'static> {
    Button::new(icons::icon(code, style, size, color))
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

fn view_mode_label(mode: ViewMode, i18n: I18n) -> String {
    match mode {
        ViewMode::Single => i18n.text("label.view.single"),
        ViewMode::DoubleLeftToRight => i18n.text("label.view.double_ltr"),
        ViewMode::DoubleRightToLeft => i18n.text("label.view.double_rtl"),
        ViewMode::SmartDoubleLeftToRight => i18n.text("label.view.smart_ltr"),
        ViewMode::SmartDoubleRightToLeft => i18n.text("label.view.smart_rtl"),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ease_out_cubic, recent_menu_width_for, recent_row_height, top_bar_overlay_is_interactive,
        top_bar_pointer_limit, top_bar_slide_offset, OPEN_MENU_MIN_WIDTH, TOP_BAR_HOVER_ZONE_EXTRA,
        TOP_BAR_MIN_INTERACTIVE_ALPHA, TOP_BAR_SLIDE_DISTANCE,
    };
    use crate::app::ui::theme;

    #[test]
    fn recent_menu_width_uses_minimum_for_short_paths() {
        assert_eq!(recent_menu_width_for(180.0, 1600.0), OPEN_MENU_MIN_WIDTH);
    }

    #[test]
    fn recent_menu_width_can_grow_to_viewport_cap() {
        assert_eq!(recent_menu_width_for(3000.0, 1600.0), 1280.0);
    }

    #[test]
    fn recent_menu_width_uses_full_path_width_before_cap() {
        assert_eq!(recent_menu_width_for(700.0, 1600.0), 744.0);
    }

    #[test]
    fn recent_row_height_uses_single_line_when_possible() {
        assert_eq!(recent_row_height(1, 18.0), 27.0);
        assert_eq!(recent_row_height(2, 18.0), 48.0);
    }

    #[test]
    fn top_bar_uses_same_pointer_zone_for_reveal_and_hide() {
        assert_eq!(
            top_bar_pointer_limit(),
            theme::TOP_BAR_HEIGHT + TOP_BAR_HOVER_ZONE_EXTRA
        );
    }

    #[test]
    fn top_bar_overlay_slides_from_above() {
        assert_eq!(top_bar_slide_offset(0.0), -TOP_BAR_SLIDE_DISTANCE);
        assert_eq!(top_bar_slide_offset(1.0), 0.0);
        assert!(top_bar_slide_offset(0.5) < 0.0);
    }

    #[test]
    fn top_bar_overlay_uses_ease_out_curve() {
        assert_eq!(ease_out_cubic(0.0), 0.0);
        assert_eq!(ease_out_cubic(1.0), 1.0);
        assert!(ease_out_cubic(0.5) > 0.5);
    }

    #[test]
    fn top_bar_overlay_stops_intercepting_clicks_while_fading_out() {
        assert!(top_bar_overlay_is_interactive(true, 0.0));
        assert!(top_bar_overlay_is_interactive(
            false,
            TOP_BAR_MIN_INTERACTIVE_ALPHA
        ));
        assert!(!top_bar_overlay_is_interactive(
            false,
            TOP_BAR_MIN_INTERACTIVE_ALPHA - 0.01
        ));
    }
}
