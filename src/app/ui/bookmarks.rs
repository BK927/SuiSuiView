use super::super::{BookmarkMarqueeKey, BookmarkMarqueeState, SuiSuiViewApp};
use super::bookmark_rows::{BookmarkFilter, BookmarkRow};
use super::bookmark_thumbnails::{thumbnail_tint_for_state, BookmarkThumbnailState};
use super::text_fit::{compact_end_to_width, compact_start_to_width};
use super::{dialog, icons, theme};
use crate::core::state::PageBookmarkEntry;
use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Frame, Margin, Rect, RichText, Sense, Stroke,
    StrokeKind,
};
use std::time::Duration;

const POPOVER_WIDTH: f32 = 430.0;
const POPOVER_HEIGHT: f32 = 386.0;
const THUMBNAIL_SIZE: egui::Vec2 = egui::vec2(64.0, 54.0);
const BOOKMARK_ROW_HEIGHT: f32 = 82.0;
const BOOKMARK_ROWS_MAX_HEIGHT: f32 = 154.0;
const ROW_ACTION_WIDTH: f32 = 48.0;
const MARQUEE_SPEED: f32 = 82.0;
const MARQUEE_MIN_SCROLL_SECONDS: f32 = 1.2;
const MARQUEE_END_PAUSE_SECONDS: f32 = 0.7;

