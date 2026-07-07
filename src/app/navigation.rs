use super::{
    perf, sibling_book_path, transition_screen_sign, worker_center_page_for_mode, EdgePrompt,
    OpenOrigin, SuiSuiViewApp, Transition, ViewMode, SIBLING_BOOK_TURN_REPAINT_DELAY,
};
use crate::core::effects::ViewEffects;
use crate::core::state::{EdgePageAction, FitMode, PageTransitionStyle, ReadingDirection};
use crate::core::worker::{DecodeOptions, NavigationDirection};
use egui::{self, Vec2};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const MAX_QUEUED_PAGE_TURNS: usize = 1;
pub(in crate::app) const MAX_QUEUED_WORKER_VISIBLE_PAGES: usize = 25;
const MAX_QUEUED_SIBLING_BOOK_TURNS: usize = 1;
const SIBLING_OPEN_RETRY_LIMIT: usize = 16;

/// A zoom gesture is treated as "in motion" for this long after the last
/// interactive `manual_zoom` change. While in motion, the WGSL downscaler is
/// routed through the cheap cached hardware-mipmap path (trilinear sample);
/// once it settles, the exact quality downscale is rendered once more.
const ZOOM_SETTLE_MS: u64 = 200;

/// Interactive manual-zoom bounds. The maximum pairs with the pixel-grid
/// inspection use case (16x shows a 64px-wide detail across ~1000px); the
/// display scale cap in `scale_for` keeps DPI headroom above this.
const MIN_MANUAL_ZOOM: f32 = 0.1;
const MAX_MANUAL_ZOOM: f32 = 16.0;

/// Active "skip unopenable sibling books" walk: while set, a failed sibling
/// open continues to the next candidate in the same direction instead of
/// stopping at the failure toast.
pub(in crate::app) struct SiblingOpenRetry {
    pub(in crate::app) direction: isize,
    pub(in crate::app) attempts_left: usize,
    pub(in crate::app) origin_book: Option<PathBuf>,
}

impl SuiSuiViewApp {
    pub(in crate::app) fn next_page(&mut self) {
        let direction = NavigationDirection::Forward;
        if let Some(target) = self.page_turn_target(direction) {
            self.navigate_to_page_turn_target(target, direction);
        } else {
            self.handle_edge_page(direction);
        }
    }

    pub(in crate::app) fn previous_page(&mut self) {
        let direction = NavigationDirection::Backward;
        if let Some(target) = self.page_turn_target(direction) {
            self.navigate_to_page_turn_target(target, direction);
        } else {
            self.handle_edge_page(direction);
        }
    }

    /// Resolve `target` past any vanished folder pages, then navigate. When a
    /// missing file was skipped (or ran off the edge), the open folder snapshot
    /// is stale, so kick off an off-thread rebuild.
    fn navigate_to_page_turn_target(&mut self, target: usize, direction: NavigationDirection) {
        match self.skip_missing_page_target(target, direction) {
            Some(resolved) => {
                if resolved != target {
                    self.request_folder_refresh();
                }
                self.set_page(resolved, direction);
            }
            None => {
                self.request_folder_refresh();
                self.handle_edge_page(direction);
            }
        }
    }

    fn page_turn_target(&self, direction: NavigationDirection) -> Option<usize> {
        self.page_turn_target_from(self.current_page, direction)
    }

    pub(in crate::app) fn page_turn_target_from(
        &self,
        page: usize,
        direction: NavigationDirection,
    ) -> Option<usize> {
        let source = self.source.as_ref()?;
        let max_page = source.page_count().saturating_sub(1);
        let page = page.min(max_page);
        match direction {
            NavigationDirection::Forward if self.view_mode.is_smart() => {
                let indices = self.spread_indices_for_unordered(page);
                let last = indices.last().copied().unwrap_or(page);
                (last < max_page).then(|| last.saturating_add(1).min(max_page))
            }
            NavigationDirection::Forward => {
                (page < max_page).then(|| page.saturating_add(self.view_mode.step()).min(max_page))
            }
            NavigationDirection::Backward if self.view_mode.is_smart() => {
                let indices = self.spread_indices_for_unordered(page);
                let first = indices.first().copied().unwrap_or(page);
                (first > 0).then(|| first.saturating_sub(1))
            }
            NavigationDirection::Backward => {
                (page > 0).then(|| page.saturating_sub(self.view_mode.step()))
            }
        }
    }

