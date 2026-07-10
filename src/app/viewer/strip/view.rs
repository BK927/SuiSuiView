//! App-side driver for the vertical-strip view mode: anchor bookkeeping, frame
//! layout + paint, pointer/keyboard scroll, and explicit page jumps. The pure
//! layout/scroll math it calls lives in the parent `strip` module.

use super::panels::{
    collect_band_pages, collect_panels, detect_gutter_rows, panel_step_delta, PanelPage,
    PANEL_STEP_MAX_VIEWPORTS, STRIP_SNAP_BASE_STEP_FRAC,
};
use super::{
    accumulate_edge_overscroll, clamp_pan_x, column_width, display_height, flick_debt,
    jump_to_page, layout_visible, median_known_height, page_at_viewport_center, recenter_target,
    scroll_by, smooth_scroll_step, StripAnchor, StripPageDims, STRIP_FLICK_DECAY_PER_SEC,
    STRIP_SCROLL_DECAY_PER_SEC,
};
use crate::app::commands::{command_for_mouse_gesture, AppCommand};
use crate::app::navigation::MAX_QUEUED_WORKER_VISIBLE_PAGES;
use crate::app::viewer::interaction::WHEEL_ZOOM_DELTA_EPSILON;
use crate::app::viewer::PagePaintOutcome;
use crate::app::SuiSuiViewApp;
use crate::core::source::BookSource;
use crate::core::state::{FitMode, MouseGesture};
use crate::core::worker::NavigationDirection;
use egui::{Rect, Vec2};
use std::sync::Arc;
use std::time::Instant;

/// Viewport size assumed before the first real viewer layout has been measured.
const STRIP_FALLBACK_VIEWPORT: Vec2 = Vec2 {
    x: 1000.0,
    y: 800.0,
};