impl SuiSuiViewApp {
    pub(in crate::app) fn show_bookmark_popover(&mut self, ctx: &egui::Context) {
        if !self.bookmark_popover_open {
            return;
        }
        if self.settings_open || self.about_open {
            self.close_bookmark_popover();
            return;
        }

        let screen = ctx.screen_rect();
        let width = POPOVER_WIDTH;
        let height = POPOVER_HEIGHT;
        let pos = egui::pos2(
            self.bookmark_popover_pos
                .x
                .clamp(screen.left() + 8.0, screen.right() - width - 8.0),
            self.bookmark_popover_pos
                .y
                .clamp(screen.top() + 8.0, screen.bottom() - height - 8.0),
        );

        let area_response = egui::Area::new(egui::Id::new("bookmark_popover"))
            .fixed_pos(pos)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(width, height),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        let frame = Frame::new()
                            .fill(theme::POPOVER_FILL)
                            .stroke(theme::subtle_stroke())
                            .corner_radius(CornerRadius::same(7))
                            .inner_margin(Margin::same(14));
                        dialog::show_sized_frame(
                            ui,
                            egui::vec2(width, POPOVER_HEIGHT),
                            frame,
                            |ui| {
                                self.show_bookmark_popover_contents(ui);
                            },
                        );
                    },
                );
            });
        self.close_bookmark_popover_on_outside_click(ctx, area_response.response.rect);
    }

    pub(in crate::app) fn toggle_bookmark_popover_below(&mut self, anchor: Rect) {
        self.bookmark_popover_open = !self.bookmark_popover_open;
        if self.bookmark_popover_open {
            self.bookmark_popover_pos =
                egui::pos2(anchor.right() - POPOVER_WIDTH, anchor.bottom() + 8.0);
            self.bookmark_popover_anchor = Some(anchor);
        } else {
            self.close_bookmark_popover();
        }
    }

    pub(in crate::app) fn toggle_bookmark_popover(&mut self, ctx: &egui::Context) {
        self.bookmark_popover_open = !self.bookmark_popover_open;
        if self.bookmark_popover_open {
            let screen = ctx.screen_rect();
            self.bookmark_popover_pos =
                egui::pos2(screen.right() - POPOVER_WIDTH - 28.0, screen.top() + 54.0);
            self.bookmark_popover_anchor = None;
        } else {
            self.close_bookmark_popover();
        }
    }

    pub(in crate::app) fn close_bookmark_popover(&mut self) {
        self.bookmark_popover_open = false;
        self.bookmark_clear_confirming = false;
        self.bookmark_popover_anchor = None;
        self.bookmark_marquee.clear();
    }

    fn close_bookmark_popover_on_outside_click(&mut self, ctx: &egui::Context, popover_rect: Rect) {
        let Some(pointer_pos) = ctx.input(|input| input.pointer.press_origin()) else {
            return;
        };
        if !ctx.input(|input| input.pointer.any_pressed()) {
            return;
        }
        let clicked_anchor = self
            .bookmark_popover_anchor
            .is_some_and(|rect| rect.contains(pointer_pos));
        if !popover_rect.contains(pointer_pos) && !clicked_anchor {
            self.close_bookmark_popover();
        }
    }

    pub(in crate::app) fn current_page_is_bookmarked(&self) -> bool {
        let Some(book_id) = self.book_id.as_deref() else {
            return false;
        };
        self.store.has_page_bookmark(book_id, self.current_page)
    }

    pub(in crate::app) fn toggle_current_page_bookmark(&mut self) {
        self.bookmark_clear_confirming = false;
        let Some(book_id) = self.book_id.clone() else {
            self.set_status("북마크할 책이 열려 있지 않습니다.");
            return;
        };
        let page = self.current_page;
        if self.store.has_page_bookmark(&book_id, page) {
            self.store.remove_page_bookmark(&book_id, page);
            self.bookmark_rows.clear();
            self.set_status(format!("p.{} 북마크를 삭제했습니다.", page + 1));
            return;
        }

        self.persist_current_bookmark();
        let title = self.default_page_bookmark_title(page);
        let page_name = self
            .source
            .as_ref()
            .and_then(|source| source.page_name(page))
            .map(str::to_owned);
        self.store
            .upsert_page_bookmark(&book_id, page, title, page_name);
        self.bookmark_rows.clear();
        self.set_status(format!("p.{} 북마크를 추가했습니다.", page + 1));
    }

    fn show_bookmark_popover_contents(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading(RichText::new("북마크").color(theme::TEXT_PRIMARY));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(small_icon_button(
                        icons::DISMISS,
                        icons::IconStyle::Regular,
                        theme::TEXT_PRIMARY,
                    ))
                    .on_hover_text("닫기")
                    .clicked()
                {
                    self.close_bookmark_popover();
                }
            });
        });

        ui.add_space(14.0);
        ui.horizontal(|ui| {
            let bookmark_count = self.bookmark_delete_scope_count();
            let search_width = (ui.available_width() - 122.0).max(168.0);
            bookmark_search_box(ui, &mut self.bookmark_search, search_width);
            self.show_bookmark_clear_button(ui, bookmark_count);
        });

        ui.add_space(12.0);
        ui.separator();
        ui.add_space(4.0);
        self.show_bookmark_filter_tabs(ui);
        ui.add_space(12.0);
        self.show_bookmark_rows(ui);
    }

    fn show_bookmark_clear_button(&mut self, ui: &mut egui::Ui, bookmark_count: usize) {
        if bookmark_count == 0 {
            self.bookmark_clear_confirming = false;
        }
        let label = if self.bookmark_clear_confirming {
            "삭제 확인"
        } else if self.bookmark_filter == BookmarkFilter::ThisBook {
            "이 책 삭제"
        } else {
            "전체 삭제"
        };
        let fill = if self.bookmark_clear_confirming {
            theme::ACCENT
        } else {
            Color32::from_rgb(38, 41, 47)
        };
        let stroke = if self.bookmark_clear_confirming {
            Stroke::new(1.0, theme::ACCENT_HOVER)
        } else {
            Stroke::new(1.0, Color32::from_rgb(58, 64, 73))
        };
        if ui
            .add_enabled(
                bookmark_count > 0,
                egui::Button::new(icons::icon_text(icons::DELETE, label))
                    .min_size(egui::vec2(108.0, 40.0))
                    .fill(fill)
                    .stroke(stroke),
            )
            .on_hover_text(if self.bookmark_filter == BookmarkFilter::ThisBook {
                "이 책의 모든 북마크 삭제"
            } else {
                "모든 북마크 삭제"
            })
            .clicked()
        {
            if self.bookmark_clear_confirming {
                self.clear_current_tab_bookmarks();
            } else {
                self.bookmark_clear_confirming = true;
            }
        }
    }

    fn show_bookmark_filter_tabs(&mut self, ui: &mut egui::Ui) {
        let width = ui.available_width();
        let tab_height = 44.0;
        let (rect, _) = ui.allocate_exact_size(egui::vec2(width, tab_height), Sense::hover());
        let painter = ui.painter_at(rect);
        let filters = [BookmarkFilter::All, BookmarkFilter::ThisBook];
        let tab_width = rect.width() / filters.len() as f32;

        painter.line_segment(
            [
                egui::pos2(rect.left(), rect.bottom() - 1.0),
                egui::pos2(rect.right(), rect.bottom() - 1.0),
            ],
            Stroke::new(1.0, Color32::from_rgb(45, 50, 57)),
        );

        for (index, filter) in filters.into_iter().enumerate() {
            let tab_rect = Rect::from_min_size(
                egui::pos2(rect.left() + tab_width * index as f32, rect.top()),
                egui::vec2(tab_width, tab_height),
            );
            let id = ui.make_persistent_id(("bookmark_filter_tab", filter));
            let response = ui.interact(tab_rect, id, Sense::click());
            let selected = self.bookmark_filter == filter;
            let accent = if selected {
                theme::ACCENT_HOVER
            } else {
                theme::TEXT_MUTED
            };
            let text_color = if selected {
                Color32::from_rgb(244, 83, 127)
            } else {
                theme::TEXT_MUTED
            };
            if response.hovered() && !selected {
                painter.rect_filled(
                    tab_rect.shrink2(egui::vec2(8.0, 7.0)),
                    CornerRadius::same(7),
                    Color32::from_rgb(31, 35, 40),
                );
            }
            draw_filter_tab_label(&painter, tab_rect, filter, accent, text_color);
            if selected {
                let underline = Rect::from_min_max(
                    egui::pos2(tab_rect.left() + 18.0, rect.bottom() - 3.0),
                    egui::pos2(tab_rect.right() - 10.0, rect.bottom()),
                );
                painter.rect_filled(underline, CornerRadius::same(3), theme::ACCENT_HOVER);
            }
            if response.clicked() {
                self.bookmark_filter = filter;
                self.bookmark_clear_confirming = false;
            }
        }
    }

    fn show_bookmark_rows(&mut self, ui: &mut egui::Ui) {
        let rows_height = ui.available_height().clamp(96.0, BOOKMARK_ROWS_MAX_HEIGHT);
        if self.bookmark_filter == BookmarkFilter::ThisBook && self.book_id.is_none() {
            empty_bookmark_message(ui, "책을 열면 이 책 북마크가 표시됩니다.", rows_height);
            return;
        }

        self.refresh_bookmark_rows_if_needed();
        if self.bookmark_rows.len() == 0 {
            empty_bookmark_message(ui, "저장된 북마크가 없습니다.", rows_height);
            return;
        }

        egui::ScrollArea::vertical()
            .max_height(rows_height)
            .show_rows(
                ui,
                BOOKMARK_ROW_HEIGHT,
                self.bookmark_rows.len(),
                |ui, row_range| {
                    for index in row_range {
                        if let Some(row) = self.bookmark_rows.row(index) {
                            self.show_bookmark_row(ui, row);
                        }
                    }
                },
            );
    }

    fn show_bookmark_row(&mut self, ui: &mut egui::Ui, row: BookmarkRow) {
        let current_book = self.book_id.as_deref() == Some(row.book_id.as_str());
        let current = current_book && row.bookmark.page == self.current_page;
        let frame = Frame::new()
            .fill(if current {
                Color32::from_rgb(31, 35, 39)
            } else {
                theme::ROW_FILL
            })
            .stroke(Stroke::new(1.0, Color32::from_rgb(49, 53, 60)))
            .corner_radius(CornerRadius::same(6))
            .inner_margin(Margin::symmetric(7, 6));

        let mut jump_to_page = false;
        let mut remove_bookmark = false;
        let mut title_rect = Rect::NOTHING;
        let mut title_hovered = false;

        let frame_response = frame.show(ui, |ui| {
            ui.set_min_height(66.0);
            ui.horizontal(|ui| {
                let thumbnail = if current_book {
                    self.bookmark_thumbnail_state(&row.book_id, row.bookmark.page)
                } else {
                    BookmarkThumbnailState::Loading
                };
                if show_bookmark_thumbnail(ui, thumbnail).clicked() {
                    jump_to_page = true;
                }

                let title_width = (ui.available_width() - ROW_ACTION_WIDTH).max(48.0);
                let title_response = allocate_bookmark_title(ui, &row, title_width);
                title_rect = title_response.rect;
                title_hovered = title_response.hovered();
                if title_response.clicked() {
                    jump_to_page = true;
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add(small_icon_button(
                            icons::BOOKMARK_FILLED,
                            icons::IconStyle::Filled,
                            theme::ACCENT,
                        ))
                        .on_hover_text("북마크 삭제")
                        .clicked()
                    {
                        remove_bookmark = true;
                    }
                });
            });
        });
        let mut jump_rect = frame_response.response.rect;
        jump_rect.set_right((jump_rect.right() - 62.0).max(jump_rect.left()));
        let row_response = ui
            .interact(
                jump_rect,
                ui.make_persistent_id(("bookmark_row", &row.book_id, row.bookmark.page)),
                Sense::click(),
            )
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        if row_response.hovered() {
            let painter = ui.painter_at(frame_response.response.rect);
            painter.rect_filled(
                frame_response.response.rect,
                CornerRadius::same(6),
                Color32::from_rgba_unmultiplied(255, 255, 255, 8),
            );
            painter.rect_stroke(
                frame_response.response.rect,
                CornerRadius::same(6),
                Stroke::new(1.0, Color32::from_rgb(64, 70, 80)),
                StrokeKind::Inside,
            );
        }
        let marquee_started_at = if row_response.hovered() || title_hovered {
            let now = ui.input(|input| input.time);
            Some(
                self.bookmark_marquee
                    .started_at_for(&row.book_id, row.bookmark.page, now),
            )
        } else {
            self.bookmark_marquee
                .clear_if_matches(&row.book_id, row.bookmark.page);
            None
        };
        paint_bookmark_title(ui, title_rect, &row, marquee_started_at);
        if row_response.clicked() {
            jump_to_page = true;
        }

        if remove_bookmark {
            self.bookmark_clear_confirming = false;
            self.store
                .remove_page_bookmark(&row.book_id, row.bookmark.page);
            self.bookmark_rows.clear();
        } else if jump_to_page {
            self.bookmark_clear_confirming = false;
            self.jump_to_bookmark(row);
        }
    }

    fn clear_current_tab_bookmarks(&mut self) {
        let removed = match self.bookmark_filter {
            BookmarkFilter::All => self.store.clear_all_page_bookmarks(),
            BookmarkFilter::ThisBook => {
                let Some(book_id) = self.book_id.clone() else {
                    self.bookmark_clear_confirming = false;
                    self.set_status("삭제할 북마크가 없습니다.");
                    return;
                };
                self.store.clear_page_bookmarks(&book_id)
            }
        };
        self.bookmark_clear_confirming = false;
        if removed == 0 {
            self.set_status("삭제할 북마크가 없습니다.");
        } else {
            self.bookmark_thumbnails.clear();
            let scope = if self.bookmark_filter == BookmarkFilter::ThisBook {
                "이 책의"
            } else {
                "전체"
            };
            self.set_status(format!("{scope} 북마크 {removed}개를 삭제했습니다."));
            self.bookmark_rows.clear();
        }
    }

    fn bookmark_delete_scope_count(&self) -> usize {
        match self.bookmark_filter {
            BookmarkFilter::All => self.store.all_page_bookmark_count(),
            BookmarkFilter::ThisBook => self
                .book_id
                .as_deref()
                .map(|book_id| self.store.page_bookmarks(book_id).len())
                .unwrap_or_default(),
        }
    }

    fn bookmark_entries_for_active_filter(&self) -> Vec<PageBookmarkEntry> {
        match self.bookmark_filter {
            BookmarkFilter::All => self.store.all_page_bookmarks(),
            BookmarkFilter::ThisBook => self
                .book_id
                .as_deref()
                .map(|book_id| self.store.page_bookmark_entries(book_id))
                .unwrap_or_default(),
        }
    }

    fn refresh_bookmark_rows_if_needed(&mut self) {
        let filter = self.bookmark_filter;
        let book_id = self.book_id.clone();
        let query = self.bookmark_search.clone();
        if self
            .bookmark_rows
            .needs_refresh(filter, book_id.as_deref(), &query)
        {
            let entries = self.bookmark_entries_for_active_filter();
            self.bookmark_rows
                .refresh(filter, book_id.as_deref(), &query, entries);
        }
    }

    fn bookmark_thumbnail_state(&mut self, book_id: &str, page: usize) -> BookmarkThumbnailState {
        let Some(source) = self.source.clone() else {
            return BookmarkThumbnailState::Failed;
        };
        let decode = self.decode_options();
        self.bookmark_thumbnails
            .thumbnail(source, book_id, page, decode)
    }

    fn jump_to_bookmark(&mut self, row: BookmarkRow) {
        if self.book_id.as_deref() == Some(row.book_id.as_str()) {
            let direction = if row.bookmark.page >= self.current_page {
                super::super::NavigationDirection::Forward
            } else {
                super::super::NavigationDirection::Backward
            };
            self.set_page(row.bookmark.page, direction);
        } else if let Some(path) = row.known_path {
            self.open_path_for_bookmark(
                std::path::PathBuf::from(path),
                row.book_id,
                row.bookmark.page,
            );
        } else {
            self.set_status("북마크 경로를 찾을 수 없습니다.");
        }
        self.close_bookmark_popover();
    }

    fn default_page_bookmark_title(&self, page: usize) -> String {
        let page_name = self
            .source
            .as_ref()
            .and_then(|source| source.page_name(page))
            .unwrap_or_default();
        if page_name.is_empty() {
            format!("p.{:03}", page + 1)
        } else {
            page_name.to_owned()
        }
    }
}