    pub(in crate::app) fn move_pages(&mut self, delta: isize) {
        let Some(source) = self.source.as_ref() else {
            return;
        };
        let max_page = source.page_count().saturating_sub(1);
        let target = if delta.is_negative() {
            self.current_page.saturating_sub(delta.unsigned_abs())
        } else {
            self.current_page
                .saturating_add(delta as usize)
                .min(max_page)
        };
        let direction = if delta < 0 {
            NavigationDirection::Backward
        } else {
            NavigationDirection::Forward
        };
        if target == self.current_page && delta != 0 {
            self.handle_edge_page(direction);
            return;
        }
        self.navigate_to_page_turn_target(target, direction);
    }

    /// Folder pages can vanish underneath the open snapshot (external delete).
    /// For user-driven turns, slide over missing files in the same direction —
    /// this is the responsive half of the vanish handling: the caller
    /// (`navigate_to_page_turn_target`) also requests an off-thread snapshot
    /// refresh (`request_folder_refresh`) that converges the page list, count,
    /// and position by page identity. Other origins return the target unchanged
    /// (ZIP pages cannot individually vanish; a single image is one page).
    fn skip_missing_page_target(
        &self,
        target: usize,
        direction: NavigationDirection,
    ) -> Option<usize> {
        if self.open_origin != Some(OpenOrigin::Folder) {
            return Some(target);
        }
        let source = self.source.as_ref()?;
        skip_missing_target(
            target,
            direction,
            |i| source.page_file_path(i).is_some_and(|p| p.exists()),
            |page, dir| self.page_turn_target_from(page, dir),
            source.page_count(),
        )
    }

    pub(in crate::app) fn handle_edge_page(&mut self, direction: NavigationDirection) {
        match self.edge_page_action_for_current_book() {
            EdgePageAction::Stop => {}
            EdgePageAction::Ask => {
                self.open_edge_prompt(direction);
            }
            EdgePageAction::Wrap => {
                self.wrap_edge_page(direction);
            }
            EdgePageAction::NextBook => match direction {
                NavigationDirection::Forward => self.open_sibling_book(1),
                NavigationDirection::Backward => self.open_sibling_book(-1),
            },
        }
    }

    pub(in crate::app) fn open_edge_prompt(&mut self, direction: NavigationDirection) {
        if self.source.is_none() {
            return;
        }
        if !should_open_edge_prompt(self.edge_prompt, direction) {
            return;
        }
        self.edge_prompt = Some(EdgePrompt::new(direction));
    }

    pub(in crate::app) fn edge_page_action_for_current_book(&self) -> EdgePageAction {
        match self.open_origin {
            Some(OpenOrigin::ZipCbz) => self.settings.archive_edge_page_action,
            Some(OpenOrigin::Folder | OpenOrigin::SingleImage) => {
                self.settings.image_edge_page_action
            }
            None => self.settings.edge_page_action,
        }
    }

    pub(in crate::app) fn wrap_edge_page(&mut self, direction: NavigationDirection) {
        let Some(source) = self.source.as_ref() else {
            return;
        };
        let target = match direction {
            NavigationDirection::Forward => 0,
            NavigationDirection::Backward => source.page_count().saturating_sub(1),
        };
        self.set_page(target, direction);
    }

    pub(in crate::app) fn random_page(&mut self, direction: NavigationDirection) {
        let Some(source) = self.source.as_ref() else {
            return;
        };
        let page_count = source.page_count();
        if page_count <= 1 {
            return;
        }
        let offset = random_offset(page_count - 1);
        let target = match direction {
            NavigationDirection::Forward => (self.current_page + offset) % page_count,
            NavigationDirection::Backward => (self.current_page + page_count - offset) % page_count,
        };
        self.set_page(target, direction);
    }