/// Upper bound on panel-gutter analyses per keyboard step, so a single press
/// only ever scans a few sub-millisecond pages of already-decoded pixels.
const STRIP_SNAP_MAX_ANALYSES: usize = 8;

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
        self.drain_strip_scroll_pending(ctx);
        let Some((anchor_index, offset_frac)) =
            self.strip_resolve_anchor(source.as_ref(), page_count)
        else {
            return;
        };

        let column = self.strip_column_width(viewport.size());
        self.pan.x = clamp_pan_x(self.pan.x, column, viewport.width());
        let column_left = viewport.center().x - column / 2.0 + self.pan.x;
        let fallback =
            self.strip_fallback_height(source.as_ref(), page_count, column, viewport.height());
        let placements = {
            let height_of =
                |index: usize| self.strip_display_height(source.as_ref(), index, column, fallback);
            layout_visible(
                anchor_index,
                offset_frac,
                viewport,
                column_left,
                column,
                page_count,
                &height_of,
            )
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
                true,
                // Strip pages each carry a distinct source_key, so one slot is
                // enough — their `draw_id`s never collide within a frame.
                0,
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
        if response.drag_started() {
            // Grabbing the strip stops any in-flight glide, like catching a
            // scrolling touch surface.
            self.strip_scroll_pending_px = 0.0;
            self.strip_flick_pending_px = 0.0;
        }
        if response.dragged() {
            let delta = ui.input(|input| input.pointer.delta());
            // Horizontal drag pans the column (only visible when it is wider than
            // the viewport; `clamp_pan_x` zeroes it otherwise, each paint).
            self.pan.x += delta.x;
            self.strip_scroll_by(-delta.y * self.settings.strip_drag_scroll_multiplier());
        }
        if response.drag_stopped() {
            // Release inertia: coast on from the release velocity with the slower
            // flick decay. Sub-threshold releases add nothing (see `flick_debt`).
            let velocity_y = ui.input(|input| input.pointer.velocity()).y;
            let viewport_height = self
                .last_viewer_size_points
                .map_or(STRIP_FALLBACK_VIEWPORT.y, |size| size.y);
            let debt = flick_debt(
                -velocity_y * self.settings.strip_drag_scroll_multiplier(),
                viewport_height,
            );
            if debt != 0.0 {
                self.strip_flick_pending_px += debt;
                self.egui_ctx.request_repaint();
            }
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
        if ctrl {
            // Ctrl+wheel zooms the column via the shared notch accumulator; the
            // fired zoom command is rerouted into the strip's Manual-column path.
            self.apply_wheel_gesture_steps(ui, scroll_y, zoom_delta);
        } else if (zoom_delta - 1.0).abs() >= WHEEL_ZOOM_DELTA_EPSILON {
            // Trackpad pinch arrives as a zoom factor without the ctrl modifier.
            self.apply_wheel_gesture_steps(ui, 0.0, zoom_delta);
        } else if scroll_y != 0.0 {
            // Continuous scroll: no 30px page-turn threshold, no notch accumulator.
            // A raw wheel notch is only ~40 points, so the sensitivity multiplier
            // is what makes strip reading tolerable.
            self.strip_scroll_animate_by(-scroll_y * self.settings.strip_wheel_scroll_multiplier());
        }
    }

    /// Queue a scroll to be applied over the next few frames with an exponential
    /// ease-out (wheel notches and keyboard steps would otherwise teleport).
    /// Drag input must NOT come through here — it tracks the pointer 1:1 via
    /// [`Self::strip_scroll_by`].
    pub(in crate::app) fn strip_scroll_animate_by(&mut self, delta_px: f32) {
        self.strip_scroll_pending_px += delta_px;
        self.egui_ctx.request_repaint();
    }

    /// Apply this frame's slice of the queued smooth scroll and keep the repaint
    /// chain alive until the debt is drained. The chain is paced at ~8ms rather
    /// than immediate: unthrottled it spins near 200fps for frames a 60Hz panel
    /// never shows, and the dt clamp bounds the jump a hitched frame can take.
    fn drain_strip_scroll_pending(&mut self, ctx: &egui::Context) {
        if self.strip_scroll_pending_px == 0.0 && self.strip_flick_pending_px == 0.0 {
            return;
        }
        let dt = ctx.input(|input| input.stable_dt).min(0.025);
        let (step, remaining) =
            smooth_scroll_step(self.strip_scroll_pending_px, dt, STRIP_SCROLL_DECAY_PER_SEC);
        let (flick_step, flick_remaining) =
            smooth_scroll_step(self.strip_flick_pending_px, dt, STRIP_FLICK_DECAY_PER_SEC);
        self.strip_scroll_pending_px = remaining;
        self.strip_flick_pending_px = flick_remaining;
        self.strip_scroll_by(step + flick_step);
        if self.strip_scroll_pending_px != 0.0 || self.strip_flick_pending_px != 0.0 {
            ctx.request_repaint_after(std::time::Duration::from_millis(8));
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
        let column = self.strip_column_width(viewport);
        let fallback = self.strip_fallback_height(source.as_ref(), page_count, column, viewport.y);
        let (new_index, new_offset, edge) = {
            let height_of =
                |index: usize| self.strip_display_height(source.as_ref(), index, column, fallback);
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
            // Any queued smooth-scroll debt is dropped so one big flick cannot
            // keep pushing into the edge and turn the book on its own.
            Some(direction) if !moved => {
                self.strip_scroll_pending_px = 0.0;
                self.strip_flick_pending_px = 0.0;
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
        // No repaint request here: direct callers run inside an input-driven
        // frame that paints the moved anchor itself, and the smooth-scroll drain
        // paces its own repaint chain (an immediate request here would defeat it).
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
        self.strip_scroll_pending_px = 0.0;
        self.strip_flick_pending_px = 0.0;
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
            AppCommand::NextPage => {
                if self.settings.strip_panel_snap {
                    let delta = self.strip_panel_step_delta(viewport_height, true);
                    self.strip_scroll_animate_by(delta);
                } else {
                    self.strip_scroll_animate_by(viewport_height * 0.9);
                }
            }
            AppCommand::PreviousPage => {
                if self.settings.strip_panel_snap {
                    let delta = self.strip_panel_step_delta(viewport_height, false);
                    self.strip_scroll_animate_by(delta);
                } else {
                    self.strip_scroll_animate_by(-viewport_height * 0.9);
                }
            }
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
            AppCommand::Zoom(factor) => {
                self.strip_begin_manual_zoom();
                self.adjust_zoom(factor);
            }
            AppCommand::ZoomFine(delta) => {
                self.strip_begin_manual_zoom();
                self.adjust_zoom_by_delta(delta);
            }
            _ => return false,
        }
        true
    }

    /// Re-express the current effective column as a Manual zoom so switching into
    /// Manual from any fit does not jump; the shared `adjust_zoom*` then applies
    /// the step with the usual clamps/persist. Idempotent once already Manual.
    fn strip_begin_manual_zoom(&mut self) {
        let viewport = self
            .last_viewer_size_points
            .unwrap_or(STRIP_FALLBACK_VIEWPORT);
        let median = self.strip_source_median_dims();
        let ppp = self.egui_ctx.pixels_per_point();
        let current = column_width(self.fit_mode, self.manual_zoom, viewport, median, ppp);
        let original = column_width(FitMode::Original, 1.0, viewport, median, ppp);
        self.fit_mode = FitMode::Manual;
        if original > 0.0 {
            self.manual_zoom = current / original;
        }
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

    /// Panel-slideshow keyboard step (points, signed): bring the next/previous
    /// cut to the viewport center, walking within cuts taller than the viewport
    /// and gliding across blank pages and inter-cut whitespace in one press.
    /// Falls back to the plain fixed step when no cut gives a better answer.
    /// Everything is computed in delta space (0.0 = the current viewport top) so
    /// far-page height estimates can never shift the result.
    fn strip_panel_step_delta(&mut self, viewport_height: f32, forward: bool) -> f32 {
        let raw = if forward {
            viewport_height * STRIP_SNAP_BASE_STEP_FRAC
        } else {
            -viewport_height * STRIP_SNAP_BASE_STEP_FRAC
        };
        let Some(source) = self.source.clone() else {
            return raw;
        };
        let page_count = source.page_count();
        if page_count == 0 {
            return raw;
        }
        let viewport = self
            .last_viewer_size_points
            .unwrap_or(STRIP_FALLBACK_VIEWPORT);
        let Some((anchor_index, offset_frac)) =
            self.strip_resolve_anchor(source.as_ref(), page_count)
        else {
            return raw;
        };
        let column = self.strip_column_width(viewport);
        let fallback = self.strip_fallback_height(source.as_ref(), page_count, column, viewport.y);

        // Pages overlapping the search reach: the current viewport (the cut under
        // the center may start a page back) plus the furthest a step may travel.
        let reach = PANEL_STEP_MAX_VIEWPORTS * viewport_height;
        let (lo, hi) = if forward {
            (-1.5 * viewport_height, reach + viewport_height)
        } else {
            (-reach - viewport_height, 1.5 * viewport_height)
        };
        let band = {
            let height_of =
                |index: usize| self.strip_display_height(source.as_ref(), index, column, fallback);
            collect_band_pages(anchor_index, offset_frac, lo, hi, page_count, &height_of)
        };

        // Gutters per page (bounded fresh analyses); a page with no decoded
        // pixels yet contributes a full-content span, the conservative estimate.
        let mut analysis_budget = STRIP_SNAP_MAX_ANALYSES;
        let pages: Vec<PanelPage> = band
            .into_iter()
            .map(|(index, top_delta, height)| {
                let gutters = self.strip_gutters_for(index, &mut analysis_budget);
                PanelPage {
                    top: top_delta,
                    height,
                    gutters: gutters.as_deref().map(<[_]>::to_vec).unwrap_or_default(),
                    analyzed: gutters.is_some(),
                }
            })
            .collect();
        let panels = collect_panels(&pages);
        panel_step_delta(
            viewport_height,
            viewport_height * STRIP_SNAP_BASE_STEP_FRAC,
            &panels,
            forward,
        )
        .unwrap_or(raw)
    }

    /// Panel-gutter ranges for `index`, from the cache or a fresh bounded scan of
    /// the best already-decoded resolution. `None` means the page is UNKNOWN —
    /// no decoded pixels yet, or the per-press budget is spent — so the caller
    /// treats it as an assumed full-content page and retries next press; a page
    /// analysed with no gutters caches (and returns) an empty list.
    fn strip_gutters_for(&mut self, index: usize, budget: &mut usize) -> Option<Arc<[(f32, f32)]>> {
        let page_id = self
            .source
            .as_ref()
            .and_then(|source| source.page_id(index))?;
        if let Some(gutters) = self.strip_panel_gutters.get(&page_id) {
            return Some(gutters.clone());
        }
        if *budget == 0 {
            return None;
        }
        let gutters = self.strip_analyze_page_gutters(index)?;
        *budget -= 1;
        let gutters: Arc<[(f32, f32)]> = Arc::from(gutters);
        self.strip_panel_gutters.insert(page_id, gutters.clone());
        Some(gutters)
    }

    /// Detect panel gutters for `index` from the best already-decoded resolution.
    /// `Some(list)` (possibly empty) when decoded pixels were available; `None`
    /// when none exist yet, so the caller does not cache a miss. Fractions are
    /// scale-invariant, so any cached resolution is fine. Uses `peek` to avoid
    /// disturbing the decoded-cache LRU order.
    fn strip_analyze_page_gutters(&self, index: usize) -> Option<Vec<(f32, f32)>> {
        let key = self.page_key_at(index, self.target_long_edge)?;
        let best_key = self.best_page_key(key)?;
        let page = self.decoded_pages.peek(&best_key)?;
        Some(detect_gutter_rows(
            page.pixels.as_slice(),
            page.pixels.bytes_per_pixel(),
            page.display_width,
            page.display_height,
        ))
    }

    /// Width in points of the centered page column under the current fit/zoom.
    /// The single source of the display width used by paint, scroll, and decode
    /// targeting so they never disagree about how wide a page is drawn.
    pub(in crate::app) fn strip_column_width(&self, viewport: Vec2) -> f32 {
        column_width(
            self.fit_mode,
            self.manual_zoom,
            viewport,
            self.strip_source_median_dims(),
            self.egui_ctx.pixels_per_point(),
        )
    }

    /// The book's typical page dimensions `[w, h]` in source pixels, or `None`
    /// until at least one page has been measured. Memoized against
    /// `strip_dims_revision` so the O(page-count) median walk runs only when a
    /// dimension actually changed, not every frame.
    fn strip_source_median_dims(&self) -> Option<[u32; 2]> {
        if let Some((revision, median)) = self.strip_median_cache.get() {
            if revision == self.strip_dims_revision {
                return median;
            }
        }
        let source = self.source.as_ref()?;
        let median = self.strip_median_dims(source.as_ref(), source.page_count());
        self.strip_median_cache
            .set(Some((self.strip_dims_revision, median)));
        median
    }

    /// Per-axis medians of every page whose size is known (page_metrics first,
    /// prescan hint next). Independent medians of width and height rather than a
    /// median aspect ratio: webtoon pages are near-uniform so the two agree, and
    /// per-axis medians resist a single stray page skewing the typical size.
    fn strip_median_dims(&self, source: &dyn BookSource, page_count: usize) -> Option<[u32; 2]> {
        let mut widths = Vec::new();
        let mut heights = Vec::new();
        for index in 0..page_count {
            if let StripPageDims::Exact([width, height]) | StripPageDims::Hint([width, height]) =
                self.strip_page_dims(source, index)
            {
                if width != 0 && height != 0 {
                    widths.push(width as f32);
                    heights.push(height as f32);
                }
            }
        }
        let median_w = median_known_height(widths.into_iter())?;
        let median_h = median_known_height(heights.into_iter())?;
        Some([
            median_w.round().max(1.0) as u32,
            median_h.round().max(1.0) as u32,
        ])
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

    /// Display height for one page at the current column width, resolving
    /// dimensions with the strip priority: authoritative `page_metrics`, then the
    /// header prescan hint, then the book-typical `fallback`.
    fn strip_display_height(
        &self,
        source: &dyn BookSource,
        index: usize,
        column: f32,
        fallback: f32,
    ) -> f32 {
        display_height(self.strip_page_dims(source, index), column, fallback)
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

    /// Median of the known page heights at the current column width, the height
    /// estimate for pages not yet measured; the viewport height when nothing is
    /// known yet.
    fn strip_fallback_height(
        &self,
        source: &dyn BookSource,
        page_count: usize,
        column: f32,
        viewport_height: f32,
    ) -> f32 {
        let known = (0..page_count).filter_map(|index| match self.strip_page_dims(source, index) {
            StripPageDims::Exact([width, height]) | StripPageDims::Hint([width, height]) => {
                (width != 0).then(|| column * height as f32 / width as f32)
            }
            StripPageDims::Unknown => None,
        });
        median_known_height(known).unwrap_or(viewport_height)
    }
}
