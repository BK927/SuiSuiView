//! App-side driver for the vertical-strip view mode: anchor bookkeeping, frame
//! layout + paint, pointer/keyboard scroll, and explicit page jumps. The pure
//! layout/scroll math it calls lives in the parent `strip` module.

use super::{
    accumulate_edge_overscroll, display_height, jump_to_page, layout_visible, median_known_height,
    page_at_viewport_center, recenter_target, scroll_by, StripAnchor, StripPageDims,
};
use crate::app::commands::{command_for_mouse_gesture, AppCommand};
use crate::app::navigation::MAX_QUEUED_WORKER_VISIBLE_PAGES;
use crate::app::viewer::PagePaintOutcome;
use crate::app::SuiSuiViewApp;
use crate::core::source::BookSource;
use crate::core::state::MouseGesture;
use crate::core::worker::NavigationDirection;
use egui::{Rect, Vec2};
use std::time::Instant;

/// A trackpad pinch factor closer to 1.0 than this counts as no zoom input.
const STRIP_ZOOM_DELTA_EPSILON: f32 = 1e-4;
/// Viewport size assumed before the first real viewer layout has been measured.
const STRIP_FALLBACK_VIEWPORT: Vec2 = Vec2 {
    x: 1000.0,
    y: 800.0,
};

impl SuiSuiViewApp {
    pub(in crate::app) fn paint_strip(
        &mut self,
        ctx: &egui::Context,
        painter: &egui::Painter,
        viewport: Rect,
    ) {
        let Some(source) = self.source.clone() else {
            return;
        };
        let page_count = source.page_count();
        if page_count == 0 {
            return;
        }
        let Some((anchor_index, offset_frac)) =
            self.strip_resolve_anchor(source.as_ref(), page_count)
        else {
            return;
        };

        let fallback = self.strip_fallback_height(
            source.as_ref(),
            page_count,
            viewport.width(),
            viewport.height(),
        );
        let placements = {
            let height_of = |index: usize| {
                self.strip_display_height(source.as_ref(), index, viewport.width(), fallback)
            };
            layout_visible(anchor_index, offset_frac, viewport, page_count, &height_of)
        };
        self.strip_visible_indices = placements.iter().map(|placement| placement.index).collect();

        if let Some(center) = page_at_viewport_center(&placements, viewport) {
            if let Some(target) = recenter_target(self.current_page, center) {
                self.current_page = target;
                self.persist_reading_position_deferred();
                let visible_count = placements
                    .len()
                    .saturating_add(4)
                    .clamp(1, MAX_QUEUED_WORKER_VISIBLE_PAGES);
                self.worker.set_page(
                    target,
                    self.strip_last_scroll_dir,
                    self.target_long_edge,
                    visible_count,
                    self.worker_options(),
                );
            }
        }

        let mut current_outcome: Option<PagePaintOutcome> = None;
        for placement in &placements {
            let visual = self.page_visual(ctx, placement.index, self.target_long_edge);
            let outcome = self.paint_page_visual(
                ctx,
                painter,
                viewport,
                placement.index,
                visual,
                placement.rect,
                1.0,
            );
            if placement.index == self.current_page {
                current_outcome = Some(outcome);
            }
        }
        // Release the sibling-book "visual pending" gate once the page the reader
        // is centred on has fully drawn, mirroring paint_spread so queued sibling
        // turns and GPU-effect fallback resolve in strip mode too.
        if let Some(outcome) = current_outcome {
            if outcome.fully_drawn {
                self.mark_current_book_visual_painted_after_spread(
                    ctx,
                    outcome.used_wgpu_callback,
                    outcome.needs_sibling_visible_hold,
                );
            }
        }
    }

