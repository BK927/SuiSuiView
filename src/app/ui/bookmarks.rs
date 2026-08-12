use super::super::SuiSuiViewApp;
use super::bookmark_rows::{BookmarkFilter, BookmarkRow};
use super::bookmark_text::{allocate_bookmark_title, paint_bookmark_title};
use super::bookmark_thumbnails::{thumbnail_tint_for_state, BookmarkThumbnailState};
use super::{dialog, icons, theme};
use crate::core::i18n::I18n;
use crate::core::state::PageBookmarkEntry;
use egui::{
    self, Align2, Color32, CornerRadius, FontId, Frame, Margin, Rect, RichText, Sense, Stroke,
    StrokeKind,
};

const POPOVER_WIDTH: f32 = 430.0;
const THUMBNAIL_SIZE: egui::Vec2 = egui::vec2(64.0, 54.0);
const BOOKMARK_ROW_HEIGHT: f32 = 82.0;
/// Title, search row, separators and the filter tabs. Fixed, so the popover
/// grows only by its list.
const POPOVER_CHROME_HEIGHT: f32 = 232.0;
/// The height the list always had. Kept as the floor so a short list looks
/// exactly as it did before; only longer lists change.
const BOOKMARK_ROWS_MIN_HEIGHT: f32 = 154.0;
const POPOVER_SCREEN_MARGIN: f32 = 8.0;
const ROW_ACTION_WIDTH: f32 = 48.0;

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
        let rows_height = self.bookmark_rows_height(ctx);
        let height = POPOVER_CHROME_HEIGHT + rows_height;
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
                        dialog::show_sized_frame(ui, egui::vec2(width, height), frame, |ui| {
                            self.show_bookmark_popover_contents(ui, rows_height);
                        });
                    },
                );
            });
        let popover_rect = area_response.response.rect;
        self.show_bookmark_delete_dialog(ctx, popover_rect);
        self.close_bookmark_popover_on_outside_click(ctx, popover_rect);
    }

    /// How tall the bookmark list wants to be: enough for every row, capped by
    /// what the screen leaves after the fixed chrome. The list used to be pinned
    /// at a height that showed under two rows no matter how many bookmarks
    /// existed, which made a long list read as if it had been truncated.
    fn bookmark_rows_height(&mut self, ctx: &egui::Context) -> f32 {
        let room = ctx.screen_rect().height() - POPOVER_CHROME_HEIGHT - POPOVER_SCREEN_MARGIN * 2.0;
        let max_height = room.max(BOOKMARK_ROWS_MIN_HEIGHT);
        if self.bookmark_filter == BookmarkFilter::ThisBook && self.book_id.is_none() {
            return BOOKMARK_ROWS_MIN_HEIGHT;
        }
        self.refresh_bookmark_rows_if_needed();
        let rows = self.bookmark_rows.len() as f32;
        if rows == 0.0 {
            return BOOKMARK_ROWS_MIN_HEIGHT;
        }
        // `ScrollArea::show_rows` pitches rows by height + item spacing, with no
        // spacing after the last one. Match it, or the list is always one
        // scrollbar short of showing what it has room for.
        let spacing = ctx.style().spacing.item_spacing.y;
        let wanted = rows * (BOOKMARK_ROW_HEIGHT + spacing) - spacing;
        wanted.clamp(BOOKMARK_ROWS_MIN_HEIGHT, max_height)
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
        self.bookmark_delete_dialog = None;
        self.bookmark_popover_anchor = None;
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
        let Some(source_path) = self.current_bookmark_source_path() else {
            return false;
        };
        self.store
            .has_page_bookmark(book_id, &source_path, self.current_page)
    }

    pub(in crate::app) fn toggle_current_page_bookmark(&mut self) {
        self.bookmark_delete_dialog = None;
        let i18n = self.i18n();
        let Some(book_id) = self.book_id.clone() else {
            self.notify(i18n.text("bookmark.no_book"));
            return;
        };
        let Some(source_path) = self.current_bookmark_source_path() else {
            self.notify(i18n.text("bookmark.no_path"));
            return;
        };
        let page = self.current_page;
        if self.store.has_page_bookmark(&book_id, &source_path, page) {
            self.store
                .remove_page_bookmark(&book_id, &source_path, page);
            self.bookmark_rows.clear();
            self.notify(i18n.with_vars("bookmark.removed", &[("page", (page + 1).to_string())]));
            return;
        }

        self.write_current_book_record();
        let title = self.default_page_bookmark_title(page);
        let page_name = self
            .source
            .as_ref()
            .and_then(|source| source.page_name(page))
            .map(str::to_owned);
        let prewarm_source = self.source.clone();
        let prewarm_path = source_path.to_string_lossy().to_string();
        let decode = self.decode_options();
        self.store
            .upsert_page_bookmark(&book_id, &source_path, page, title, page_name.clone());
        self.ensure_bookmark_thumbnails().prewarm(
            prewarm_source,
            &book_id,
            Some(prewarm_path.as_str()),
            page,
            page_name.as_deref(),
            decode,
        );
        self.bookmark_rows.clear();
        self.notify(i18n.with_vars("bookmark.added", &[("page", (page + 1).to_string())]));
    }

    fn current_bookmark_source_path(&self) -> Option<std::path::PathBuf> {
        let source = self.source.as_ref()?;
        Some(self.current_bookmark_path(source.as_ref()).to_path_buf())
    }

    fn show_bookmark_popover_contents(&mut self, ui: &mut egui::Ui, rows_height: f32) {
        let i18n = self.i18n();
        ui.horizontal(|ui| {
            ui.heading(RichText::new(i18n.text("bookmark.title")).color(theme::TEXT_PRIMARY));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(small_icon_button(
                        icons::DISMISS,
                        icons::IconStyle::Regular,
                        theme::TEXT_PRIMARY,
                    ))
                    .on_hover_text(i18n.text("common.close"))
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
            bookmark_search_box(ui, &mut self.bookmark_search, search_width, i18n);
            self.show_bookmark_clear_button(ui, bookmark_count);
        });

        ui.add_space(12.0);
        ui.separator();
        ui.add_space(4.0);
        self.show_bookmark_filter_tabs(ui);
        ui.add_space(12.0);
        self.show_bookmark_rows(ui, rows_height);
    }

    fn show_bookmark_clear_button(&mut self, ui: &mut egui::Ui, bookmark_count: usize) {
        if bookmark_count == 0 {
            self.bookmark_delete_dialog = None;
        }
        let label = if self.bookmark_filter == BookmarkFilter::ThisBook {
            self.i18n().text("bookmark.clear_this_book")
        } else {
            self.i18n().text("bookmark.clear_all")
        };
        if ui
            .add_enabled(
                bookmark_count > 0,
                egui::Button::new(icons::icon_text(icons::DELETE, &label))
                    .min_size(egui::vec2(108.0, 40.0))
                    .fill(Color32::from_rgb(38, 41, 47))
                    .stroke(Stroke::new(1.0, Color32::from_rgb(58, 64, 73))),
            )
            .on_hover_text(if self.bookmark_filter == BookmarkFilter::ThisBook {
                self.i18n().text("bookmark.clear_this_book.help")
            } else {
                self.i18n().text("bookmark.clear_all.help")
            })
            .clicked()
        {
            self.clear_pending_page_turns();
            self.clear_pending_sibling_book_turns();
            self.bookmark_delete_dialog = Some(super::super::BookmarkDeleteDialog {
                scope: self.bookmark_filter,
                count: bookmark_count,
            });
        }
    }

    fn show_bookmark_delete_dialog(&mut self, ctx: &egui::Context, popover_rect: Rect) {
        let Some(mut dialog) = self.bookmark_delete_dialog else {
            return;
        };
        let count = self.bookmark_delete_scope_count_for(dialog.scope);
        if count == 0 {
            self.bookmark_delete_dialog = None;
            return;
        }
        dialog.count = count;
        self.bookmark_delete_dialog = Some(dialog);

        let i18n = self.i18n();
        let scope_label = match dialog.scope {
            BookmarkFilter::All => i18n.text("bookmark.scope_all"),
            BookmarkFilter::ThisBook => i18n.text("bookmark.scope_this_book"),
        };
        let message = i18n.with_vars(
            "bookmark.delete_message",
            &[("scope", scope_label), ("count", count.to_string())],
        );
        let note = if self.bookmark_search.trim().is_empty() {
            i18n.text("bookmark.delete_irreversible")
        } else {
            i18n.text("bookmark.delete_search_note")
        };
        let dialog_size = egui::vec2(320.0, 164.0);
        let mut cancel_clicked = false;
        let mut delete_clicked = false;

        egui::Area::new(egui::Id::new("bookmark_delete_dialog"))
            .fixed_pos(popover_rect.min)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                let (overlay_rect, _) = ui.allocate_exact_size(popover_rect.size(), Sense::click());
                ui.painter().rect_filled(
                    overlay_rect,
                    CornerRadius::same(7),
                    Color32::from_rgba_unmultiplied(0, 0, 0, 112),
                );

                let dialog_rect = Rect::from_center_size(overlay_rect.center(), dialog_size);
                ui.scope_builder(egui::UiBuilder::new().max_rect(dialog_rect), |ui| {
                    Frame::new()
                        .fill(Color32::from_rgb(23, 25, 29))
                        .stroke(Stroke::new(1.0, Color32::from_rgb(76, 82, 92)))
                        .corner_radius(CornerRadius::same(8))
                        .inner_margin(Margin::symmetric(16, 14))
                        .show(ui, |ui| {
                            ui.set_min_size(dialog_size - egui::vec2(32.0, 28.0));
                            ui.label(
                                RichText::new(i18n.text("bookmark.delete_title"))
                                    .size(18.0)
                                    .strong()
                                    .color(theme::TEXT_PRIMARY),
                            );
                            ui.add_space(8.0);
                            ui.label(
                                RichText::new(message.as_str())
                                    .size(14.0)
                                    .color(theme::TEXT_PRIMARY),
                            );
                            ui.label(RichText::new(note).size(12.5).color(theme::TEXT_MUTED));
                            ui.add_space(16.0);
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui
                                        .add_sized(
                                            [82.0, 34.0],
                                            egui::Button::new(i18n.text("common.delete"))
                                                .fill(theme::ACCENT)
                                                .stroke(Stroke::new(1.0, theme::ACCENT_HOVER)),
                                        )
                                        .clicked()
                                    {
                                        delete_clicked = true;
                                    }
                                    if ui
                                        .add_sized(
                                            [82.0, 34.0],
                                            egui::Button::new(i18n.text("common.cancel"))
                                                .fill(Color32::from_rgb(38, 41, 47))
                                                .stroke(Stroke::new(
                                                    1.0,
                                                    Color32::from_rgb(58, 64, 73),
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

        if cancel_clicked {
            self.bookmark_delete_dialog = None;
        } else if delete_clicked {
            self.clear_bookmarks_for_scope(dialog.scope);
        }
    }

    fn show_bookmark_filter_tabs(&mut self, ui: &mut egui::Ui) {
        let i18n = self.i18n();
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
            draw_filter_tab_label(&painter, tab_rect, filter, accent, text_color, i18n);
            if selected {
                let underline = Rect::from_min_max(
                    egui::pos2(tab_rect.left() + 18.0, rect.bottom() - 3.0),
                    egui::pos2(tab_rect.right() - 10.0, rect.bottom()),
                );
                painter.rect_filled(underline, CornerRadius::same(3), theme::ACCENT_HOVER);
            }
            if response.clicked() {
                self.bookmark_filter = filter;
                self.bookmark_delete_dialog = None;
            }
        }
    }

    fn show_bookmark_rows(&mut self, ui: &mut egui::Ui, rows_height: f32) {
        if self.bookmark_filter == BookmarkFilter::ThisBook && self.book_id.is_none() {
            empty_bookmark_message(
                ui,
                &self.i18n().text("bookmark.empty_this_book"),
                rows_height,
            );
            return;
        }

        self.refresh_bookmark_rows_if_needed();
        if self.bookmark_rows.len() == 0 {
            empty_bookmark_message(ui, &self.i18n().text("bookmark.empty_all"), rows_height);
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
        let active_path = self.current_bookmark_source_path();
        let current_book = self.book_id.as_deref() == Some(row.book_id.as_str())
            && active_path.as_ref().is_some_and(|path| {
                row.known_path
                    .as_deref()
                    .is_some_and(|row_path| row_path == path.to_string_lossy().as_ref())
            });
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

        let frame_response = frame.show(ui, |ui| {
            ui.set_min_height(66.0);
            ui.horizontal(|ui| {
                let thumbnail = self.bookmark_thumbnail_state(&row, current_book);
                if show_bookmark_thumbnail(ui, thumbnail).clicked() {
                    jump_to_page = true;
                }

                let title_width = (ui.available_width() - ROW_ACTION_WIDTH).max(48.0);
                let title_response = allocate_bookmark_title(ui, &row, title_width);
                title_rect = title_response.rect;
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
                        .on_hover_text(self.i18n().text("bookmark.delete_one.help"))
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
        paint_bookmark_title(ui, title_rect, &row);
        if row_response.clicked() {
            jump_to_page = true;
        }

        if remove_bookmark {
            self.bookmark_delete_dialog = None;
            if let Some(path) = row.known_path.as_deref() {
                self.store.remove_page_bookmark(
                    &row.book_id,
                    std::path::Path::new(path),
                    row.bookmark.page,
                );
            }
            self.bookmark_rows.clear();
        } else if jump_to_page {
            self.bookmark_delete_dialog = None;
            self.jump_to_bookmark(row);
        }
    }

    fn clear_bookmarks_for_scope(&mut self, scope: BookmarkFilter) {
        let removed = match scope {
            BookmarkFilter::All => self.store.clear_all_page_bookmarks(),
            BookmarkFilter::ThisBook => {
                let Some(book_id) = self.book_id.clone() else {
                    self.bookmark_delete_dialog = None;
                    self.notify(self.i18n().text("bookmark.no_delete_target"));
                    return;
                };
                let Some(source_path) = self.current_bookmark_source_path() else {
                    self.bookmark_delete_dialog = None;
                    self.notify(self.i18n().text("bookmark.no_delete_target"));
                    return;
                };
                self.store.clear_page_bookmarks(&book_id, &source_path)
            }
        };
        self.bookmark_delete_dialog = None;
        if removed == 0 {
            self.notify(self.i18n().text("bookmark.no_delete_target"));
        } else {
            if let Some(thumbnails) = self.bookmark_thumbnails.as_mut() {
                thumbnails.clear();
            }
            let key = if scope == BookmarkFilter::ThisBook {
                "bookmark.clear_done_this_book"
            } else {
                "bookmark.clear_done_all"
            };
            self.notify(
                self.i18n()
                    .with_vars(key, &[("count", removed.to_string())]),
            );
            self.bookmark_rows.clear();
        }
    }

    fn bookmark_delete_scope_count(&self) -> usize {
        self.bookmark_delete_scope_count_for(self.bookmark_filter)
    }

    fn bookmark_delete_scope_count_for(&self, scope: BookmarkFilter) -> usize {
        match scope {
            BookmarkFilter::All => self.store.all_page_bookmark_count(),
            BookmarkFilter::ThisBook => self
                .book_id
                .as_deref()
                .and_then(|book_id| {
                    self.current_bookmark_source_path()
                        .map(|path| self.store.page_bookmark_entries(book_id, &path).len())
                })
                .unwrap_or_default(),
        }
    }

    fn bookmark_entries_for_active_filter(&self) -> Vec<PageBookmarkEntry> {
        match self.bookmark_filter {
            BookmarkFilter::All => self.store.all_page_bookmarks(),
            BookmarkFilter::ThisBook => self
                .book_id
                .as_deref()
                .and_then(|book_id| {
                    self.current_bookmark_source_path()
                        .map(|path| self.store.page_bookmark_entries(book_id, &path))
                })
                .unwrap_or_default(),
        }
    }

    fn refresh_bookmark_rows_if_needed(&mut self) {
        let filter = self.bookmark_filter;
        let book_id = self.book_id.clone();
        let source_path = self
            .current_bookmark_source_path()
            .map(|path| path.to_string_lossy().to_string());
        let query = self.bookmark_search.clone();
        if self.bookmark_rows.needs_refresh(
            filter,
            book_id.as_deref(),
            source_path.as_deref(),
            &query,
        ) {
            let entries = self.bookmark_entries_for_active_filter();
            self.bookmark_rows.refresh(
                filter,
                book_id.as_deref(),
                source_path.as_deref(),
                &query,
                entries,
            );
        }
    }

    fn bookmark_thumbnail_state(
        &mut self,
        row: &BookmarkRow,
        current_book: bool,
    ) -> BookmarkThumbnailState {
        let source = current_book.then(|| self.source.clone()).flatten();
        let decode = self.decode_options();
        self.ensure_bookmark_thumbnails().thumbnail(
            source,
            &row.book_id,
            row.known_path.as_deref(),
            row.bookmark.page,
            row.bookmark.page_name.as_deref(),
            decode,
        )
    }

    fn jump_to_bookmark(&mut self, row: BookmarkRow) {
        let active_path = self.current_bookmark_source_path();
        let current_bookmark_path = active_path.as_ref().is_some_and(|path| {
            row.known_path
                .as_deref()
                .is_some_and(|row_path| row_path == path.to_string_lossy().as_ref())
        });
        if self.book_id.as_deref() == Some(row.book_id.as_str()) && current_bookmark_path {
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
            self.notify(self.i18n().text("bookmark.no_path"));
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

fn bookmark_search_box(ui: &mut egui::Ui, search: &mut String, width: f32, i18n: I18n) {
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
                        .hint_text(i18n.text("common.search"))
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
    i18n: I18n,
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
        filter.label_i18n(i18n),
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