    pub(in crate::app) fn set_page(&mut self, target: usize, direction: NavigationDirection) {
        // Strip mode has no paged commit path: explicit page selection (bookmark
        // jump, top-bar page field, wrap/random edge actions) re-anchors the strip
        // at the target's top instead of turning a spread.
        if self.view_mode == ViewMode::VerticalStrip {
            self.strip_jump_to_page(target);
            return;
        }
        let Some(source) = self.source.as_ref() else {
            return;
        };
        let target = target.min(source.page_count().saturating_sub(1));
        if target == self.current_page {
            return;
        }
        if self.pending_page_turn.is_some() || self.page_turn_paint_hold {
            self.queue_page_turn(direction);
            return;
        }
        self.edge_prompt = None;

        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        let turn_started = Instant::now();
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        let cache_state = self
            .page_key_at(target, self.target_long_edge)
            .map_or(perf::PageCacheState::Miss, |requested_key| {
                self.page_turn_cache_state(requested_key)
            });
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        perf::record_page_turn_request(cache_state, target, self.target_long_edge);
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        if cache_state.cached() {
            self.page_turn_started_at = None;
            perf::record_page_turn_ready(turn_started, cache_state, target, self.target_long_edge);
        } else {
            self.page_turn_started_at = Some((target, turn_started));
        }
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        self.record_cache_snapshot("page_turn");

        if !self.page_turn_target_ready(target) {
            self.defer_page_turn(target, direction);
            return;
        }

        self.commit_page_turn(target, direction);
    }

    fn defer_page_turn(&mut self, target: usize, direction: NavigationDirection) {
        self.pending_page_turn = Some(super::PendingPageTurn { target, direction });
        self.last_nav_direction = direction;
        self.request_pending_page_turn_work();
    }

    pub(in crate::app) fn commit_pending_page_turn_if_ready(&mut self) {
        let Some(pending) = self.pending_page_turn else {
            return;
        };
        if !self.page_turn_target_ready(pending.target) {
            return;
        }
        self.pending_page_turn = None;
        self.commit_page_turn(pending.target, pending.direction);
        self.page_turn_paint_hold = true;
        self.egui_ctx
            .request_repaint_after(Duration::from_millis(16));
    }

    fn queue_page_turn(&mut self, direction: NavigationDirection) {
        self.edge_prompt = None;
        if let Some(pending) = self.pending_page_turn {
            if pending.direction != direction {
                self.cancel_or_rewind_queued_page_turn(direction);
                return;
            }
        }

        push_queued_page_turn(&mut self.queued_page_turns, direction);
        self.request_pending_page_turn_work();
        self.egui_ctx
            .request_repaint_after(Duration::from_millis(16));
    }

    fn cancel_or_rewind_queued_page_turn(&mut self, direction: NavigationDirection) {
        match self.queued_page_turns.as_mut() {
            Some(queued) if queued.remaining > 1 => {
                queued.remaining -= 1;
            }
            Some(_) => {
                self.queued_page_turns = None;
            }
            None => {
                self.clear_pending_page_turns();
                self.queue_page_turn(direction);
            }
        }
    }

    pub(in crate::app) fn drive_queued_page_turn_after_paint(&mut self, ctx: &egui::Context) {
        self.page_turn_paint_hold = false;
        if self.pending_page_turn.is_some() {
            return;
        }
        let Some(mut queued) = self.queued_page_turns.take() else {
            return;
        };
        if queued.remaining == 0 {
            return;
        }
        let direction = queued.direction;
        queued.remaining -= 1;

        if let Some(target) = self.page_turn_target(direction) {
            if queued.remaining > 0 {
                self.queued_page_turns = Some(queued);
            }
            self.set_page(target, direction);
            if self.queued_page_turns.is_some() || self.pending_page_turn.is_some() {
                ctx.request_repaint_after(Duration::from_millis(16));
            }
        } else {
            self.handle_edge_page(direction);
        }
    }

    pub(in crate::app) fn clear_pending_page_turns(&mut self) {
        self.pending_page_turn = None;
        self.queued_page_turns = None;
        self.page_turn_paint_hold = false;
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        {
            self.page_turn_started_at = None;
        }
    }

    pub(in crate::app) fn clear_queued_page_turns(&mut self) {
        self.queued_page_turns = None;
    }