    pub(in crate::app) fn handle_strip_pointer(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
    ) {
        if response.double_clicked() {
            if let Some(command) =
                command_for_mouse_gesture(MouseGesture::DoubleClick, &self.settings)
            {
                self.apply_command(ui.ctx(), command);
            }
        }
        if response.middle_clicked() {
            let gesture = if ui.input(|input| input.modifiers.ctrl) {
                MouseGesture::CtrlMiddleClick
            } else {
                MouseGesture::MiddleClick
            };
            if let Some(command) = command_for_mouse_gesture(gesture, &self.settings) {
                self.apply_command(ui.ctx(), command);
            }
        }
        if response.dragged() {
            let delta_y = ui.input(|input| input.pointer.delta().y);
            self.strip_scroll_by(-delta_y * self.settings.strip_drag_scroll_multiplier());
        }
        if !response.hovered() {
            return;
        }
        let (scroll_y, ctrl, zoom_delta) = ui.input(|input| {
            (
                input.raw_scroll_delta.y,
                input.modifiers.ctrl,
                input.zoom_delta(),
            )
        });
        if ctrl || (zoom_delta - 1.0).abs() >= STRIP_ZOOM_DELTA_EPSILON {
            // Ctrl+wheel and trackpad pinch are zoom gestures, unsupported in strip.
            self.notify_strip_zoom_unsupported();
        } else if scroll_y != 0.0 {
            // Continuous scroll: no 30px page-turn threshold, no notch accumulator.
            // A raw wheel notch is only ~40 points, so the sensitivity multiplier
            // is what makes strip reading tolerable.
            self.strip_scroll_by(-scroll_y * self.settings.strip_wheel_scroll_multiplier());
        }
    }

    pub(in crate::app) fn strip_scroll_by(&mut self, delta_px: f32) {
        let Some(source) = self.source.clone() else {
            return;
        };
        let page_count = source.page_count();
        if page_count == 0 {
            return;
        }
        let viewport = self
            .last_viewer_size_points
            .unwrap_or(STRIP_FALLBACK_VIEWPORT);
        let Some((anchor_index, offset_frac)) =
            self.strip_resolve_anchor(source.as_ref(), page_count)
        else {
            return;
        };
        let fallback =
            self.strip_fallback_height(source.as_ref(), page_count, viewport.x, viewport.y);
        let (new_index, new_offset, edge) = {
            let height_of = |index: usize| {
                self.strip_display_height(source.as_ref(), index, viewport.x, fallback)
            };
            scroll_by(
                anchor_index,
                offset_frac,
                delta_px,
                viewport.y,
                page_count,
                &height_of,
            )
        };
        let moved = new_index != anchor_index || new_offset != offset_frac;
        if let Some(page_id) = source.page_id(new_index) {
            self.strip_anchor = Some(StripAnchor {
                page_id,
                offset_frac: new_offset,
            });
        }
        if delta_px > 0.0 {
            self.strip_last_scroll_dir = NavigationDirection::Forward;
        } else if delta_px < 0.0 {
            self.strip_last_scroll_dir = NavigationDirection::Backward;
        }
        match edge {
            // Already pinned to the edge (the scroll produced no movement):
            // sustained overscroll turns the page. The event that merely reaches
            // the edge moves the anchor and so falls through to the reset arm.
            Some(direction) if !moved => {
                if accumulate_edge_overscroll(
                    &mut self.strip_edge_overscroll_px,
                    &mut self.strip_edge_overscroll_at,
                    direction,
                    delta_px,
                    Instant::now(),
                ) {
                    self.handle_edge_page(direction);
                }
            }
            _ => {
                self.strip_edge_overscroll_px = 0.0;
                self.strip_edge_overscroll_at = None;
            }
        }
        self.egui_ctx.request_repaint();
    }

    pub(in crate::app) fn strip_jump_to_page(&mut self, index: usize) {
        let Some(source) = self.source.clone() else {
            return;
        };
        let page_count = source.page_count();
        if page_count == 0 {
            return;
        }
        let (index, offset_frac) = jump_to_page(index.min(page_count - 1));
        let Some(page_id) = source.page_id(index) else {
            return;
        };
        self.strip_anchor = Some(StripAnchor {
            page_id,
            offset_frac,
        });
        self.current_page = index;
        self.strip_edge_overscroll_px = 0.0;
        self.strip_edge_overscroll_at = None;
        self.persist_reading_position_deferred();
        self.worker.set_page(
            index,
            self.strip_last_scroll_dir,
            self.target_long_edge,
            self.visible_page_count(),
            self.worker_options(),
        );
        self.egui_ctx.request_repaint();
    }