fn bookmark_search_box(ui: &mut egui::Ui, search: &mut String, width: f32) {
    Frame::new()
        .fill(theme::INPUT_FILL)
        .stroke(Stroke::new(1.0, Color32::from_rgb(58, 64, 73)))
        .corner_radius(CornerRadius::same(7))
        .inner_margin(Margin::symmetric(12, 4))
        .show(ui, |ui| {
            ui.set_min_size(egui::vec2(width, 32.0));
            ui.horizontal_centered(|ui| {
                ui.label(icons::icon(
                    icons::SEARCH,
                    icons::IconStyle::Regular,
                    20.0,
                    theme::TEXT_MUTED,
                ));
                ui.add(
                    egui::TextEdit::singleline(search)
                        .hint_text("검색")
                        .desired_width(width - 36.0)
                        .frame(false),
                );
            });
        });
}

fn draw_filter_tab_label(
    painter: &egui::Painter,
    rect: Rect,
    filter: BookmarkFilter,
    icon_color: Color32,
    text_color: Color32,
) {
    let center = rect.center();
    let icon_pos = egui::pos2(center.x, center.y - 8.0);
    let text_pos = egui::pos2(center.x, center.y + 12.0);
    match filter {
        BookmarkFilter::All => {
            painter.text(
                icon_pos,
                Align2::CENTER_CENTER,
                icons::BOOKMARK.to_string(),
                icons::icon_font(icons::IconStyle::Regular, 20.0),
                icon_color,
            );
        }
        BookmarkFilter::ThisBook => {
            painter.text(
                icon_pos,
                Align2::CENTER_CENTER,
                icons::DOCUMENT.to_string(),
                icons::icon_font(icons::IconStyle::Regular, 20.0),
                icon_color,
            );
        }
    }
    painter.text(
        text_pos,
        Align2::CENTER_CENTER,
        filter.label(),
        FontId::proportional(14.0),
        text_color,
    );
}