    pub(in crate::app) fn clear_pending_sibling_book_turns(&mut self) {
        self.queued_sibling_book_turns.clear();
        self.sibling_book_wgpu_present_wait = None;
        self.sibling_book_visual_hold_until = None;
    }

    pub(in crate::app) fn clear_queued_sibling_book_turns(&mut self) {
        self.queued_sibling_book_turns.clear();
    }

    pub(in crate::app) fn mark_current_book_visual_painted(&mut self) {
        self.sibling_book_visual_pending = false;
        self.sibling_book_wgpu_present_wait = None;
        self.sibling_book_visual_hold_until = None;
    }

    pub(in crate::app) fn mark_current_book_visual_painted_with_hold(&mut self, hold: Duration) {
        self.sibling_book_visual_pending = false;
        self.sibling_book_wgpu_present_wait = None;
        self.sibling_book_visual_hold_until = Some(Instant::now() + hold);
    }

    fn request_pending_page_turn_work(&mut self) {
        let Some(pending) = self.pending_page_turn else {
            return;
        };
        self.worker.set_page(
            worker_center_page_for_mode(pending.target, self.view_mode),
            pending.direction,
            self.target_long_edge,
            self.queued_worker_visible_page_count(),
            self.worker_options(),
        );
    }

    fn queued_worker_visible_page_count(&self) -> usize {
        self.visible_page_count()
            .saturating_add(
                self.queued_page_turns
                    .map(|queued| queued.remaining)
                    .unwrap_or(0),
            )
            .clamp(1, MAX_QUEUED_WORKER_VISIBLE_PAGES)
    }

    fn page_turn_target_ready(&self, target: usize) -> bool {
        let indices = self.spread_indices_for(target);
        if indices.is_empty() {
            return false;
        }
        indices.iter().all(|index| {
            let Some(key) = self.page_key_at(*index, self.target_long_edge) else {
                return false;
            };
            self.page_errors.contains_key(&key) || self.final_quality_page_key(key).is_some()
        })
    }

    fn commit_page_turn(&mut self, target: usize, direction: NavigationDirection) {
        let previous_indices = self.spread_indices_for(self.current_page);
        self.current_page = target;
        self.last_nav_direction = direction;
        self.pan = Vec2::ZERO;

        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        if self
            .page_turn_started_at
            .as_ref()
            .is_some_and(|(page, _started)| *page == target)
        {
            let (_page, started) = self
                .page_turn_started_at
                .take()
                .expect("checked pending page turn");
            perf::record_page_turn_ready(
                started,
                perf::PageCacheState::Miss,
                target,
                self.target_long_edge,
            );
        }

        let transition_style = self.active_page_transition_style();
        if transition_style != PageTransitionStyle::None {
            self.transition = Some(Transition {
                from_indices: previous_indices,
                target_long_edge: self.target_long_edge,
                started_at: Instant::now(),
                screen_sign: transition_screen_sign(self.reading_direction, direction),
                style: transition_style,
            });
        } else {
            self.transition = None;
        }

        self.worker.set_page(
            self.worker_center_page(),
            direction,
            self.target_long_edge,
            self.queued_worker_visible_page_count(),
            self.worker_options(),
        );
        self.persist_reading_position_deferred();
    }

    pub(in crate::app) fn active_page_transition_style(&self) -> PageTransitionStyle {
        self.settings.effective_page_transition_style()
    }

    /// Record that `manual_zoom` was just changed by an interactive gesture.
    /// Every user-driven zoom funnels through [`adjust_zoom`] or
    /// [`adjust_zoom_by_delta`], so calling this from both covers keyboard,
    /// wheel/pinch, top-bar buttons, and the context menu. Programmatic resets
    /// (fit-mode changes, book-open restore) deliberately do not call it.
    pub(in crate::app) fn note_zoom_motion(&mut self) {
        self.last_zoom_motion = Some(Instant::now());
    }

    /// Whether a zoom gesture is still in motion (the last interactive change was
    /// within [`ZOOM_SETTLE_MS`]). While true, the render side substitutes the
    /// cached hardware-mipmap downscale for the per-frame quality downscale.
    pub(in crate::app) fn zoom_in_motion(&self) -> bool {
        zoom_motion_active(self.last_zoom_motion, Instant::now())
    }

