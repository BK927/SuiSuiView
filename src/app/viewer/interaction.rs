use super::{ViewMode, SPREAD_GAP_POINTS};
use crate::app::commands::command_for_mouse_gesture;
use crate::app::{perf, ui, SuiSuiViewApp};
use crate::core::state::{FitMode, MouseGesture, WheelMode};
use crate::core::worker::{
    clamp_navigation_target_long_edge, clamp_target_long_edge, NavigationDirection,
};
use egui::{self, Align2, Color32, Rect, Vec2};
use std::time::{Duration, Instant};

/// How long the page viewport (page rect x pixels-per-point) must hold steady
/// before an exact display-sized target is applied. During a resize drag the
/// viewport keeps changing, so the timer keeps restarting and the existing
/// prepared target stays frozen (scaled by the sampler); once the drag settles
/// the exact new target is applied in a single re-prepare burst.
const VIEW_TARGET_SETTLE_DELAY: Duration = Duration::from_millis(250);

impl SuiSuiViewApp {
    pub(super) fn paint_page_arrows(
        &mut self,
        ctx: &egui::Context,
        painter: &egui::Painter,
        rect: Rect,
    ) {
        if !self.settings.show_page_arrows || self.source.is_none() {
            return;
        }
        let Some(pointer_pos) = ctx.pointer_hover_pos() else {
            return;
        };
        if !rect.contains(pointer_pos) {
            return;
        }
        let zone_width = rect.width().clamp(56.0, 96.0);
        let side = if pointer_pos.x <= rect.left() + zone_width {
            Some((ui::icons::CHEVRON_LEFT, NavigationDirection::Backward))
        } else if pointer_pos.x >= rect.right() - zone_width {
            Some((ui::icons::CHEVRON_RIGHT, NavigationDirection::Forward))
        } else {
            None
        };
        let Some((icon, direction)) = side else {
            return;
        };
        let button_rect = Rect::from_center_size(
            egui::pos2(
                if direction == NavigationDirection::Backward {
                    rect.left() + 42.0
                } else {
                    rect.right() - 42.0
                },
                rect.center().y,
            ),
            egui::vec2(52.0, 76.0),
        );
        painter.rect_filled(
            button_rect,
            8.0,
            Color32::from_rgba_unmultiplied(18, 20, 24, 180),
        );
        painter.text(
            button_rect.center(),
            Align2::CENTER_CENTER,
            icon,
            ui::icons::icon_font(ui::icons::IconStyle::Regular, 32.0),
            ui::theme::TEXT_PRIMARY,
        );
        let clicked_arrow_zone = ctx.input(|input| {
            input.pointer.primary_released()
                && input
                    .pointer
                    .press_origin()
                    .is_some_and(|origin| button_rect.contains(origin))
        });
        if clicked_arrow_zone {
            match direction {
                NavigationDirection::Forward => self.next_page(),
                NavigationDirection::Backward => self.previous_page(),
            }
        }
    }

    pub(super) fn update_target_long_edge(&mut self, ctx: &egui::Context, viewport: Vec2) {
        if self.source.is_none() {
            return;
        }

        if self.view_target_update_is_blocked(ctx) {
            return;
        }

        let page_viewport = self.page_viewport_for_target(viewport);
        let pixels_per_point = ctx.pixels_per_point();
        let signature = ViewTargetSignature::new(
            self.fit_mode,
            self.view_mode,
            page_viewport,
            pixels_per_point,
        );
        let previous_intent = self.current_prepared_target_intent();
        let next = self.target_long_edge_for(page_viewport, pixels_per_point);
        let next_intent = self.prepared_target_intent_for_target(next);
        if next == self.target_long_edge {
            self.view_target_settle.pending = None;
            self.view_target_settle.applied = Some(signature);
            return;
        }

        // Freeze the target while the page viewport is still in motion (e.g. a
        // resize drag) and only settle to the exact new size once it has held
        // steady for `VIEW_TARGET_SETTLE_DELAY`. The debounce is bypassed when
        // the change is not driven by viewport motion: the first prepare, a fit
        // or view mode switch, and page turns that keep the same viewport but
        // change the source pages.
        let now = Instant::now();
        if !self.view_target_change_applies_immediately(signature) {
            if let Some(remaining) =
                view_target_settle_wait(&mut self.view_target_settle.pending, next, now)
            {
                self.record_view_target_update(ctx, viewport, "settle_wait", next);
                ctx.request_repaint_after(remaining);
                return;
            }
        }

        let leaving_high_target_intent = previous_intent.keeps_exact_prefetch_lightweight()
            && !next_intent.keeps_exact_prefetch_lightweight();
        self.record_view_target_update(ctx, viewport, "apply", next);
        self.target_long_edge = next;
        self.view_target_settle.pending = None;
        self.view_target_settle.applied = Some(signature);
        if leaving_high_target_intent {
            self.schedule_original_inspection_cache_cleanup(ctx);
        }
        self.clear_pending_page_turns();
        self.worker.set_page(
            self.worker_center_page(),
            self.last_nav_direction,
            next,
            self.visible_page_count(),
            self.worker_options(),
        );
        self.request_adjacent_seed_prefetch();
        ctx.request_repaint();
    }