fn show_bookmark_thumbnail(ui: &mut egui::Ui, thumbnail: BookmarkThumbnailState) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(THUMBNAIL_SIZE, Sense::click());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::same(4), theme::INPUT_FILL);
    painter.rect_stroke(
        rect,
        CornerRadius::same(4),
        Stroke::new(1.0, Color32::from_rgb(62, 66, 74)),
        StrokeKind::Inside,
    );

    match thumbnail {
        BookmarkThumbnailState::Ready {
            texture,
            original_size,
        } => {
            let image_rect = fit_rect(rect.shrink(2.0), original_size);
            painter.image(
                texture.id(),
                image_rect,
                Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        }
        state => {
            let tint = thumbnail_tint_for_state(&state);
            let icon = match state {
                BookmarkThumbnailState::Loading => icons::DOCUMENT,
                BookmarkThumbnailState::Failed => icons::DISMISS,
                BookmarkThumbnailState::Ready { .. } => unreachable!(),
            };
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                icon.to_string(),
                icons::icon_font(icons::IconStyle::Regular, 20.0),
                tint,
            );
        }
    }

    response
}

fn allocate_bookmark_title(ui: &mut egui::Ui, row: &BookmarkRow, width: f32) -> egui::Response {
    let (_, response) = ui.allocate_exact_size(egui::vec2(width, 54.0), Sense::click());
    response.on_hover_text(&row.display_name)
}