    /// Remaining time in the current settle window, or `None` once it has
    /// elapsed. The update loop schedules a repaint after this so the quality
    /// re-render fires automatically when the gesture stops without more input.
    pub(in crate::app) fn zoom_settle_repaint_delay(&self) -> Option<Duration> {
        zoom_settle_remaining(self.last_zoom_motion, Instant::now())
    }

    pub(in crate::app) fn adjust_zoom(&mut self, factor: f32) {
        self.note_zoom_motion();
        let previous_decode = self.decode_options();
        let previous_intent = self.current_prepared_target_intent();
        self.clear_pending_page_turns();
        self.fit_mode = FitMode::Manual;
        self.manual_zoom = (self.manual_zoom * factor).clamp(MIN_MANUAL_ZOOM, MAX_MANUAL_ZOOM);
        self.schedule_high_target_cleanup_if_leaving_target_intent(previous_intent);
        self.request_page_if_decode_or_target_intent_changed(previous_decode, previous_intent);
        self.persist_reading_position();
    }

    pub(in crate::app) fn adjust_zoom_by_delta(&mut self, delta: f32) {
        self.note_zoom_motion();
        let previous_decode = self.decode_options();
        let previous_intent = self.current_prepared_target_intent();
        self.clear_pending_page_turns();
        self.fit_mode = FitMode::Manual;
        self.manual_zoom = (self.manual_zoom + delta).clamp(MIN_MANUAL_ZOOM, MAX_MANUAL_ZOOM);
        self.schedule_high_target_cleanup_if_leaving_target_intent(previous_intent);
        self.request_page_if_decode_or_target_intent_changed(previous_decode, previous_intent);
        self.persist_reading_position();
    }

    pub(in crate::app) fn set_fit_mode(&mut self, mode: FitMode) {
        // The strip lays out fit-width only; any other fit accepted here would
        // just skew the decode targets and lie on the toolbar indicator while
        // the layout keeps rendering fit-width. (Entering the strip sets
        // FitWidth through this same path, so that must stay allowed.)
        if self.view_mode == ViewMode::VerticalStrip && mode != FitMode::FitWidth {
            self.notify_strip_fit_locked();
            return;
        }
        let previous_decode = self.decode_options();
        let previous_intent = self.current_prepared_target_intent();
        self.clear_pending_page_turns();
        self.fit_mode = mode;
        if mode == FitMode::Original {
            self.manual_zoom = 1.0;
        }
        self.schedule_high_target_cleanup_if_leaving_target_intent(previous_intent);
        self.request_page_if_decode_or_target_intent_changed(previous_decode, previous_intent);
        self.persist_reading_position();
    }

    fn request_page_if_decode_or_target_intent_changed(
        &mut self,
        previous_decode: DecodeOptions,
        previous_intent: crate::core::worker::PreparedTargetIntent,
    ) {
        if self.source.is_some()
            && (previous_decode != self.decode_options()
                || previous_intent != self.current_prepared_target_intent())
        {
            self.worker.set_page(
                self.worker_center_page(),
                self.last_nav_direction,
                self.target_long_edge,
                self.visible_page_count(),
                self.worker_options(),
            );
        }
    }

    pub(in crate::app) fn set_double_mode(&mut self, direction: ReadingDirection) {
        self.clear_pending_page_turns();
        self.view_mode = match direction {
            ReadingDirection::LeftToRight => ViewMode::DoubleLeftToRight,
            ReadingDirection::RightToLeft => ViewMode::DoubleRightToLeft,
        };
        self.reading_direction = direction;
        self.worker.set_page(
            self.worker_center_page(),
            self.last_nav_direction,
            self.target_long_edge,
            self.visible_page_count(),
            self.worker_options(),
        );
        self.persist_reading_position();
    }

