use super::{SuiSuiViewApp, STATE_SAVE_DEBOUNCE};
use crate::core::state::WindowPlacement;
use eframe::egui::{self, Pos2, Rect, Vec2};
use std::time::{Duration, Instant};

const POSITION_EDGE_PADDING: f32 = 16.0;
const MIN_VISIBLE_EDGE: f32 = 80.0;
const SCALE_CHANGE_EPSILON: f32 = 0.01;
const DPI_ARTIFACT_RATIO_TOLERANCE: f32 = 0.08;
const DPI_ARTIFACT_MIN_DELTA_POINTS: f32 = 48.0;
const DPI_SIZE_GUARD_DURATION: Duration = Duration::from_millis(2_000);
const DPI_VIEW_TARGET_STABILITY_DELAY: Duration = Duration::from_millis(250);

pub(in crate::app) struct WindowDpiSizeGuard {
    previous_scale: f32,
    current_scale: f32,
    stable_size: [f32; 2],
    expires_at: Instant,
}

impl SuiSuiViewApp {
    pub(in crate::app) fn maintain_native_window_state(&mut self, ctx: &egui::Context) {
        if self.startup_reveal_pending {
            return;
        }
        self.sync_viewport_flags(ctx);
        self.ensure_window_position_visible(ctx);
        self.persist_window_placement_deferred(ctx);
    }

    pub(in crate::app) fn reveal_startup_window_after_first_frame(&mut self, ctx: &egui::Context) {
        if take_startup_reveal_request(&mut self.startup_reveal_pending) {
            crate::startup_window::reveal_main_windows();
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        }
    }

    fn sync_viewport_flags(&mut self, ctx: &egui::Context) {
        ctx.input(|input| {
            let viewport = input.viewport();
            if let Some(maximized) = viewport.maximized {
                self.maximized = maximized;
            }
            if let Some(fullscreen) = viewport.fullscreen {
                self.fullscreen = fullscreen;
            }
        });
    }

    fn ensure_window_position_visible(&mut self, ctx: &egui::Context) {
        if self.window_position_checked {
            return;
        }

        let (outer_rect, monitor_size, minimized, maximized, fullscreen) = ctx.input(|input| {
            let viewport = input.viewport();
            (
                viewport.outer_rect,
                viewport.monitor_size,
                viewport.minimized,
                viewport.maximized,
                viewport.fullscreen,
            )
        });
        if minimized == Some(true) || maximized == Some(true) || fullscreen == Some(true) {
            self.window_position_checked = true;
            return;
        }

        let Some(outer_rect) = outer_rect else {
            return;
        };
        let Some(monitor_size) = monitor_size else {
            return;
        };
        if monitor_size.x <= MIN_VISIBLE_EDGE || monitor_size.y <= MIN_VISIBLE_EDGE {
            return;
        }

        if let Some(position) = clamped_window_position(outer_rect, monitor_size) {
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(position));
        }
        self.window_position_checked = true;
    }

    fn persist_window_placement_deferred(&mut self, ctx: &egui::Context) {
        let Some(placement) = self.current_window_placement(ctx) else {
            return;
        };
        if self.store.update_window_placement_deferred(placement) {
            self.pending_state_save_at = Some(Instant::now() + STATE_SAVE_DEBOUNCE);
            self.egui_ctx.request_repaint_after(STATE_SAVE_DEBOUNCE);
        }
    }

    fn current_window_placement(&mut self, ctx: &egui::Context) -> Option<WindowPlacement> {
        let (inner_rect, outer_rect, minimized, maximized, fullscreen, native_pixels_per_point) =
            ctx.input(|input| {
                let viewport = input.viewport();
                (
                    viewport.inner_rect,
                    viewport.outer_rect,
                    viewport.minimized,
                    viewport.maximized,
                    viewport.fullscreen,
                    viewport.native_pixels_per_point,
                )
            });
        if minimized == Some(true) || fullscreen == Some(true) {
            return None;
        }

        let now = Instant::now();
        self.observe_window_scale(native_pixels_per_point, now);

        let mut placement = self.store.window_placement().clone();
        placement.maximized = maximized.unwrap_or(placement.maximized);
        if placement.maximized {
            return Some(placement);
        }

        if let Some(inner_rect) = inner_rect {
            let size = inner_rect.size();
            if size.x.is_finite() && size.y.is_finite() && size.x > 0.0 && size.y > 0.0 {
                self.suspend_dpi_artifact_size_save_if_needed(size, now);
                placement.inner_size = Some(self.inner_size_for_persistence(size, now));
            }
        }
        if let Some(outer_rect) = outer_rect {
            let position = outer_rect.min;
            if position.x.is_finite() && position.y.is_finite() {
                placement.outer_position = Some(round_pos(position));
            }
        }

        placement.inner_size.map(|_| placement)
    }

    fn observe_window_scale(&mut self, scale: Option<f32>, now: Instant) {
        let Some(current_scale) = valid_scale(scale) else {
            return;
        };
        if scale_changed(
            self.window_last_native_pixels_per_point,
            Some(current_scale),
        ) {
            self.window_size_save_block_until = Some(now + DPI_SIZE_GUARD_DURATION);
            self.view_target_update_block_until = Some(now + DPI_VIEW_TARGET_STABILITY_DELAY);
            self.pending_target_long_edge_increase = None;
            self.egui_ctx
                .request_repaint_after(DPI_VIEW_TARGET_STABILITY_DELAY);
            if let Some(previous_scale) = self.window_last_native_pixels_per_point {
                self.window_dpi_size_guard =
                    self.window_stable_inner_size
                        .map(|stable_size| WindowDpiSizeGuard {
                            previous_scale,
                            current_scale,
                            stable_size,
                            expires_at: now + DPI_SIZE_GUARD_DURATION,
                        });
            }
        }
        self.window_last_native_pixels_per_point = Some(current_scale);
    }

    fn suspend_dpi_artifact_size_save_if_needed(&mut self, current_size: Vec2, now: Instant) {
        if self.window_dpi_size_guard.as_ref().is_some_and(|guard| {
            now > guard.expires_at || size_close_to(guard.stable_size, current_size)
        }) {
            self.window_dpi_size_guard = None;
            return;
        }

        let Some(guard) = self.window_dpi_size_guard.as_mut() else {
            return;
        };
        if !looks_like_dpi_size_artifact(
            guard.previous_scale,
            guard.current_scale,
            guard.stable_size,
            current_size,
        ) {
            return;
        }

        self.window_size_save_block_until = Some(guard.expires_at);
    }

    fn inner_size_for_persistence(&mut self, current_size: Vec2, now: Instant) -> [f32; 2] {
        let suspended = self
            .window_size_save_block_until
            .is_some_and(|block_until| now < block_until);
        if !suspended {
            self.window_size_save_block_until = None;
        }
        let (size, stable_size) = persistent_inner_size(
            self.window_stable_inner_size,
            self.store.window_placement().inner_size,
            current_size,
            suspended,
        );
        self.window_stable_inner_size = stable_size;
        size
    }
}

