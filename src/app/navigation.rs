use super::{
    edge_prompt_button, perf, sibling_book_path, transition_screen_sign, ui,
    worker_center_page_for_mode, OpenOrigin, PageCacheKey, SuiSuiViewApp, Transition, ViewMode,
    SIBLING_BOOK_TURN_REPAINT_DELAY,
};
use crate::core::effects::ViewEffects;
use crate::core::state::{
    CommandId, EdgePageAction, FitMode, PageTransitionStyle, ReadingDirection,
};
use crate::core::worker::{DecodeOptions, NavigationDirection};
use eframe::egui::{self, Color32, Pos2, RichText, Stroke, Vec2};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const MAX_QUEUED_PAGE_TURNS: usize = 128;
const MAX_QUEUED_WORKER_VISIBLE_PAGES: usize = 25;
const MAX_QUEUED_SIBLING_BOOK_TURNS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app) struct EdgePrompt {
    pub(in crate::app) direction: NavigationDirection,
}

impl SuiSuiViewApp {
    pub(in crate::app) fn next_page(&mut self) {
        let direction = NavigationDirection::Forward;
        if let Some(target) = self.page_turn_target(direction) {
            self.set_page(target, direction);
        } else {
            self.handle_edge_page(direction);
        }
    }

    pub(in crate::app) fn previous_page(&mut self) {
        let direction = NavigationDirection::Backward;
        if let Some(target) = self.page_turn_target(direction) {
            self.set_page(target, direction);
        } else {
            self.handle_edge_page(direction);
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
        self.set_page(target, direction);
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
        self.edge_prompt = Some(EdgePrompt { direction });
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

    pub(in crate::app) fn show_edge_prompt(&mut self, ctx: &egui::Context) {
        let Some(prompt) = self.edge_prompt else {
            return;
        };
        if self.source.is_none() {
            self.edge_prompt = None;
            return;
        }
        if self.settings_open || self.about_open || self.bookmark_popover_open {
            self.edge_prompt = None;
            return;
        }

        let screen = ctx.screen_rect();
        let available_width = (screen.width() - 32.0).max(280.0);
        let width = available_width.min(560.0).max(available_width.min(360.0));
        let pos = Pos2::new(
            screen.center().x - width * 0.5,
            (screen.bottom() - 164.0).max(screen.top() + 80.0),
        );
        let response = egui::Area::new("edge_page_prompt".into())
            .order(egui::Order::Foreground)
            .fixed_pos(pos)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(Color32::from_rgb(8, 9, 11))
                    .stroke(Stroke::new(1.2, ui::theme::SUBTLE_STROKE))
                    .corner_radius(egui::CornerRadius::same(14))
                    .shadow(egui::epaint::Shadow {
                        offset: [0, 10],
                        blur: 22,
                        spread: 0,
                        color: Color32::from_black_alpha(150),
                    })
                    .inner_margin(egui::Margin::symmetric(22, 20))
                    .show(ui, |ui| {
                        ui.set_width(width - 44.0);
                        ui.vertical_centered(|ui| {
                            let i18n = self.i18n();
                            let title = match prompt.direction {
                                NavigationDirection::Forward => i18n.text("navigation.edge.last"),
                                NavigationDirection::Backward => i18n.text("navigation.edge.first"),
                            };
                            ui.label(
                                RichText::new(title)
                                    .size(24.0)
                                    .color(ui::theme::TEXT_PRIMARY)
                                    .strong(),
                            );
                            ui.add_space(18.0);
                            ui.horizontal_centered(|ui| {
                                let previous_file = i18n.text("navigation.edge.previous_file");
                                let previous_label = self.edge_action_button_text(
                                    &previous_file,
                                    CommandId::PreviousBook,
                                );
                                if edge_prompt_button(ui, &previous_label).clicked() {
                                    self.edge_prompt = None;
                                    self.open_sibling_book(-1);
                                }

                                let next_file = i18n.text("navigation.edge.next_file");
                                let next_label =
                                    self.edge_action_button_text(&next_file, CommandId::NextBook);
                                if edge_prompt_button(ui, &next_label).clicked() {
                                    self.edge_prompt = None;
                                    self.open_sibling_book(1);
                                }
                            });
                        });
                    });
            });

        let prompt_rect = response.response.rect;
        let clicked_outside = ctx.input(|input| {
            input.pointer.any_click()
                && input
                    .pointer
                    .interact_pos()
                    .is_some_and(|pos| !prompt_rect.contains(pos))
        });
        if clicked_outside {
            self.edge_prompt = None;
        }
    }

    pub(in crate::app) fn edge_action_button_text(
        &self,
        label: &str,
        command: CommandId,
    ) -> String {
        self.shortcut_hint_for_command(command).map_or_else(
            || label.to_owned(),
            |shortcut| format!("{label} ({shortcut})"),
        )
    }

    pub(in crate::app) fn shortcut_hint_for_command(&self, command: CommandId) -> Option<String> {
        self.settings
            .key_bindings
            .iter()
            .find(|binding| binding.command == command)
            .map(|binding| binding.shortcut.label())
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
        let cache_state = {
            let requested_key = PageCacheKey {
                index: target,
                target_long_edge: self.target_long_edge,
                decode: self.decode_options(),
            };
            self.page_turn_cache_state(requested_key)
        };
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

        match self.queued_page_turns.as_mut() {
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
                self.queued_page_turns = None;
            }
            None => {
                self.queued_page_turns = Some(super::QueuedPageTurns {
                    direction,
                    remaining: 1,
                });
            }
        }
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