    pub(in crate::app) fn toggle_double_mode(&mut self) {
        self.clear_pending_page_turns();
        self.view_mode = match self.view_mode {
            // From strip, toggling leaves into the double mode for the current
            // reading direction (same as toggling up from Single).
            ViewMode::Single | ViewMode::VerticalStrip => match self.reading_direction {
                ReadingDirection::LeftToRight => ViewMode::DoubleLeftToRight,
                ReadingDirection::RightToLeft => ViewMode::DoubleRightToLeft,
            },
            ViewMode::DoubleLeftToRight
            | ViewMode::DoubleRightToLeft
            | ViewMode::SmartDoubleLeftToRight
            | ViewMode::SmartDoubleRightToLeft => ViewMode::Single,
        };
        if let Some(direction) = self.view_mode.reading_direction() {
            self.reading_direction = direction;
        }
        self.worker.set_page(
            self.worker_center_page(),
            self.last_nav_direction,
            self.target_long_edge,
            self.visible_page_count(),
            self.worker_options(),
        );
    }

    pub(in crate::app) fn update_effects(&mut self, update: impl FnOnce(&mut ViewEffects)) {
        update(&mut self.effects);
        self.textures.clear();
        self.request_original_texture_only_decode_if_needed();
        self.set_status(self.effect_status());
    }

    fn effect_status(&self) -> String {
        let i18n = self.i18n();
        let mut parts = vec![self.effects.filter.label_i18n(i18n)];
        if self.effects.gamma {
            parts.push(i18n.text("status.effect.gamma"));
        }
        if self.effects.invert_colors {
            parts.push(i18n.text("status.effect.inverted"));
        }

        let transform = self.effects.transform;
        if transform.rotation_quadrants != 0 {
            parts.push(i18n.with_vars(
                "status.effect.rotation",
                &[(
                    "degrees",
                    (u16::from(transform.rotation_quadrants % 4) * 90).to_string(),
                )],
            ));
        }
        if transform.flip_horizontal {
            parts.push(i18n.text("status.effect.flip_h"));
        }
        if transform.flip_vertical {
            parts.push(i18n.text("status.effect.flip_v"));
        }
        parts.join(", ")
    }

    pub(in crate::app) fn open_sibling_book(&mut self, direction: isize) {
        let direction = normalize_sibling_book_direction(direction);
        if self.should_queue_sibling_book_turn() {
            self.queue_sibling_book_turn(direction);
            return;
        }
        self.open_sibling_book_now(direction);
    }

    fn should_queue_sibling_book_turn(&self) -> bool {
        !self.queued_sibling_book_turns.is_empty()
            || self.loader_pending
            || self.sibling_book_visual_pending
    }

    fn sibling_book_turn_in_progress(&self) -> bool {
        self.loader_pending || self.sibling_book_visual_pending
    }

    pub(in crate::app) fn sibling_book_hold_active(&self) -> bool {
        self.sibling_book_visual_hold_until
            .is_some_and(|until| Instant::now() < until)
    }

    pub(in crate::app) fn sibling_book_transition_stabilizing(&self) -> bool {
        self.loader_pending
            || self.sibling_book_visual_pending
            || self.sibling_book_hold_active()
            || !self.queued_sibling_book_turns.is_empty()
    }

    fn queue_sibling_book_turn(&mut self, direction: isize) {
        self.edge_prompt = None;
        push_queued_sibling_book_turn(&mut self.queued_sibling_book_turns, direction);
        self.egui_ctx
            .request_repaint_after(SIBLING_BOOK_TURN_REPAINT_DELAY);
    }

    pub(in crate::app) fn drive_queued_sibling_book_turn(&mut self, ctx: &egui::Context) {
        if self.queued_sibling_book_turns.is_empty() {
            return;
        }
        if self.sibling_book_turn_in_progress() {
            ctx.request_repaint_after(SIBLING_BOOK_TURN_REPAINT_DELAY);
            return;
        }
        let Some(direction) = self.queued_sibling_book_turns.pop_front() else {
            return;
        };
        self.open_sibling_book_now(direction);
        if !self.queued_sibling_book_turns.is_empty() || self.sibling_book_turn_in_progress() {
            ctx.request_repaint_after(SIBLING_BOOK_TURN_REPAINT_DELAY);
        }
    }

