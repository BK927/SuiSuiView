use super::ViewMode;
use crate::app::commands::command_for_mouse_gesture;
use crate::app::SuiSuiViewApp;
use crate::core::state::MouseGesture;
use std::time::{Duration, Instant};

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
pub(super) enum WheelBehavior {
    PageTurn,
    Zoom,
    DirectScroll,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum WheelScrollRoute {
    Ignore,
    DirectScroll(f32),
    PageTurn(f32),
}

pub(super) fn route_wheel_scroll(scroll_points: f32, direct_scroll: bool) -> WheelScrollRoute {
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

impl SuiSuiViewApp {
    pub(super) fn sync_wheel_view_mode(&mut self, ui: &egui::Ui) {
        let reset_zoom = ui.ctx().data_mut(|data| {
            data.get_temp_mut_or_default::<WheelInteractionState>(wheel_interaction_state_id())
                .set_view_mode(self.view_mode)
        });
        if reset_zoom {
            self.reset_zoom_wheel_accumulator();
        }
    }

    pub(super) fn begin_wheel_behavior(&mut self, ui: &egui::Ui, behavior: WheelBehavior) {
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

    pub(super) fn apply_page_turn_wheel_steps(&mut self, ui: &egui::Ui, scroll_points: f32) {
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

#[cfg(test)]
mod tests {
    use super::{
        route_wheel_scroll, wheel_gesture_steps, WheelBehavior, WheelInteractionState,
        WheelScrollRoute, WHEEL_MAX_STEPS_PER_FRAME,
    };
    use crate::app::viewer::ViewMode;
    use std::time::{Duration, Instant};

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
}