fn round_vec2(value: Vec2) -> [f32; 2] {
    [value.x.round(), value.y.round()]
}

fn round_pos(value: Pos2) -> [f32; 2] {
    [value.x.round(), value.y.round()]
}

fn take_startup_reveal_request(pending: &mut bool) -> bool {
    let should_reveal = *pending;
    *pending = false;
    should_reveal
}

fn valid_scale(scale: Option<f32>) -> Option<f32> {
    scale.filter(|scale| scale.is_finite() && *scale > 0.0)
}

fn scale_changed(previous: Option<f32>, current: Option<f32>) -> bool {
    let (Some(previous), Some(current)) = (valid_scale(previous), valid_scale(current)) else {
        return false;
    };
    (previous - current).abs() > SCALE_CHANGE_EPSILON
}

fn persistent_inner_size(
    stable_size: Option<[f32; 2]>,
    stored_size: Option<[f32; 2]>,
    current_size: Vec2,
    suspended: bool,
) -> ([f32; 2], Option<[f32; 2]>) {
    let current = round_vec2(current_size);
    if suspended {
        return (stable_size.or(stored_size).unwrap_or(current), stable_size);
    }
    (current, Some(current))
}

fn looks_like_dpi_size_artifact(
    previous_scale: f32,
    current_scale: f32,
    stable_size: [f32; 2],
    current_size: Vec2,
) -> bool {
    let (Some(previous_scale), Some(current_scale)) = (
        valid_scale(Some(previous_scale)),
        valid_scale(Some(current_scale)),
    ) else {
        return false;
    };
    let expected_ratio = previous_scale / current_scale;
    if !expected_ratio.is_finite() || (expected_ratio - 1.0).abs() <= SCALE_CHANGE_EPSILON {
        return false;
    }

    let stable = Vec2::new(stable_size[0], stable_size[1]);
    if stable.x <= 0.0 || stable.y <= 0.0 || current_size.x <= 0.0 || current_size.y <= 0.0 {
        return false;
    }

    let delta_x = (current_size.x - stable.x).abs();
    let delta_y = (current_size.y - stable.y).abs();
    if delta_x < DPI_ARTIFACT_MIN_DELTA_POINTS || delta_y < DPI_ARTIFACT_MIN_DELTA_POINTS {
        return false;
    }

    let x_ratio = current_size.x / stable.x;
    let y_ratio = current_size.y / stable.y;
    (x_ratio - expected_ratio).abs() <= DPI_ARTIFACT_RATIO_TOLERANCE
        && (y_ratio - expected_ratio).abs() <= DPI_ARTIFACT_RATIO_TOLERANCE
}