fn paint_bookmark_title(
    ui: &egui::Ui,
    rect: Rect,
    row: &BookmarkRow,
    marquee_started_at: Option<f64>,
) {
    if !rect.is_positive() {
        return;
    }
    let painter = ui.painter_at(rect);
    let title_font = FontId::proportional(15.0);
    let context_font = FontId::proportional(12.5);
    let text_width = (rect.width() - 12.0).max(0.0);
    let title_pos = egui::pos2(rect.left() + 6.0, rect.center().y - 9.0);
    let context_pos = egui::pos2(rect.left() + 6.0, rect.center().y + 11.0);
    if let Some(started_at) = marquee_started_at {
        let mut repaint_after = draw_marquee_text(
            ui,
            rect,
            title_pos,
            &row.title,
            title_font,
            theme::TEXT_PRIMARY,
            text_width,
            started_at,
        );
        if let Some(context_repaint_after) = draw_marquee_text(
            ui,
            rect,
            context_pos,
            &row.context,
            context_font,
            theme::TEXT_MUTED,
            text_width,
            started_at,
        ) {
            repaint_after = Some(repaint_after.map_or(context_repaint_after, |delay| {
                delay.min(context_repaint_after)
            }));
        }
        if let Some(repaint_after) = repaint_after {
            ui.ctx().request_repaint_after(repaint_after);
        }
        return;
    }

    let title = compact_end_to_width(
        ui,
        &row.title,
        title_font.clone(),
        theme::TEXT_PRIMARY,
        text_width,
    );
    let context = compact_start_to_width(
        ui,
        &row.context,
        context_font.clone(),
        theme::TEXT_MUTED,
        text_width,
    );
    painter.text(
        title_pos,
        Align2::LEFT_CENTER,
        title,
        title_font,
        theme::TEXT_PRIMARY,
    );
    if !context.is_empty() {
        painter.text(
            context_pos,
            Align2::LEFT_CENTER,
            context,
            context_font,
            theme::TEXT_MUTED,
        );
    }
}