    /// Whether the pending target change should skip the settle debounce and
    /// apply on this frame: the first prepared target (`applied` is `None`), a
    /// fit/view mode change, or a page turn (same viewport, different source
    /// pages) — none of which are the resize-motion case the debounce guards.
    fn view_target_change_applies_immediately(&self, signature: ViewTargetSignature) -> bool {
        match self.view_target_settle.applied {
            None => true,
            Some(applied) => {
                applied.mode != signature.mode || applied.viewport == signature.viewport
            }
        }
    }

    fn record_view_target_update(
        &self,
        ctx: &egui::Context,
        viewport: Vec2,
        reason: &'static str,
        next: u32,
    ) {
        perf::record_target_long_edge_update(
            reason,
            self.current_page,
            self.target_long_edge,
            next,
            viewport.x.round().max(1.0) as u32,
            viewport.y.round().max(1.0) as u32,
            (ctx.pixels_per_point() * 1000.0).round().max(1.0) as u32,
        );
    }

    fn view_target_update_is_blocked(&mut self, ctx: &egui::Context) -> bool {
        let Some(block_until) = self.view_target_update_block_until else {
            return false;
        };
        let now = Instant::now();
        if now < block_until {
            ctx.request_repaint_after(block_until.saturating_duration_since(now));
            return true;
        }
        self.view_target_update_block_until = None;
        false
    }

    fn page_viewport_for_target(&self, viewport: Vec2) -> Vec2 {
        if self.page_viewport_count_for_target() <= 1 {
            viewport
        } else {
            Vec2::new((viewport.x - SPREAD_GAP_POINTS).max(1.0) * 0.5, viewport.y)
        }
    }

    fn target_long_edge_for(&self, page_viewport: Vec2, pixels_per_point: f32) -> u32 {
        target_long_edge_for_view(
            self.fit_mode,
            self.manual_zoom,
            page_viewport,
            pixels_per_point,
            &self.visible_original_page_sizes(),
        )
    }

    fn visible_original_page_sizes(&self) -> Vec<OriginalPageSize> {
        self.spread_indices()
            .into_iter()
            .filter_map(|index| {
                let metrics = self.page_metrics_at(index)?;
                Some(OriginalPageSize {
                    width: metrics.width,
                    height: metrics.height,
                })
            })
            .collect()
    }

    fn page_viewport_count_for_target(&self) -> usize {
        if !self.view_mode.is_smart() {
            return self.view_mode.step();
        }
        let Some(source) = self.source.as_ref() else {
            return 2;
        };
        let page_count = source.page_count();
        if page_count == 0 {
            return 1;
        }
        let page = self.current_page.min(page_count - 1);
        let anchor = page - (page % 2);
        let Some(next) = anchor.checked_add(1).filter(|next| *next < page_count) else {
            return 1;
        };
        if self.page_metrics_at(anchor).is_some() && self.page_metrics_at(next).is_some() {
            self.smart_spread_indices_for(page, page_count).len()
        } else {
            2
        }
    }

