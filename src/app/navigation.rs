use super::{
    edge_prompt_button, perf, sibling_book_path, transition_screen_sign, ui, OpenOrigin,
    SuiSuiViewApp, Transition, ViewMode,
};
use crate::core::effects::{transform_status_suffix, ViewEffects};
use crate::core::state::{
    CommandId, EdgePageAction, FitMode, PageTransitionStyle, ReadingDirection,
};
use crate::core::worker::{DecodeOptions, NavigationDirection};
use eframe::egui::{self, Color32, Pos2, RichText, Stroke, Vec2};
use std::path::PathBuf;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app) struct EdgePrompt {
    pub(in crate::app) direction: NavigationDirection,
}

impl SuiSuiViewApp {
    pub(in crate::app) fn next_page(&mut self) {
        let Some(source) = self.source.as_ref() else {
            return;
        };
        let max_page = source.page_count().saturating_sub(1);
        let target = if self.view_mode.is_smart() {
            let indices = self.spread_indices_for_unordered(self.current_page);
            let last = indices.last().copied().unwrap_or(self.current_page);
            if last >= max_page {
                self.handle_edge_page(NavigationDirection::Forward);
                return;
            }
            last.saturating_add(1).min(max_page)
        } else {
            if self.current_page >= max_page {
                self.handle_edge_page(NavigationDirection::Forward);
                return;
            }
            self.current_page
                .saturating_add(self.view_mode.step())
                .min(max_page)
        };
        self.set_page(target, NavigationDirection::Forward);
    }

    pub(in crate::app) fn previous_page(&mut self) {
        let target = if self.view_mode.is_smart() {
            let indices = self.spread_indices_for_unordered(self.current_page);
            let first = indices.first().copied().unwrap_or(self.current_page);
            if first == 0 {
                self.handle_edge_page(NavigationDirection::Backward);
                return;
            }
            first.saturating_sub(1)
        } else {
            if self.current_page == 0 {
                self.handle_edge_page(NavigationDirection::Backward);
                return;
            }
            let step = self.view_mode.step();
            self.current_page.saturating_sub(step)
        };
        self.set_page(target, NavigationDirection::Backward);
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
                            let title = match prompt.direction {
                                NavigationDirection::Forward => "마지막 이미지입니다.",
                                NavigationDirection::Backward => "첫 번째 이미지입니다.",
                            };
                            ui.label(
                                RichText::new(title)
                                    .size(24.0)
                                    .color(ui::theme::TEXT_PRIMARY)
                                    .strong(),
                            );
                            ui.add_space(18.0);
                            ui.horizontal_centered(|ui| {
                                let previous_label = self
                                    .edge_action_button_text("이전 파일", CommandId::PreviousBook);
                                if edge_prompt_button(ui, &previous_label).clicked() {
                                    self.edge_prompt = None;
                                    self.open_sibling_book(-1);
                                }

                                let next_label =
                                    self.edge_action_button_text("다음 파일", CommandId::NextBook);
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
        let previous_indices = self.spread_indices_for(self.current_page);
        self.current_page = target;
        self.last_nav_direction = direction;
        self.pan = Vec2::ZERO;
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
            self.visible_page_count(),
            self.worker_options(),
        );
        self.persist_current_bookmark_deferred();
        self.refresh_ai_prefetch_queue();
    }

    pub(in crate::app) fn active_page_transition_style(&self) -> PageTransitionStyle {
        self.settings.effective_page_transition_style()
    }

    pub(in crate::app) fn adjust_zoom(&mut self, factor: f32) {
        let previous_decode = self.decode_options();
        self.fit_mode = FitMode::Manual;
        self.manual_zoom = (self.manual_zoom * factor).clamp(0.1, 8.0);
        self.request_page_if_decode_changed(previous_decode);
        self.persist_current_bookmark();
    }

    pub(in crate::app) fn adjust_zoom_by_delta(&mut self, delta: f32) {
        let previous_decode = self.decode_options();
        self.fit_mode = FitMode::Manual;
        self.manual_zoom = (self.manual_zoom + delta).clamp(0.1, 8.0);
        self.request_page_if_decode_changed(previous_decode);
        self.persist_current_bookmark();
    }

    pub(in crate::app) fn set_fit_mode(&mut self, mode: FitMode) {
        let previous_decode = self.decode_options();
        self.fit_mode = mode;
        if mode == FitMode::Original {
            self.manual_zoom = 1.0;
        }
        self.request_page_if_decode_changed(previous_decode);
        self.persist_current_bookmark();
    }

    pub(in crate::app) fn request_page_if_decode_changed(
        &mut self,
        previous_decode: DecodeOptions,
    ) {
        if self.source.is_some() && previous_decode != self.decode_options() {
            self.worker.set_page(
                self.worker_center_page(),
                self.last_nav_direction,
                self.target_long_edge,
                self.visible_page_count(),
                self.worker_options(),
            );
            self.refresh_ai_prefetch_queue();
        }
    }

    pub(in crate::app) fn set_double_mode(&mut self, direction: ReadingDirection) {
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
        self.refresh_ai_prefetch_queue();
    }

    pub(in crate::app) fn toggle_double_mode(&mut self) {
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
        self.refresh_ai_prefetch_queue();
    }

    pub(in crate::app) fn set_use_ai_upscaled_pages(&mut self, enabled: bool) {
        if self.use_ai_upscaled_pages == enabled {
            return;
        }
        self.use_ai_upscaled_pages = enabled;
        if enabled {
            self.set_status("AI 업스케일 결과를 기본 표시에서 우선 사용합니다.");
        } else {
            self.set_status("AI 업스케일 결과를 숨기고 일반 표시를 사용합니다.");
        }
    }

    pub(in crate::app) fn update_effects(&mut self, update: impl FnOnce(&mut ViewEffects)) {
        update(&mut self.effects);
        self.textures.clear();
        self.set_status(format!(
            "{}{}{}{}",
            self.effects.filter.label(),
            if self.effects.gamma { ", gamma" } else { "" },
            if self.effects.invert_colors {
                ", inverted"
            } else {
                ""
            },
            transform_status_suffix(self.effects.transform)
        ));
    }

    pub(in crate::app) fn open_sibling_book(&mut self, direction: isize) {
        let Some(current) = self.current_book_reference_path() else {
            self.set_status("No current book to move from.");
            return;
        };
        if perf::adjacent_seed_prefetch_enabled() {
            if let Some(cache) = self.take_adjacent_seed_for_direction(direction) {
                perf::record_adjacent_seed_prefetch_hit(true, cache.target_long_edge);
                self.pending_bookmark_jump = None;
                self.loader_generation = self.loader_generation.wrapping_add(1);
                self.clear_adjacent_seed_cache();
                #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
                {
                    self.open_to_first_visible_trace = Some(perf::OpenToFirstVisibleTrace::new(
                        cache.origin.perf_label(),
                    ));
                }
                self.install_source(
                    cache.source,
                    cache.forced_page,
                    cache.origin,
                    cache.path,
                    Some(cache.seeded_page),
                );
                return;
            }
            perf::record_adjacent_seed_prefetch_hit(false, self.target_long_edge);
        }
        let Some(next) = sibling_book_path(&current, direction) else {
            self.set_status("No sibling folder, ZIP, or CBZ found.");
            return;
        };
        self.open_path(next);
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
