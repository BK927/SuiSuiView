use super::{SuiSuiViewApp, STATE_SAVE_DEBOUNCE};
use crate::core::state::WindowPlacement;
use eframe::egui::{self, Pos2, Rect, Vec2};
use std::time::{Duration, Instant};

const POSITION_EDGE_PADDING: f32 = 16.0;
const MIN_VISIBLE_EDGE: f32 = 80.0;
const SCALE_CHANGE_EPSILON: f32 = 0.01;
const SIZE_SAVE_BLOCK_AFTER_SCALE_CHANGE: Duration = Duration::from_millis(750);

impl SuiSuiViewApp {
    pub(in crate::app) fn maintain_native_window_state(&mut self, ctx: &egui::Context) {
        self.sync_viewport_flags(ctx);
        self.ensure_window_position_visible(ctx);
        self.persist_window_placement_deferred(ctx);
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
        let Some(scale) = valid_scale(scale) else {
            return;
        };
        if scale_changed(self.window_last_native_pixels_per_point, Some(scale)) {
            self.window_size_save_block_until = Some(now + SIZE_SAVE_BLOCK_AFTER_SCALE_CHANGE);
        }
        self.window_last_native_pixels_per_point = Some(scale);
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
    use super::{clamped_window_position, persistent_inner_size, scale_changed};
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
}