    pub(super) fn handle_viewer_pointer(&mut self, ui: &egui::Ui, response: &egui::Response) {
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
            self.pan += ui.input(|input| input.pointer.delta());
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
            let Some(gesture) = ctrl_wheel_gesture(scroll_y, zoom_delta) else {
                return;
            };
            if let Some(command) = command_for_mouse_gesture(gesture, &self.settings) {
                self.apply_command(ui.ctx(), command);
            }
        } else if let Some(gesture) = zoom_delta_gesture(zoom_delta) {
            if let Some(command) = command_for_mouse_gesture(gesture, &self.settings) {
                self.apply_command(ui.ctx(), command);
            }
        } else if scroll_y.abs() < 1.0 {
            // Swallow sub-pixel scroll noise.
        } else if self.settings.wheel_mode == WheelMode::ScrollWhenZoomed
            && self.fit_mode == FitMode::Manual
            && self.manual_zoom > 1.01
        {
            self.pan.y += scroll_y;
        } else if scroll_y < -30.0 {
            if let Some(command) =
                command_for_mouse_gesture(MouseGesture::WheelDown, &self.settings)
            {
                self.apply_command(ui.ctx(), command);
            }
        } else if scroll_y > 30.0 {
            if let Some(command) = command_for_mouse_gesture(MouseGesture::WheelUp, &self.settings)
            {
                self.apply_command(ui.ctx(), command);
            }
        }
    }
}

/// Debounces a target change until the requested value has held steady for
/// `VIEW_TARGET_SETTLE_DELAY`. Returns `Some(remaining)` while the change should
/// be held (target frozen), where `remaining` is how much longer the value must
/// stay put — the caller schedules a repaint after it so the settle is not
/// missed once input stops. Returns `None` once the value has held long enough
/// and should be applied. Any different `next` restarts the timer, so a viewport
/// that keeps churning during a resize never settles until it stops moving.
fn view_target_settle_wait(
    pending: &mut Option<(u32, Instant)>,
    next: u32,
    now: Instant,
) -> Option<Duration> {
    match *pending {
        Some((pending_target, first_seen_at)) if pending_target == next => {
            let elapsed = now.duration_since(first_seen_at);
            if elapsed >= VIEW_TARGET_SETTLE_DELAY {
                *pending = None;
                None
            } else {
                Some(VIEW_TARGET_SETTLE_DELAY - elapsed)
            }
        }
        _ => {
            *pending = Some((next, now));
            Some(VIEW_TARGET_SETTLE_DELAY)
        }
    }
}

fn ctrl_wheel_gesture(scroll_y: f32, zoom_delta: f32) -> Option<MouseGesture> {
    if scroll_y.abs() >= 1.0 {
        return Some(if scroll_y > 0.0 {
            MouseGesture::CtrlWheelUp
        } else {
            MouseGesture::CtrlWheelDown
        });
    }
    zoom_delta_gesture(zoom_delta)
}

fn zoom_delta_gesture(zoom_delta: f32) -> Option<MouseGesture> {
    if zoom_delta > 1.001 {
        Some(MouseGesture::CtrlWheelUp)
    } else if zoom_delta < 0.999 {
        Some(MouseGesture::CtrlWheelDown)
    } else {
        None
    }
}

pub(in crate::app) fn target_long_edge_for_view(
    fit_mode: FitMode,
    manual_zoom: f32,
    page_viewport: Vec2,
    pixels_per_point: f32,
    original_pages: &[OriginalPageSize],
) -> u32 {
    let display_target = display_target_long_edge_for_view(
        fit_mode,
        manual_zoom,
        page_viewport,
        pixels_per_point,
        original_pages,
    );
    original_inspection_target_long_edge(fit_mode, manual_zoom, original_pages)
        .map_or(display_target, |original_target| {
            original_target.max(display_target)
        })
}

fn display_target_long_edge_for_view(
    fit_mode: FitMode,
    manual_zoom: f32,
    page_viewport: Vec2,
    pixels_per_point: f32,
    original_pages: &[OriginalPageSize],
) -> u32 {
    let base_points = match fit_mode {
        FitMode::FitWidth => page_viewport.x,
        FitMode::FitHeight => page_viewport.y,
        _ => page_viewport.x.max(page_viewport.y),
    };
    let viewport_pixels = base_points * pixels_per_point;
    let zoom_multiplier = match fit_mode {
        FitMode::Manual => manual_zoom.max(1.0),
        _ => 1.0,
    };
    let oversample = match fit_mode {
        FitMode::FitPage | FitMode::FitWidth | FitMode::FitHeight => 1.0,
        FitMode::Manual | FitMode::Original => 1.5,
    };
    let raw = viewport_pixels * oversample * zoom_multiplier;
    let exact = raw.ceil() as u32;
    let viewport_target = match fit_mode {
        FitMode::FitPage | FitMode::FitWidth | FitMode::FitHeight => clamp_target_long_edge(exact),
        FitMode::Manual | FitMode::Original => clamp_navigation_target_long_edge(exact),
    };
    fit_axis_target_long_edge(fit_mode, page_viewport, pixels_per_point, original_pages)
        .map_or(viewport_target, |axis_target| {
            viewport_target.max(axis_target)
        })
}