fn size_close_to(stable_size: [f32; 2], current_size: Vec2) -> bool {
    (current_size.x - stable_size[0]).abs() <= 4.0 && (current_size.y - stable_size[1]).abs() <= 4.0
}

fn clamped_window_position(outer_rect: Rect, monitor_size: Vec2) -> Option<Pos2> {
    let mut position = outer_rect.min;
    let max_x =
        (monitor_size.x - outer_rect.width() - POSITION_EDGE_PADDING).max(POSITION_EDGE_PADDING);
    let max_y =
        (monitor_size.y - outer_rect.height() - POSITION_EDGE_PADDING).max(POSITION_EDGE_PADDING);

    if outer_rect.right() < MIN_VISIBLE_EDGE {
        position.x = POSITION_EDGE_PADDING;
    } else if outer_rect.left() > monitor_size.x - MIN_VISIBLE_EDGE {
        position.x = max_x;
    }

    if outer_rect.bottom() < MIN_VISIBLE_EDGE {
        position.y = POSITION_EDGE_PADDING;
    } else if outer_rect.top() > monitor_size.y - MIN_VISIBLE_EDGE {
        position.y = max_y;
    }

    (position != outer_rect.min).then_some(position)
}

#[cfg(test)]
mod tests {
    use super::{
        clamped_window_position, looks_like_dpi_size_artifact, persistent_inner_size,
        scale_changed, size_close_to, take_startup_reveal_request,
    };
    use eframe::egui::{pos2, vec2, Rect};

    #[test]
    fn clamped_window_position_keeps_visible_window() {
        let rect = Rect::from_min_size(pos2(100.0, 120.0), vec2(640.0, 480.0));

        assert_eq!(clamped_window_position(rect, vec2(1920.0, 1080.0)), None);
    }

    #[test]
    fn clamped_window_position_recovers_offscreen_window() {
        let rect = Rect::from_min_size(pos2(2200.0, 1400.0), vec2(640.0, 480.0));

        assert_eq!(
            clamped_window_position(rect, vec2(1920.0, 1080.0)),
            Some(pos2(1264.0, 584.0))
        );
    }

    #[test]
    fn scale_change_is_detected_from_native_pixels_per_point() {
        assert!(scale_changed(Some(1.0), Some(1.5)));
        assert!(!scale_changed(Some(1.0), Some(1.005)));
        assert!(!scale_changed(None, Some(1.5)));
    }

    #[test]
    fn persistent_inner_size_uses_stable_size_while_suspended() {
        assert_eq!(
            persistent_inner_size(
                Some([1200.0, 800.0]),
                Some([1100.0, 700.0]),
                vec2(1800.0, 1200.0),
                true,
            ),
            ([1200.0, 800.0], Some([1200.0, 800.0]))
        );
    }

    #[test]
    fn persistent_inner_size_falls_back_to_stored_size_while_suspended() {
        assert_eq!(
            persistent_inner_size(None, Some([1100.0, 700.0]), vec2(1800.0, 1200.0), true),
            ([1100.0, 700.0], None)
        );
    }

    #[test]
    fn persistent_inner_size_accepts_current_size_when_not_suspended() {
        assert_eq!(
            persistent_inner_size(Some([1200.0, 800.0]), None, vec2(1500.0, 900.0), false),
            ([1500.0, 900.0], Some([1500.0, 900.0]))
        );
    }

    #[test]
    fn startup_reveal_request_is_one_shot() {
        let mut pending = true;

        assert!(take_startup_reveal_request(&mut pending));
        assert!(!pending);
        assert!(!take_startup_reveal_request(&mut pending));
    }

    #[test]
    fn dpi_artifact_detection_matches_high_to_low_scale_growth() {
        assert!(looks_like_dpi_size_artifact(
            1.75,
            1.25,
            [1280.0, 820.0],
            vec2(1792.0, 1148.0)
        ));
    }

    #[test]
    fn dpi_artifact_detection_ignores_ordinary_resize() {
        assert!(!looks_like_dpi_size_artifact(
            1.75,
            1.25,
            [1280.0, 820.0],
            vec2(1380.0, 860.0)
        ));
    }

    #[test]
    fn dpi_artifact_detection_ignores_tiny_scale_delta() {
        assert!(!looks_like_dpi_size_artifact(
            1.25,
            1.255,
            [1280.0, 820.0],
            vec2(1275.0, 817.0)
        ));
    }

    #[test]
    fn stable_size_close_check_allows_small_rounding_drift() {
        assert!(size_close_to([1280.0, 820.0], vec2(1277.0, 823.0)));
        assert!(!size_close_to([1280.0, 820.0], vec2(1270.0, 823.0)));
    }
}