    /// Strip overrides for the navigation/zoom commands while strip mode is
    /// active. Returns `true` when the command was consumed here (and must not
    /// fall through to the paged handler). Double-mode and toggle commands are
    /// intentionally left to fall through so they exit the strip into their mode.
    pub(in crate::app) fn apply_strip_keyboard_override(&mut self, command: AppCommand) -> bool {
        let viewport_height = self
            .last_viewer_size_points
            .map_or(STRIP_FALLBACK_VIEWPORT.y, |size| size.y);
        match command {
            AppCommand::NextPage => self.strip_scroll_by(viewport_height * 0.9),
            AppCommand::PreviousPage => self.strip_scroll_by(-viewport_height * 0.9),
            AppCommand::Home => self.strip_jump_to_page(0),
            AppCommand::End => {
                if let Some(source) = self.source.as_ref() {
                    let last = source.page_count().saturating_sub(1);
                    self.strip_jump_to_page(last);
                }
            }
            AppCommand::MovePages(delta) | AppCommand::ForceMovePages(delta) => {
                self.strip_jump_to_page(self.strip_clamped_target(delta));
            }
            AppCommand::Zoom(_) | AppCommand::ZoomFine(_) => self.notify_strip_zoom_unsupported(),
            _ => return false,
        }
        true
    }

    fn strip_clamped_target(&self, delta: isize) -> usize {
        let max_page = self
            .source
            .as_ref()
            .map_or(0, |source| source.page_count().saturating_sub(1));
        if delta < 0 {
            self.current_page.saturating_sub(delta.unsigned_abs())
        } else {
            self.current_page
                .saturating_add(delta as usize)
                .min(max_page)
        }
    }

    fn notify_strip_zoom_unsupported(&mut self) {
        if self.strip_zoom_notice_shown {
            return;
        }
        self.strip_zoom_notice_shown = true;
        let text = self.i18n().text("strip.zoom_unsupported");
        self.notify(text);
    }

    /// Resolve the anchor to `(index, offset_frac)` in the current source,
    /// rebuilding it at the current page's top when it is absent or its page has
    /// vanished (folder refresh, book switch). `None` only for a degenerate book
    /// whose current page has no resolvable id.
    fn strip_resolve_anchor(
        &mut self,
        source: &dyn BookSource,
        page_count: usize,
    ) -> Option<(usize, f32)> {
        if let Some(anchor) = self.strip_anchor {
            if let Some(index) = source.page_index_for_id(anchor.page_id) {
                return Some((index, anchor.offset_frac));
            }
        }
        let index = self.current_page.min(page_count - 1);
        let page_id = source.page_id(index)?;
        self.strip_anchor = Some(StripAnchor {
            page_id,
            offset_frac: 0.0,
        });
        Some((index, 0.0))
    }

    /// Fit-width display height for one page, resolving dimensions with the strip
    /// priority: authoritative `page_metrics`, then the header prescan hint, then
    /// the book-typical `fallback`.
    fn strip_display_height(
        &self,
        source: &dyn BookSource,
        index: usize,
        viewport_width: f32,
        fallback: f32,
    ) -> f32 {
        display_height(
            self.strip_page_dims(source, index),
            viewport_width,
            fallback,
        )
    }

    fn strip_page_dims(&self, source: &dyn BookSource, index: usize) -> StripPageDims {
        let Some(page_id) = source.page_id(index) else {
            return StripPageDims::Unknown;
        };
        if let Some(metrics) = self.page_metrics.get(&page_id) {
            return StripPageDims::Exact([
                metrics.width.max(1.0) as u32,
                metrics.height.max(1.0) as u32,
            ]);
        }
        if let Some(dims) = self.strip_dim_hints.get(&page_id) {
            return StripPageDims::Hint(*dims);
        }
        StripPageDims::Unknown
    }

    /// Median of the known fit-width page heights, the height estimate for pages
    /// not yet measured; the viewport height when nothing is known yet.
    fn strip_fallback_height(
        &self,
        source: &dyn BookSource,
        page_count: usize,
        viewport_width: f32,
        viewport_height: f32,
    ) -> f32 {
        let known = (0..page_count).filter_map(|index| match self.strip_page_dims(source, index) {
            StripPageDims::Exact([width, height]) | StripPageDims::Hint([width, height]) => {
                (width != 0).then(|| viewport_width * height as f32 / width as f32)
            }
            StripPageDims::Unknown => None,
        });
        median_known_height(known).unwrap_or(viewport_height)
    }
}