fn fit_axis_target_long_edge(
    fit_mode: FitMode,
    page_viewport: Vec2,
    pixels_per_point: f32,
    original_pages: &[OriginalPageSize],
) -> Option<u32> {
    let target_axis_pixels = match fit_mode {
        FitMode::FitWidth => page_viewport.x * pixels_per_point,
        FitMode::FitHeight => page_viewport.y * pixels_per_point,
        FitMode::FitPage | FitMode::Manual | FitMode::Original => return None,
    }
    .max(1.0);

    original_pages
        .iter()
        .filter_map(|size| {
            let source_axis = match fit_mode {
                FitMode::FitWidth => size.width,
                FitMode::FitHeight => size.height,
                FitMode::FitPage | FitMode::Manual | FitMode::Original => return None,
            }
            .max(1.0);
            let source_long_edge = size.long_edge().max(1.0);
            let prepared_axis = display_axis_pixels(target_axis_pixels, source_axis);
            let target = (source_long_edge * prepared_axis / source_axis).ceil() as u32;
            Some(clamp_target_long_edge(target))
        })
        .max()
}

fn display_axis_pixels(axis_pixels: f32, source_axis: f32) -> f32 {
    axis_pixels.max(1.0).ceil().min(source_axis.max(1.0))
}

fn original_inspection_target_long_edge(
    fit_mode: FitMode,
    manual_zoom: f32,
    original_pages: &[OriginalPageSize],
) -> Option<u32> {
    let needs_original_pixels = match fit_mode {
        FitMode::Original => true,
        FitMode::Manual => manual_zoom >= 1.0,
        FitMode::FitPage | FitMode::FitWidth | FitMode::FitHeight => false,
    };
    if !needs_original_pixels {
        return None;
    }
    original_pages
        .iter()
        .map(|size| clamp_target_long_edge(size.long_edge().ceil() as u32))
        .max()
}

/// Freeze/debounce state for the settle-to-exact prepared target. `applied`
/// records the view signature of the currently applied target so the update
/// point can tell resize motion (viewport changed) apart from mode switches and
/// page turns (which apply immediately). `pending` holds the debounced target
/// value and the instant it was first requested.
#[derive(Debug, Default)]
pub(in crate::app) struct ViewTargetSettle {
    applied: Option<ViewTargetSignature>,
    pending: Option<(u32, Instant)>,
}

/// A comparable fingerprint of the inputs that drive the display target: the
/// fit/view mode and the page viewport in whole device pixels. Sub-pixel float
/// noise is rounded away so an unchanged layout compares equal frame to frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ViewTargetSignature {
    mode: (FitMode, ViewMode),
    viewport: [u32; 3],
}