    fn open_sibling_book_now(&mut self, direction: isize) {
        let Some(current) = self.current_book_reference_path() else {
            self.set_status(self.i18n().text("status.no_current_book"));
            return;
        };
        if perf::adjacent_seed_prefetch_enabled() {
            if let Some(cache) = self.take_adjacent_seed_for_direction(direction) {
                self.install_adjacent_seed_cache(
                    cache,
                    navigation_direction_for_sibling(direction),
                    self.open_view_fallback(),
                    None,
                );
                return;
            }
            perf::record_adjacent_seed_prefetch_hit(false, self.target_long_edge);
        }
        let Some(next) = sibling_book_path(&current, direction) else {
            self.set_status(self.i18n().text("status.no_sibling_book"));
            return;
        };
        self.sibling_open_retry = Some(SiblingOpenRetry {
            direction,
            attempts_left: SIBLING_OPEN_RETRY_LIMIT,
            origin_book: self.current_book_reference_path(),
        });
        self.open_sibling_path_with_initial_direction(
            next,
            navigation_direction_for_sibling(direction),
        );
    }

    pub(in crate::app) fn current_book_reference_path(&self) -> Option<PathBuf> {
        let source = self.source.as_ref()?;
        match self.open_origin? {
            OpenOrigin::ZipCbz => Some(source.source_path().to_path_buf()),
            OpenOrigin::Folder | OpenOrigin::SingleImage => {
                Some(source.source_path().to_path_buf())
            }
        }
    }
}

/// Whether a zoom gesture recorded at `last` is still in motion at `now`
/// (within the [`ZOOM_SETTLE_MS`] window). `None` (no motion recorded) is not
/// in motion. Pure for testing.
fn zoom_motion_active(last: Option<Instant>, now: Instant) -> bool {
    zoom_settle_remaining(last, now).is_some()
}

/// Time left in the settle window for a motion recorded at `last`, or `None`
/// once the window has elapsed (or nothing was recorded).
fn zoom_settle_remaining(last: Option<Instant>, now: Instant) -> Option<Duration> {
    let settle = Duration::from_millis(ZOOM_SETTLE_MS);
    let elapsed = now.saturating_duration_since(last?);
    (elapsed < settle).then(|| settle - elapsed)
}

fn random_offset(max: usize) -> usize {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as usize)
        .unwrap_or(1);
    nanos % max + 1
}

fn navigation_direction_for_sibling(direction: isize) -> NavigationDirection {
    if direction < 0 {
        NavigationDirection::Backward
    } else {
        NavigationDirection::Forward
    }
}

fn normalize_sibling_book_direction(direction: isize) -> isize {
    if direction < 0 {
        -1
    } else {
        1
    }
}

fn should_open_edge_prompt(current: Option<EdgePrompt>, direction: NavigationDirection) -> bool {
    !current.is_some_and(|prompt| prompt.direction == direction)
}

fn push_queued_page_turn(
    queue: &mut Option<super::QueuedPageTurns>,
    direction: NavigationDirection,
) {
    match queue.as_mut() {
        Some(queued) if queued.direction == direction => {
            queued.remaining = queued
                .remaining
                .saturating_add(1)
                .min(MAX_QUEUED_PAGE_TURNS);
        }
        Some(queued) if queued.remaining > 1 => {
            queued.remaining -= 1;
        }
        Some(_) => {
            *queue = None;
        }
        None => {
            *queue = Some(super::QueuedPageTurns {
                direction,
                remaining: 1,
            });
        }
    }
}

fn push_queued_sibling_book_turn(queue: &mut std::collections::VecDeque<isize>, direction: isize) {
    if queue.len() >= MAX_QUEUED_SIBLING_BOOK_TURNS {
        return;
    }
    queue.push_back(normalize_sibling_book_direction(direction));
}

/// Walks `step` from `start` in `direction` until `exists(candidate)` holds,
/// bounded by `max_steps`. Returns the first existing candidate, or `None`
/// when the boundary (`step` returns `None`) or the bound is reached.
fn skip_missing_target(
    start: usize,
    direction: NavigationDirection,
    exists: impl Fn(usize) -> bool,
    step: impl Fn(usize, NavigationDirection) -> Option<usize>,
    max_steps: usize,
) -> Option<usize> {
    let mut current = start;
    let mut examined = 0;
    loop {
        if exists(current) {
            return Some(current);
        }
        if examined >= max_steps {
            return None;
        }
        current = step(current, direction)?;
        examined += 1;
    }
}

#[cfg(test)]
mod tests;