fn draw_marquee_text(
    ui: &egui::Ui,
    clip_rect: Rect,
    pos: egui::Pos2,
    text: &str,
    font_id: FontId,
    color: Color32,
    width: f32,
    started_at: f64,
) -> Option<Duration> {
    if text.is_empty() {
        return None;
    }
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_owned(), font_id.clone(), color);
    let overflow = (galley.size().x - width).max(0.0);
    if overflow <= 0.0 {
        ui.painter()
            .text(pos, Align2::LEFT_CENTER, text, font_id, color);
        return None;
    }

    let scroll_seconds = (overflow / MARQUEE_SPEED).max(MARQUEE_MIN_SCROLL_SECONDS);
    let cycle = scroll_seconds + MARQUEE_END_PAUSE_SECONDS;
    let elapsed = ((ui.input(|input| input.time) - started_at) as f32).max(0.0) % cycle;
    let scrolling = elapsed < scroll_seconds;
    let offset = if scrolling {
        overflow * (elapsed / scroll_seconds)
    } else {
        overflow
    };
    let text_clip = Rect::from_min_max(
        egui::pos2(pos.x, clip_rect.top()),
        egui::pos2(pos.x + width, clip_rect.bottom()),
    );
    ui.painter().with_clip_rect(text_clip).galley(
        egui::pos2(pos.x - offset, pos.y - galley.size().y * 0.5),
        galley,
        color,
    );
    if scrolling {
        Some(Duration::from_millis(16))
    } else {
        Some(Duration::from_secs_f32((cycle - elapsed).max(0.016)))
    }
}

