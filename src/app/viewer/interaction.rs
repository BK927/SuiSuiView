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
        // In the strip the column IS the display width: size the decode target as
        // fit-width against a column-wide viewport, regardless of the user-facing
        // fit mode, so narrow columns don't over-decode nor Manual under-decode.
        if self.view_mode == ViewMode::VerticalStrip {
            let column = self.strip_column_width(page_viewport);
            return target_long_edge_for_view(
                FitMode::FitWidth,
                1.0,
                Vec2::new(column, page_viewport.y),
                pixels_per_point,
                &self.visible_original_page_sizes(),
            );
        }
        target_long_edge_for_view(
            self.fit_mode,
            self.manual_zoom,
            page_viewport,
            pixels_per_point,
            &self.visible_original_page_sizes(),
        )
    }

    fn visible_original_page_sizes(&self) -> Vec<OriginalPageSize> {
        self.visible_page_indices()
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
        self.sync_wheel_view_mode(ui);
        if self.view_mode == ViewMode::VerticalStrip {
            if response.hovered() {
                let (scroll_y, ctrl, zoom_delta) = ui.input(|input| {
                    (
                        input.raw_scroll_delta.y,
                        input.modifiers.ctrl,
                        input.zoom_delta(),
                    )
                });
                if !ctrl && (zoom_delta - 1.0).abs() < WHEEL_ZOOM_DELTA_EPSILON && scroll_y != 0.0 {
                    self.begin_wheel_behavior(ui, WheelBehavior::DirectScroll);
                }
            }
            self.handle_strip_pointer(ui, response);
            return;
        }
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
            self.apply_wheel_gesture_steps(ui, scroll_y, zoom_delta);
        } else if (zoom_delta - 1.0).abs() >= WHEEL_ZOOM_DELTA_EPSILON {
            // Trackpad pinch arrives as a zoom factor without the ctrl modifier.
            self.apply_wheel_gesture_steps(ui, 0.0, zoom_delta);
        } else {
            let direct_scroll = self.settings.wheel_mode == WheelMode::ScrollWhenZoomed
                && self.fit_mode == FitMode::Manual
                && self.manual_zoom > 1.01;
            match route_wheel_scroll(scroll_y, direct_scroll) {
                WheelScrollRoute::Ignore => {}
                WheelScrollRoute::DirectScroll(points) => {
                    self.begin_wheel_behavior(ui, WheelBehavior::DirectScroll);
                    if points.abs() >= 1.0 {
                        self.pan.y += points;
                    }
                }
                WheelScrollRoute::PageTurn(points) => {
                    self.apply_page_turn_wheel_steps(ui, points);
                }
            }
        }
    }

    fn sync_wheel_view_mode(&mut self, ui: &egui::Ui) {
        let reset_zoom = ui.ctx().data_mut(|data| {
            data.get_temp_mut_or_default::<WheelInteractionState>(wheel_interaction_state_id())
                .set_view_mode(self.view_mode)
        });
        if reset_zoom {
            self.reset_zoom_wheel_accumulator();
        }
    }

    fn begin_wheel_behavior(&mut self, ui: &egui::Ui, behavior: WheelBehavior) {
        let reset_zoom = ui.ctx().data_mut(|data| {
            data.get_temp_mut_or_default::<WheelInteractionState>(wheel_interaction_state_id())
                .begin_behavior(behavior)
        });
        if reset_zoom {
            self.reset_zoom_wheel_accumulator();
        }
    }

    fn reset_zoom_wheel_accumulator(&mut self) {
        self.wheel_gesture_accum = 0.0;
        self.wheel_gesture_last = None;
    }

    fn apply_page_turn_wheel_steps(&mut self, ui: &egui::Ui, scroll_points: f32) {
        self.begin_wheel_behavior(ui, WheelBehavior::PageTurn);
        let now = Instant::now();
        let steps = ui.ctx().data_mut(|data| {
            data.get_temp_mut_or_default::<WheelInteractionState>(wheel_interaction_state_id())
                .page_turn_steps(scroll_points, now)
        });
        if steps == 0 {
            return;
        }
        let gesture = if steps > 0 {
            MouseGesture::WheelUp
        } else {
            MouseGesture::WheelDown
        };
        let Some(command) = command_for_mouse_gesture(gesture, &self.settings) else {
            return;
        };
        for _ in 0..steps.unsigned_abs() {
            self.apply_command(ui.ctx(), command);
        }
    }

    /// Drain this frame's analog wheel/pinch input through the notch
    /// accumulator and apply the bound CtrlWheel gesture once per whole step.
    /// Shared with the strip, whose zoom command reroutes into its column model.
    pub(in crate::app) fn apply_wheel_gesture_steps(
        &mut self,
        ui: &egui::Ui,
        scroll_points: f32,
        zoom_delta: f32,
    ) {
        self.begin_wheel_behavior(ui, WheelBehavior::Zoom);
        let now = Instant::now();
        let steps = timed_wheel_gesture_steps(
            &mut self.wheel_gesture_accum,
            &mut self.wheel_gesture_last,
            scroll_points,
            zoom_delta,
            now,
        );
        if steps == 0 {
            return;
        }
        let gesture = if steps > 0 {
            MouseGesture::CtrlWheelUp
        } else {
            MouseGesture::CtrlWheelDown
        };
        let Some(command) = command_for_mouse_gesture(gesture, &self.settings) else {
            return;
        };
        for _ in 0..steps.unsigned_abs() {
            self.apply_command(ui.ctx(), command);
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

/// One classic wheel notch in egui points (egui's native `line_scroll_speed`
/// converts `MouseScrollDelta::LineDelta(1.0)` to 40 points).
const WHEEL_NOTCH_POINTS: f32 = 40.0;
/// ln(1.1): the per-notch zoom step in log space, used to convert an analog
/// pinch factor into notch equivalents (7 notches ≈ 2x, matching the wheel).
const WHEEL_STEP_LN: f32 = 0.095_310_2;
/// A pinch factor closer to 1.0 than this is treated as no zoom input.
pub(in crate::app) const WHEEL_ZOOM_DELTA_EPSILON: f32 = 1e-4;
/// Idle time after which a leftover partial notch is forgotten, so half a
/// notch flicked now does not surface as a ghost step much later.
const WHEEL_ACCUM_TIMEOUT: Duration = Duration::from_millis(300);
/// Upper bound on gesture steps applied from a single frame's input.
const WHEEL_MAX_STEPS_PER_FRAME: i32 = 8;
/// Tolerance used only when an accumulated gesture is effectively an integer.
const WHEEL_STEP_BOUNDARY_EPSILON: f32 = 1e-5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WheelBehavior {
    PageTurn,
    Zoom,
    DirectScroll,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum WheelScrollRoute {
    Ignore,
    DirectScroll(f32),
    PageTurn(f32),
}

fn route_wheel_scroll(scroll_points: f32, direct_scroll: bool) -> WheelScrollRoute {
    if scroll_points.abs() < f32::EPSILON {
        return WheelScrollRoute::Ignore;
    }
    if direct_scroll {
        // The handler preserves the old noise threshold for pixel panning, but
        // still observes this behavior change so a page-turn remainder cannot
        // leak across modes.
        return WheelScrollRoute::DirectScroll(scroll_points);
    }
    WheelScrollRoute::PageTurn(scroll_points)
}

#[derive(Debug, Clone, Default)]
struct WheelInteractionState {
    view_mode: Option<ViewMode>,
    behavior: Option<WheelBehavior>,
    page_turn_accum: f32,
    page_turn_last: Option<Instant>,
}

impl WheelInteractionState {
    fn set_view_mode(&mut self, view_mode: ViewMode) -> bool {
        if self.view_mode == Some(view_mode) {
            return false;
        }
        self.view_mode = Some(view_mode);
        self.behavior = None;
        self.reset_page_turn();
        true
    }

    fn begin_behavior(&mut self, behavior: WheelBehavior) -> bool {
        if self.behavior == Some(behavior) {
            return false;
        }
        self.behavior = Some(behavior);
        self.reset_page_turn();
        true
    }

    fn reset_page_turn(&mut self) {
        self.page_turn_accum = 0.0;
        self.page_turn_last = None;
    }

    fn page_turn_steps(&mut self, scroll_points: f32, now: Instant) -> i32 {
        timed_wheel_gesture_steps(
            &mut self.page_turn_accum,
            &mut self.page_turn_last,
            scroll_points,
            1.0,
            now,
        )
    }
}

fn wheel_interaction_state_id() -> egui::Id {
    egui::Id::new("viewer_wheel_interaction_state")
}

fn timed_wheel_gesture_steps(
    accum: &mut f32,
    last: &mut Option<Instant>,
    scroll_points: f32,
    zoom_delta: f32,
    now: Instant,
) -> i32 {
    if last.is_some_and(|previous| now.duration_since(previous) > WHEEL_ACCUM_TIMEOUT) {
        *accum = 0.0;
    }
    if scroll_points.abs() >= f32::EPSILON || (zoom_delta - 1.0).abs() >= WHEEL_ZOOM_DELTA_EPSILON {
        *last = Some(now);
    }
    wheel_gesture_steps(accum, scroll_points, zoom_delta)
}

/// Convert one frame's analog wheel/pinch input into whole gesture steps,
/// carrying the fractional remainder in `accum`. High-resolution wheels and
/// trackpads deliver a notch's worth of input spread over many small frames;
/// accumulating keeps one physical notch equal to exactly one step instead of
/// one step per frame. Raw scroll wins over the synthesized pinch factor when
/// both are present (same priority as the previous per-frame gesture logic).
fn wheel_gesture_steps(accum: &mut f32, scroll_points: f32, zoom_delta: f32) -> i32 {
    let notches = if scroll_points.abs() >= f32::EPSILON {
        scroll_points / WHEEL_NOTCH_POINTS
    } else if (zoom_delta - 1.0).abs() >= WHEEL_ZOOM_DELTA_EPSILON {
        zoom_delta.ln() / WHEEL_STEP_LN
    } else {
        0.0
    };
    if *accum != 0.0 && notches != 0.0 && accum.is_sign_positive() != notches.is_sign_positive() {
        *accum = 0.0;
    }
    *accum += notches;
    // Repeated fractional input can land microscopically below an exact
    // notch. Snap only values at the floating-point boundary so a physical
    // 40-point notch is not delayed by another frame.
    let nearest_step = accum.round();
    if (*accum - nearest_step).abs() <= WHEEL_STEP_BOUNDARY_EPSILON {
        *accum = nearest_step;
    }
    let steps = *accum as i32;
    if steps.abs() >= WHEEL_MAX_STEPS_PER_FRAME {
        *accum = 0.0;
        return steps.clamp(-WHEEL_MAX_STEPS_PER_FRAME, WHEEL_MAX_STEPS_PER_FRAME);
    }
    *accum -= steps as f32;
    steps
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
        route_wheel_scroll, target_long_edge_for_view, view_target_settle_wait,
        wheel_gesture_steps, OriginalPageSize, ViewTargetSignature, WheelBehavior,
        WheelInteractionState, WheelScrollRoute, VIEW_TARGET_SETTLE_DELAY,
        WHEEL_MAX_STEPS_PER_FRAME,
    };
    use super::{FitMode, ViewMode};
    use egui::Vec2;
    use std::time::{Duration, Instant};

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
    fn one_wheel_notch_is_exactly_one_step() {
        let mut accum = 0.0;
        assert_eq!(wheel_gesture_steps(&mut accum, 40.0, 1.0), 1);
        assert_eq!(accum, 0.0);
        assert_eq!(wheel_gesture_steps(&mut accum, -40.0, 1.0), -1);
        assert_eq!(accum, 0.0);
    }

    #[test]
    fn fragmented_high_resolution_flick_sums_to_its_true_notch_count() {
        // 15 frames of small deltas that add up to just over one notch: the old
        // per-frame logic applied 15 steps; the accumulator applies exactly 1.
        let mut accum = 0.0;
        let mut steps = 0;
        for _ in 0..15 {
            steps += wheel_gesture_steps(&mut accum, 2.7, 1.0);
        }
        assert_eq!(steps, 1);
    }

    #[test]
    fn fragmented_page_turn_flick_sums_to_one_step() {
        let mut state = WheelInteractionState::default();
        assert!(state.set_view_mode(ViewMode::Single));
        assert!(state.begin_behavior(WheelBehavior::PageTurn));
        let started = Instant::now();
        let mut steps = 0;
        for frame in 0..15 {
            steps += state.page_turn_steps(2.7, started + Duration::from_millis(frame * 10));
        }
        assert_eq!(steps, 1);
    }

    #[test]
    fn sub_point_page_turn_fragments_reach_the_accumulator() {
        let mut state = WheelInteractionState::default();
        state.set_view_mode(ViewMode::Single);
        state.begin_behavior(WheelBehavior::PageTurn);
        let started = Instant::now();
        let mut steps = 0;
        for frame in 0..80 {
            let WheelScrollRoute::PageTurn(points) = route_wheel_scroll(0.5, false) else {
                panic!("page-turn routing must preserve sub-point input");
            };
            steps += state.page_turn_steps(points, started + Duration::from_millis(frame * 3));
        }
        assert_eq!(steps, 1);
        assert_eq!(state.page_turn_accum, 0.0);
        assert_eq!(
            route_wheel_scroll(0.5, true),
            WheelScrollRoute::DirectScroll(0.5)
        );
    }

    #[test]
    fn page_turn_and_zoom_remainders_are_independent() {
        let mut page_turn_accum = 0.0;
        let mut zoom_accum = 0.0;

        assert_eq!(wheel_gesture_steps(&mut page_turn_accum, 20.0, 1.0), 0);
        assert_eq!(wheel_gesture_steps(&mut zoom_accum, 20.0, 1.0), 0);
        assert_eq!(wheel_gesture_steps(&mut page_turn_accum, 20.0, 1.0), 1);
        assert_eq!(zoom_accum, 0.5);
    }

    #[test]
    fn changing_wheel_behavior_drops_page_turn_remainder() {
        let mut state = WheelInteractionState::default();
        state.set_view_mode(ViewMode::Single);
        state.begin_behavior(WheelBehavior::PageTurn);
        assert_eq!(state.page_turn_steps(20.0, Instant::now()), 0);
        assert_eq!(state.page_turn_accum, 0.5);

        assert!(state.begin_behavior(WheelBehavior::Zoom));
        assert_eq!(state.page_turn_accum, 0.0);
        assert!(state.begin_behavior(WheelBehavior::PageTurn));
        assert_eq!(state.page_turn_steps(20.0, Instant::now()), 0);
    }

    #[test]
    fn changing_view_mode_drops_page_turn_remainder() {
        let mut state = WheelInteractionState::default();
        state.set_view_mode(ViewMode::Single);
        state.begin_behavior(WheelBehavior::PageTurn);
        assert_eq!(state.page_turn_steps(20.0, Instant::now()), 0);

        assert!(state.set_view_mode(ViewMode::DoubleRightToLeft));
        assert_eq!(state.page_turn_accum, 0.0);
        assert_eq!(state.behavior, None);
    }

    #[test]
    fn reversing_wheel_direction_drops_the_opposite_remainder() {
        let mut accum = 0.0;
        assert_eq!(wheel_gesture_steps(&mut accum, 20.0, 1.0), 0);
        assert_eq!(accum, 0.5);
        assert_eq!(wheel_gesture_steps(&mut accum, -40.0, 1.0), -1);
        assert_eq!(accum, 0.0);
    }

    #[test]
    fn pinch_zoom_delta_converts_to_notch_equivalents() {
        // ln(1.21) / ln(1.1) ~= 2.0: a 1.21x pinch frame is two zoom steps.
        let mut accum = 0.0;
        assert_eq!(wheel_gesture_steps(&mut accum, 0.0, 1.21), 2);
        // A gentle pinch accumulates across frames instead of firing every frame.
        let mut accum = 0.0;
        let mut steps = 0;
        for _ in 0..5 {
            steps += wheel_gesture_steps(&mut accum, 0.0, 1.02);
        }
        assert_eq!(steps, 1);
        assert_eq!(wheel_gesture_steps(&mut accum, 0.0, 1.0), 0);
    }

    #[test]
    fn raw_scroll_wins_over_the_synthesized_pinch_factor() {
        let mut accum = 0.0;
        assert_eq!(wheel_gesture_steps(&mut accum, 40.0, 0.9), 1);
        let mut accum = 0.0;
        assert_eq!(wheel_gesture_steps(&mut accum, -40.0, 1.1), -1);
    }

    #[test]
    fn wheel_steps_per_frame_are_clamped() {
        let mut accum = 0.0;
        assert_eq!(
            wheel_gesture_steps(&mut accum, 4000.0, 1.0),
            WHEEL_MAX_STEPS_PER_FRAME
        );
        // The clamp also drops the excess instead of replaying it later.
        assert_eq!(accum, 0.0);
    }

    fn page_size(width: f32, height: f32) -> OriginalPageSize {
        OriginalPageSize { width, height }
    }
}