    pub(in crate::app) fn clear_pending_sibling_book_turns(&mut self) {
        self.queued_sibling_book_turns.clear();
        self.sibling_book_wgpu_present_wait = None;
        self.sibling_book_visual_hold_until = None;
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
        let decode = self.decode_options();
        indices.iter().all(|index| {
            let key = PageCacheKey {
                index: *index,
                target_long_edge: self.target_long_edge,
                decode,
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
        self.persist_current_bookmark_deferred();
    }

    pub(in crate::app) fn active_page_transition_style(&self) -> PageTransitionStyle {
        self.settings.effective_page_transition_style()
    }

    pub(in crate::app) fn adjust_zoom(&mut self, factor: f32) {
        let previous_decode = self.decode_options();
        let previous_intent = self.current_prepared_target_intent();
        self.clear_pending_page_turns();
        self.fit_mode = FitMode::Manual;
        self.manual_zoom = (self.manual_zoom * factor).clamp(0.1, 8.0);
        self.schedule_high_target_cleanup_if_leaving_target_intent(previous_intent);
        self.request_page_if_decode_or_target_intent_changed(previous_decode, previous_intent);
        self.persist_current_bookmark();
    }

    pub(in crate::app) fn adjust_zoom_by_delta(&mut self, delta: f32) {
        let previous_decode = self.decode_options();
        let previous_intent = self.current_prepared_target_intent();
        self.clear_pending_page_turns();
        self.fit_mode = FitMode::Manual;
        self.manual_zoom = (self.manual_zoom + delta).clamp(0.1, 8.0);
        self.schedule_high_target_cleanup_if_leaving_target_intent(previous_intent);
        self.request_page_if_decode_or_target_intent_changed(previous_decode, previous_intent);
        self.persist_current_bookmark();
    }

    pub(in crate::app) fn set_fit_mode(&mut self, mode: FitMode) {
        let previous_decode = self.decode_options();
        let previous_intent = self.current_prepared_target_intent();
        self.clear_pending_page_turns();
        self.fit_mode = mode;
        if mode == FitMode::Original {
            self.manual_zoom = 1.0;
        }
        self.schedule_high_target_cleanup_if_leaving_target_intent(previous_intent);
        self.request_page_if_decode_or_target_intent_changed(previous_decode, previous_intent);
        self.persist_current_bookmark();
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
        self.persist_current_bookmark();
    }

    pub(in crate::app) fn toggle_double_mode(&mut self) {
        self.clear_pending_page_turns();
        self.view_mode = match self.view_mode {
            ViewMode::Single => match self.reading_direction {
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
                let target_long_edge = cache.target_long_edge;
                let origin = cache.origin;
                let source = cache.source;
                let forced_page = cache.forced_page;
                let path = cache.path;
                let seeded_page = cache.seeded_page;
                let seeded_followup_page = cache.seeded_followup_page;
                let view_fallback = Some(self.open_view_fallback());

                perf::record_adjacent_seed_prefetch_hit(true, target_long_edge);
                self.pending_bookmark_jump = None;
                self.loader_generation = self.loader_generation.wrapping_add(1);
                self.clear_adjacent_seed_cache();
                #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
                {
                    self.open_to_first_visible_trace =
                        Some(perf::OpenToFirstVisibleTrace::new(origin.perf_label()));
                }
                self.install_source(
                    source,
                    forced_page,
                    origin,
                    path,
                    Some(seeded_page),
                    navigation_direction_for_sibling(direction),
                    view_fallback,
                    None,
                );
                self.insert_seeded_page_if_matching_target(seeded_followup_page);
                return;
            }
            perf::record_adjacent_seed_prefetch_hit(false, self.target_long_edge);
        }
        let Some(next) = sibling_book_path(&current, direction) else {
            self.set_status(self.i18n().text("status.no_sibling_book"));
            return;
        };
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

fn push_queued_sibling_book_turn(queue: &mut std::collections::VecDeque<isize>, direction: isize) {
    if queue.len() >= MAX_QUEUED_SIBLING_BOOK_TURNS {
        return;
    }
    queue.push_back(normalize_sibling_book_direction(direction));
}

#[cfg(test)]
mod tests {
    use super::{
        normalize_sibling_book_direction, push_queued_sibling_book_turn,
        MAX_QUEUED_SIBLING_BOOK_TURNS,
    };
    use std::collections::VecDeque;

    #[test]
    fn sibling_book_direction_normalizes_to_step() {
        assert_eq!(normalize_sibling_book_direction(-4), -1);
        assert_eq!(normalize_sibling_book_direction(0), 1);
        assert_eq!(normalize_sibling_book_direction(3), 1);
    }

    #[test]
    fn queued_sibling_book_turns_preserve_mixed_order() {
        let mut queue = VecDeque::new();

        push_queued_sibling_book_turn(&mut queue, 1);
        push_queued_sibling_book_turn(&mut queue, -1);
        push_queued_sibling_book_turn(&mut queue, 1);

        assert_eq!(queue.into_iter().collect::<Vec<_>>(), vec![1, -1, 1]);
    }

    #[test]
    fn queued_sibling_book_turns_are_capped() {
        let mut queue = VecDeque::new();

        for _ in 0..MAX_QUEUED_SIBLING_BOOK_TURNS + 8 {
            push_queued_sibling_book_turn(&mut queue, 1);
        }

        assert_eq!(queue.len(), MAX_QUEUED_SIBLING_BOOK_TURNS);
    }
}