impl BookmarkMarqueeState {
    fn started_at_for(&mut self, book_id: &str, page: usize, now: f64) -> f64 {
        let key = BookmarkMarqueeKey {
            book_id: book_id.to_owned(),
            page,
        };
        if self.key.as_ref() != Some(&key) {
            self.key = Some(key);
            self.started_at = now;
        }
        self.started_at
    }

    fn clear_if_matches(&mut self, book_id: &str, page: usize) {
        let matches_current = self
            .key
            .as_ref()
            .is_some_and(|key| key.book_id == book_id && key.page == page);
        if matches_current {
            self.clear();
        }
    }

    fn clear(&mut self) {
        self.key = None;
        self.started_at = 0.0;
    }
}

fn fit_rect(rect: Rect, original_size: egui::Vec2) -> Rect {
    if original_size.x <= 0.0 || original_size.y <= 0.0 {
        return rect;
    }
    let scale = (rect.width() / original_size.x).min(rect.height() / original_size.y);
    let size = original_size * scale;
    Rect::from_center_size(rect.center(), size)
}

fn small_icon_button(code: char, style: icons::IconStyle, color: Color32) -> egui::Button<'static> {
    egui::Button::new(icons::icon(code, style, 18.0, color))
        .min_size(egui::vec2(28.0, 28.0))
        .fill(Color32::TRANSPARENT)
}

fn empty_bookmark_message(ui: &mut egui::Ui, text: &str, max_height: f32) {
    let width = ui.available_width();
    let height = max_height.clamp(104.0, 184.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::same(7), Color32::from_rgb(25, 28, 32));
    painter.rect_stroke(
        rect,
        CornerRadius::same(7),
        Stroke::new(1.0, Color32::from_rgb(41, 46, 54)),
        StrokeKind::Inside,
    );

    let icon_center = egui::pos2(rect.center().x, rect.center().y - 20.0);
    painter.circle_stroke(icon_center, 32.0, Stroke::new(1.6, theme::ACCENT));
    painter.text(
        icon_center,
        Align2::CENTER_CENTER,
        icons::BOOKMARK.to_string(),
        icons::icon_font(icons::IconStyle::Regular, 38.0),
        theme::ACCENT,
    );
    painter.text(
        egui::pos2(rect.center().x, icon_center.y + 62.0),
        Align2::CENTER_CENTER,
        text,
        FontId::proportional(16.5),
        theme::TEXT_MUTED,
    );
}