impl ViewTargetSignature {
    fn new(
        fit_mode: FitMode,
        view_mode: ViewMode,
        page_viewport: Vec2,
        pixels_per_point: f32,
    ) -> Self {
        let width_px = (page_viewport.x * pixels_per_point).round().max(0.0) as u32;
        let height_px = (page_viewport.y * pixels_per_point).round().max(0.0) as u32;
        let ppp = (pixels_per_point * 1000.0).round().max(0.0) as u32;
        Self {
            mode: (fit_mode, view_mode),
            viewport: [width_px, height_px, ppp],
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(in crate::app) struct OriginalPageSize {
    pub(in crate::app) width: f32,
    pub(in crate::app) height: f32,
}

impl OriginalPageSize {
    pub(in crate::app) fn long_edge(self) -> f32 {
        self.width.max(self.height)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ctrl_wheel_gesture, target_long_edge_for_view, view_target_settle_wait, zoom_delta_gesture,
        OriginalPageSize, ViewTargetSignature, VIEW_TARGET_SETTLE_DELAY,
    };
    use super::{FitMode, ViewMode};
    use crate::core::state::MouseGesture;
    use egui::Vec2;
    use std::time::Instant;

    #[test]
    fn fit_modes_can_request_viewport_native_targets_above_navigation_cap() {
        assert_eq!(
            target_long_edge_for_view(
                FitMode::FitPage,
                1.0,
                Vec2::new(10_000.0, 10_000.0),
                1.0,
                &[page_size(8192.0, 8192.0)],
            ),
            10_000
        );
    }

    #[test]
    fn fit_modes_stay_unchanged_below_navigation_cap() {
        assert_eq!(
            target_long_edge_for_view(
                FitMode::FitPage,
                1.0,
                Vec2::new(3600.0, 2100.0),
                1.0,
                &[page_size(8192.0, 8192.0)],
            ),
            3600
        );
    }

    #[test]
    fn fit_width_preserves_webtoon_source_width_when_viewport_is_wider() {
        assert_eq!(
            target_long_edge_for_view(
                FitMode::FitWidth,
                1.0,
                Vec2::new(1200.0, 1600.0),
                1.0,
                &[page_size(800.0, 20_000.0)],
            ),
            20_000
        );
    }

    #[test]
    fn fit_width_uses_display_width_for_high_resolution_webtoon() {
        assert_eq!(
            target_long_edge_for_view(
                FitMode::FitWidth,
                1.0,
                Vec2::new(1200.0, 1600.0),
                1.0,
                &[page_size(1600.0, 20_000.0)],
            ),
            15_000
        );
    }

    #[test]
    fn fit_height_uses_display_height_for_wide_panorama() {
        assert_eq!(
            target_long_edge_for_view(
                FitMode::FitHeight,
                1.0,
                Vec2::new(1600.0, 1200.0),
                1.0,
                &[page_size(20_000.0, 1600.0)],
            ),
            15_000
        );
    }

    #[test]
    fn original_mode_requests_source_long_edge_when_known() {
        assert_eq!(
            target_long_edge_for_view(
                FitMode::Original,
                1.0,
                Vec2::new(1200.0, 1600.0),
                1.0,
                &[page_size(8192.0, 8192.0)],
            ),
            8192
        );
    }

    #[test]
    fn manual_zoom_at_original_scale_requests_source_long_edge() {
        assert_eq!(
            target_long_edge_for_view(
                FitMode::Manual,
                1.1,
                Vec2::new(1200.0, 1600.0),
                1.0,
                &[page_size(8192.0, 8192.0)],
            ),
            8192
        );
    }

    #[test]
    fn original_mode_without_metrics_uses_display_target() {
        assert_eq!(
            target_long_edge_for_view(FitMode::Original, 1.0, Vec2::new(1200.0, 1600.0), 1.0, &[],),
            2400
        );
    }

    #[test]
    fn fit_target_uses_exact_display_pixels_without_quantization() {
        // A viewport that lands between the old 256px steps must resolve to its
        // exact pixel long edge (clamped to MIN 1024) rather than a rounded-up
        // multiple of 256.
        assert_eq!(
            target_long_edge_for_view(
                FitMode::FitPage,
                1.0,
                Vec2::new(1500.0, 1400.0),
                1.0,
                &[page_size(8192.0, 8192.0)],
            ),
            1500
        );
        assert_eq!(
            target_long_edge_for_view(
                FitMode::FitPage,
                1.0,
                Vec2::new(1500.4, 1400.0),
                1.0,
                &[page_size(8192.0, 8192.0)],
            ),
            1501
        );
    }

    #[test]
    fn fit_width_axis_target_uses_exact_prepared_axis() {
        // FitWidth on a source narrower than the viewport is capped at the
        // source width and scaled with a plain ceil (no 256 quantization).
        assert_eq!(
            target_long_edge_for_view(
                FitMode::FitWidth,
                1.0,
                Vec2::new(1000.0, 1600.0),
                1.0,
                &[page_size(1300.0, 5000.0)],
            ),
            // prepared axis = 1000 (< source 1300); long edge = ceil(5000 * 1000 / 1300)
            3847
        );
    }

    #[test]
    fn view_target_change_waits_for_a_stable_settle_window() {
        let now = Instant::now();
        let mut pending = None;

        // First sight of the new target holds it for the full settle window.
        assert_eq!(
            view_target_settle_wait(&mut pending, 1500, now),
            Some(VIEW_TARGET_SETTLE_DELAY)
        );
        // Half way through, only the remaining time is requested.
        assert_eq!(
            view_target_settle_wait(&mut pending, 1500, now + VIEW_TARGET_SETTLE_DELAY / 2),
            Some(VIEW_TARGET_SETTLE_DELAY / 2)
        );
        // Once it has held for the whole window the change is applied.
        assert_eq!(
            view_target_settle_wait(&mut pending, 1500, now + VIEW_TARGET_SETTLE_DELAY),
            None
        );
        assert!(pending.is_none());
    }

    #[test]
    fn view_target_change_that_keeps_moving_never_settles() {
        // A resize drag produces a slightly different exact target each frame;
        // every change restarts the timer so the target stays frozen.
        let now = Instant::now();
        let mut pending = None;

        assert_eq!(
            view_target_settle_wait(&mut pending, 1500, now),
            Some(VIEW_TARGET_SETTLE_DELAY)
        );
        // A new value even after the delay restarts the full window.
        assert_eq!(
            view_target_settle_wait(&mut pending, 1512, now + VIEW_TARGET_SETTLE_DELAY),
            Some(VIEW_TARGET_SETTLE_DELAY)
        );
        // The latest value has only just been seen, so it is still held.
        assert!(view_target_settle_wait(
            &mut pending,
            1512,
            now + VIEW_TARGET_SETTLE_DELAY + VIEW_TARGET_SETTLE_DELAY / 2,
        )
        .is_some());
    }

    #[test]
    fn view_target_signature_detects_mode_and_viewport_changes() {
        let base = ViewTargetSignature::new(
            FitMode::FitPage,
            ViewMode::Single,
            Vec2::new(1000.0, 800.0),
            1.0,
        );
        // Sub-pixel noise on an otherwise identical layout compares equal.
        assert_eq!(
            base,
            ViewTargetSignature::new(
                FitMode::FitPage,
                ViewMode::Single,
                Vec2::new(1000.2, 799.8),
                1.0,
            )
        );
        // A fit-mode switch is a different signature (immediate-apply case).
        assert_ne!(
            base.mode,
            ViewTargetSignature::new(
                FitMode::FitWidth,
                ViewMode::Single,
                Vec2::new(1000.0, 800.0),
                1.0,
            )
            .mode
        );
        // A resize keeps the mode but changes the viewport fingerprint.
        let resized = ViewTargetSignature::new(
            FitMode::FitPage,
            ViewMode::Single,
            Vec2::new(1100.0, 800.0),
            1.0,
        );
        assert_eq!(base.mode, resized.mode);
        assert_ne!(base.viewport, resized.viewport);
    }

    #[test]
    fn ctrl_wheel_gesture_accepts_egui_zoom_delta() {
        assert_eq!(
            ctrl_wheel_gesture(0.0, 1.1),
            Some(MouseGesture::CtrlWheelUp)
        );
        assert_eq!(
            ctrl_wheel_gesture(0.0, 0.9),
            Some(MouseGesture::CtrlWheelDown)
        );
        assert_eq!(ctrl_wheel_gesture(0.0, 1.0), None);
    }

    #[test]
    fn zoom_delta_gesture_does_not_require_modifier_state() {
        assert_eq!(zoom_delta_gesture(1.1), Some(MouseGesture::CtrlWheelUp));
        assert_eq!(zoom_delta_gesture(0.9), Some(MouseGesture::CtrlWheelDown));
        assert_eq!(zoom_delta_gesture(1.0), None);
    }

    #[test]
    fn ctrl_wheel_gesture_prefers_raw_scroll_direction() {
        assert_eq!(
            ctrl_wheel_gesture(120.0, 0.9),
            Some(MouseGesture::CtrlWheelUp)
        );
        assert_eq!(
            ctrl_wheel_gesture(-120.0, 1.1),
            Some(MouseGesture::CtrlWheelDown)
        );
    }

    fn page_size(width: f32, height: f32) -> OriginalPageSize {
        OriginalPageSize { width, height }
    }
}
