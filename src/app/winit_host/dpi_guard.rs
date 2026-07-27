use egui_winit::winit;
use std::time::{Duration, Instant};

/// Sizes reported within this window of a scale change are transition/storm
/// artifacts, not a settled resize the user asked for, so they must not update
/// the authoritative logical size.
const STORM_WINDOW: Duration = Duration::from_millis(500);

/// A calm-period resize only becomes the authoritative size after surviving
/// this long without a scale change following it. Measured pathology: winit
/// can apply a stale DPI suggested rect as a plain `Resized` ~0.4ms BEFORE the
/// corresponding `ScaleFactorChanged` (and 2.5s after the previous one, i.e.
/// far outside the storm window), which would otherwise poison the tracked
/// size right before the guard uses it.
const CONFIRM_WINDOW: Duration = Duration::from_millis(100);

/// The window's live OS state at the instant an event is handled, read from
/// Win32 style bits (`IsWindowVisible`/`IsZoomed`), NOT winit's cached flags —
/// see the call site in `winit_host.rs` for why the cached flags lag the reveal.
/// The guard consults it to decide whether a resize/scale event may drive a
/// correction or update the tracked restore size.
#[derive(Clone, Copy)]
pub(super) struct WindowLiveState {
    /// A hidden window's size is only a creation hint; worse, a correction
    /// emitted while hidden can be delayed by the ShowWindow message pump and
    /// land AFTER the reveal maximizes, knocking the window out of the maximized
    /// state and clobbering rcNormalPosition. So hidden events never correct or
    /// learn a size.
    pub(super) visible: bool,
    /// A maximized window's size is OS-determined; it is never corrected, nor
    /// learned as a restore size.
    pub(super) maximized: bool,
}

/// winit 0.30 can corrupt the window's logical size when WM_DPICHANGED
/// oscillates while a drag straddles monitors with different scales (measured:
/// 4 scale flips in 18ms, after which the logical size drifted and stuck).
/// The guard remembers the logical size from stable periods and re-asserts it
/// on every scale change, making each transition idempotent: a correct winit
/// transition writes the same value (no-op), a corrupted one is overridden.
pub(super) struct DpiSizeGuard {
    stable_logical: Option<(f64, f64)>,
    /// Candidate from the most recent calm resize, promoted to stable only
    /// after `CONFIRM_WINDOW` passes without a scale change (see the const).
    pending_logical: Option<((f64, f64), Instant)>,
    current_scale: f64,
    last_scale_change: Option<Instant>,
}

impl DpiSizeGuard {
    /// A fresh guard: scale 0.0 = unknown until the first `observe_scale`.
    pub(super) fn new() -> Self {
        Self {
            stable_logical: None,
            pending_logical: None,
            current_scale: 0.0,
            last_scale_change: None,
        }
    }

    /// Promote a surviving pending size to stable once it has outlived
    /// `CONFIRM_WINDOW` (no scale change arrived to disqualify it).
    fn promote_confirmed_pending(&mut self, now: Instant) {
        if let Some((logical, since)) = self.pending_logical {
            if now.duration_since(since) >= CONFIRM_WINDOW {
                self.stable_logical = Some(logical);
                self.pending_logical = None;
            }
        }
    }

    /// Record a `Resized` event. Sizes observed within `STORM_WINDOW` of a
    /// scale change are transition/storm artifacts and must NOT update the
    /// stable logical size; when such a size also DEVIATES from the tracked
    /// logical size, the corrective physical size is returned and the host
    /// must re-request it on the window. Measured pathology: winit 0.30
    /// applies a stale DPI suggested rect ~12ms after a boundary storm as a
    /// plain `Resized` with no `ScaleFactorChanged` (1920x1230 at 1.5 followed
    /// by a phantom 2308x1487 = x1.2 re-applied), and that wrong size sticks
    /// when the drag ends there. The correction converges in one step: the
    /// re-requested size matches the expectation, so its own `Resized` returns
    /// `None`. `0x0` (minimize) and resizes before the scale is known are
    /// ignored.
    #[must_use]
    pub(super) fn observe_resize(
        &mut self,
        physical: (u32, u32),
        now: Instant,
        live: WindowLiveState,
    ) -> Option<(u32, u32)> {
        let (w, h) = physical;
        if w == 0 || h == 0 || self.current_scale <= 0.0 {
            return None;
        }
        self.promote_confirmed_pending(now);
        // Hidden or maximized: this is not a user-chosen restore size, so never
        // correct it and never let it become the tracked logical size. A maximized
        // `Resized` (delivered inside the storm window of the reveal-time scale
        // change) is OS-determined; adopting it is what re-requested a shrunk size
        // on the maximized window and made Windows rewrite rcNormalPosition to the
        // maximized rect. A hidden `Resized` is only creation-hint churn, and a
        // correction emitted for it can be queued behind ShowWindow and land on
        // the post-reveal maximized window — the same corruption by a later race.
        // The prior pending size (a genuine pre-maximize resize) is still promoted.
        if !live.visible || live.maximized {
            return None;
        }
        if let Some(last) = self.last_scale_change {
            if now.duration_since(last) < STORM_WINDOW {
                let (expected_w, expected_h) = self.stable_logical.map(|(lw, lh)| {
                    (
                        (lw * self.current_scale).round() as u32,
                        (lh * self.current_scale).round() as u32,
                    )
                })?;
                let off_by = |actual: u32, expected: u32| actual.abs_diff(expected) > 2;
                return (off_by(w, expected_w) || off_by(h, expected_h))
                    .then_some((expected_w, expected_h));
            }
        }
        self.pending_logical = Some((
            (
                f64::from(w) / self.current_scale,
                f64::from(h) / self.current_scale,
            ),
            now,
        ));
        None
    }

    /// Record a `ScaleFactorChanged`; returns the physical size to request via
    /// the `inner_size_writer` (`stable_logical x new_scale`), or `None` when no
    /// stable size is known yet. A pending size younger than `CONFIRM_WINDOW`
    /// is discarded here: a resize immediately followed by a scale change is a
    /// stale DPI suggested rect (measured 0.4ms apart), not user intent.
    pub(super) fn on_scale_change(
        &mut self,
        new_scale: f64,
        now: Instant,
        live: WindowLiveState,
    ) -> Option<(u32, u32)> {
        self.promote_confirmed_pending(now);
        self.pending_logical = None;
        self.last_scale_change = Some(now);
        self.current_scale = new_scale;
        // The DPI fact (the new scale and its timing) is real and is recorded
        // above so a later visible, non-maximized resize is judged against the
        // right scale and storm window. But withhold the corrective size while
        // hidden or maximized: request_inner_size on a maximized window shrinks it
        // (corrupting its restore rect), and on a hidden window queues a
        // correction that can land post-reveal.
        if !live.visible || live.maximized {
            return None;
        }
        self.stable_logical.map(|(w, h)| {
            (
                (w * new_scale).round() as u32,
                (h * new_scale).round() as u32,
            )
        })
    }

    /// Seed/refresh the scale used to derive logical from physical (for the
    /// initial window scale, before any `ScaleFactorChanged`).
    pub(super) fn observe_scale(&mut self, scale: f64) {
        self.current_scale = scale;
    }

    /// Seed the guard from the freshly created window: its starting scale and
    /// inner size. `last_scale_change` is still `None`, so this counts as a calm
    /// period and the logical size is recorded immediately.
    pub(super) fn seed_initial(&mut self, window: &winit::window::Window) {
        let size = window.inner_size();
        self.observe_scale(window.scale_factor());
        // Seed the initial logical size from the freshly created window. This is
        // the intentional stable seed, exempt from the visible/maximized gate: the
        // window is still hidden here (reveal is deferred) and non-maximized, but
        // the creation size IS the authoritative starting restore size, so pass
        // `visible: true` to record it. Only EVENT-driven resizes while hidden are
        // suppressed.
        let _ = self.observe_resize(
            (size.width, size.height),
            Instant::now(),
            WindowLiveState {
                visible: true,
                maximized: false,
            },
        );
    }

    /// Host hook for `window_event`: feed a `Resized` to `observe_resize` and,
    /// on a `ScaleFactorChanged`, request `tracked_logical x new_scale` via the
    /// event's `inner_size_writer`. Normal transition = one `ScaleFactorChanged`
    /// then a `Resized` ~2ms later; a storm = several `ScaleFactorChanged`
    /// within tens of ms, during which winit derives the next size from an
    /// already-wrong in-flight size. Re-asserting the tracked logical size makes
    /// each transition idempotent. Returns a corrective physical size when a
    /// storm-window `Resized` deviated from the tracked size (a stale suggested
    /// rect applied late, with no scale event to hang the correction on) — the
    /// caller must re-request it on the window. Non-consuming: the caller passes
    /// the event on to egui afterward (e.g. for pixels_per_point).
    #[must_use]
    pub(super) fn defend_scale_change(
        &mut self,
        event: &mut winit::event::WindowEvent,
        live: WindowLiveState,
    ) -> Option<(u32, u32)> {
        match event {
            winit::event::WindowEvent::Resized(size) => {
                self.observe_resize((size.width, size.height), Instant::now(), live)
            }
            winit::event::WindowEvent::ScaleFactorChanged {
                scale_factor,
                inner_size_writer,
            } => {
                if let Some((w, h)) = self.on_scale_change(*scale_factor, Instant::now(), live) {
                    let _ =
                        inner_size_writer.request_inner_size(winit::dpi::PhysicalSize::new(w, h));
                }
                None
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{DpiSizeGuard, WindowLiveState, STORM_WINDOW};
    use std::time::{Duration, Instant};

    // Real measured logical size from the instrumented drag capture.
    const LOGICAL: (u32, u32) = (860, 691);

    // The event-time window states the guard branches on.
    const VISIBLE: WindowLiveState = WindowLiveState {
        visible: true,
        maximized: false,
    };
    const MAXIMIZED: WindowLiveState = WindowLiveState {
        visible: true,
        maximized: true,
    };
    const HIDDEN: WindowLiveState = WindowLiveState {
        visible: false,
        maximized: false,
    };

    use super::CONFIRM_WINDOW;

    /// Seeds the guard and returns the instant at which the seeded size has
    /// outlived `CONFIRM_WINDOW` (i.e. will be promoted to stable on the next
    /// call).
    fn seeded(scale: f64, now: Instant) -> (DpiSizeGuard, Instant) {
        let mut guard = DpiSizeGuard::new();
        guard.observe_scale(scale);
        let _ = guard.observe_resize(
            (
                (f64::from(LOGICAL.0) * scale).round() as u32,
                (f64::from(LOGICAL.1) * scale).round() as u32,
            ),
            now,
            VISIBLE,
        );
        (guard, now + CONFIRM_WINDOW)
    }

    #[test]
    fn stable_size_tracked_from_calm_resize() {
        // 860x691 logical at 1.5 = 1290x1037 physical, observed in a calm
        // period, confirmed, then re-derived at a new scale.
        let now = Instant::now();
        let (mut guard, confirmed) = seeded(1.5, now);
        assert_eq!(
            guard.on_scale_change(1.25, confirmed, VISIBLE),
            Some((1075, 864))
        );
    }

    #[test]
    fn resize_within_storm_window_is_ignored_and_corrected() {
        let now = Instant::now();
        let (mut guard, confirmed) = seeded(1.5, now);
        // A corrupt resize arrives right after a scale change; it must not
        // become the new stable size, and the guard hands back the corrective
        // size for the current (1.25) scale.
        guard.on_scale_change(1.25, confirmed, VISIBLE);
        assert_eq!(
            guard.observe_resize((714, 570), confirmed + Duration::from_millis(10), VISIBLE),
            Some((1075, 864))
        );
        // The stable size is still the calm one: 1.5 request stays 1290x1037.
        assert_eq!(
            guard.on_scale_change(1.5, confirmed, VISIBLE),
            Some((1290, 1037))
        );
    }

    #[test]
    fn matching_transition_resize_needs_no_correction() {
        let now = Instant::now();
        let (mut guard, confirmed) = seeded(1.5, now);
        // The normal post-transition Resized (~2ms later) matches the tracked
        // size and must not be "corrected" (no feedback loop).
        guard.on_scale_change(1.25, confirmed, VISIBLE);
        assert_eq!(
            guard.observe_resize((1075, 864), confirmed + Duration::from_millis(2), VISIBLE),
            None
        );
    }

    #[test]
    fn phantom_post_storm_resize_is_corrected() {
        // Second measured pathology: after a storm settles at scale 1.5 with
        // the correct 1920x1230 (logical 1280x820), a stale suggested rect is
        // applied ~12ms later as a plain Resized 2308x1487 (= x1.2 re-applied)
        // with NO ScaleFactorChanged. The guard must hand back 1920x1230, and
        // the follow-up Resized from that correction must converge (None).
        let t0 = Instant::now();
        let mut guard = DpiSizeGuard::new();
        guard.observe_scale(1.5);
        let _ = guard.observe_resize((1920, 1230), t0, VISIBLE);
        let confirmed = t0 + CONFIRM_WINDOW;
        guard.on_scale_change(1.25, confirmed + Duration::from_millis(1), VISIBLE);
        guard.on_scale_change(1.5, confirmed + Duration::from_millis(3), VISIBLE);
        assert_eq!(
            guard.observe_resize((2308, 1487), confirmed + Duration::from_millis(15), VISIBLE),
            Some((1920, 1230))
        );
        assert_eq!(
            guard.observe_resize((1920, 1230), confirmed + Duration::from_millis(30), VISIBLE),
            None
        );
    }

    #[test]
    fn late_phantom_right_before_scale_change_cannot_poison() {
        // Third measured pathology (the teleport): during a drag, a stale
        // suggested rect lands as a plain Resized 1434x1202 a full 2.5s after
        // the previous scale change (outside the storm window) and only 0.4ms
        // BEFORE the ScaleFactorChanged it belongs to. With immediate adoption
        // it would poison the tracked size (860x560 -> 956x801) right before
        // the guard uses it. The confirmation window discards it instead.
        let t0 = Instant::now();
        let mut guard = DpiSizeGuard::new();
        guard.observe_scale(1.5);
        let _ = guard.observe_resize((1290, 840), t0, VISIBLE); // logical 860x560
        let calm = t0 + CONFIRM_WINDOW + Duration::from_secs(2);
        let _ = guard.observe_resize((1434, 1202), calm, VISIBLE); // the phantom
        assert_eq!(
            guard.on_scale_change(1.25, calm + Duration::from_micros(400), VISIBLE),
            Some((1075, 700)) // 860x560 x 1.25 — the phantom did not stick.
        );
    }

    #[test]
    fn genuine_calm_resize_confirms_and_becomes_stable() {
        // A real user resize (measured: settled at 1520x840 @1.5) survives the
        // confirmation window and drives later transitions: 1520/1.5 x 1.25 =
        // 1267x700, exactly as captured in the healthy part of the log.
        let t0 = Instant::now();
        let mut guard = DpiSizeGuard::new();
        guard.observe_scale(1.5);
        let _ = guard.observe_resize((1290, 840), t0, VISIBLE);
        let calm = t0 + CONFIRM_WINDOW;
        let _ = guard.observe_resize((1520, 840), calm, VISIBLE);
        assert_eq!(
            guard.on_scale_change(1.25, calm + CONFIRM_WINDOW, VISIBLE),
            Some((1267, 700))
        );
    }

    #[test]
    fn on_scale_change_returns_stable_times_new_scale() {
        let now = Instant::now();
        let (mut guard, confirmed) = seeded(1.25, now);
        // Real measured numbers for 860x691 logical.
        assert_eq!(
            guard.on_scale_change(1.25, confirmed, VISIBLE),
            Some((1075, 864))
        );
        assert_eq!(
            guard.on_scale_change(1.5, confirmed, VISIBLE),
            Some((1290, 1037))
        );
    }

    #[test]
    fn storm_sequence_returns_same_size_no_drift() {
        // The bug: rapid 1.5 -> 1.25 -> 1.5 flips with corrupt in-flight
        // resizes between them. Each transition must request the SAME physical
        // size as the first, so the storm cannot make the size drift.
        let t0 = Instant::now();
        let (mut guard, confirmed) = seeded(1.5, t0);

        let first = guard.on_scale_change(1.25, confirmed + Duration::from_millis(1), VISIBLE);
        // winit hands us a corrupted in-flight size mid-storm.
        let _ = guard.observe_resize((714, 570), confirmed + Duration::from_millis(3), VISIBLE);
        let second = guard.on_scale_change(1.5, confirmed + Duration::from_millis(6), VISIBLE);
        // Another corrupt resize (the grow direction).
        let _ = guard.observe_resize((1438, 1211), confirmed + Duration::from_millis(12), VISIBLE);
        let third = guard.on_scale_change(1.25, confirmed + Duration::from_millis(18), VISIBLE);

        assert_eq!(first, Some((1075, 864)));
        assert_eq!(second, Some((1290, 1037)));
        assert_eq!(third, first); // back at 1.25: identical to the first request.
    }

    #[test]
    fn zero_size_is_ignored() {
        let now = Instant::now();
        let mut guard = DpiSizeGuard::new();
        guard.observe_scale(1.5);
        let _ = guard.observe_resize((0, 0), now, VISIBLE);
        // No stable size recorded, so a scale change has nothing to assert.
        assert_eq!(
            guard.on_scale_change(1.25, now + CONFIRM_WINDOW, VISIBLE),
            None
        );
    }

    #[test]
    fn unknown_scale_start_returns_none_until_seeded() {
        let now = Instant::now();
        let mut guard = DpiSizeGuard::new();
        // Scale unknown (0.0): a resize cannot be turned into a logical size.
        let _ = guard.observe_resize((1290, 1037), now, VISIBLE);
        assert_eq!(guard.on_scale_change(1.25, now, VISIBLE), None);
        // Once seeded and given a calm, confirmed resize, it defends the size.
        guard.observe_scale(1.5);
        let _ = guard.observe_resize((1290, 1037), now + STORM_WINDOW, VISIBLE);
        assert_eq!(
            guard.on_scale_change(1.25, now + STORM_WINDOW + CONFIRM_WINDOW, VISIBLE),
            Some((1075, 864))
        );
    }

    #[test]
    fn maximized_storm_resize_ignored_and_leaves_stable_intact() {
        // The reveal maximizes via SetWindowPlacement, which fires a Resized of
        // the maximized size INSIDE the storm window of the reveal-time scale
        // change. It must return None AND not disturb stable_logical: a following
        // non-maximized deviating resize is still corrected to the ORIGINAL size.
        let now = Instant::now();
        let (mut guard, confirmed) = seeded(1.5, now);
        // Opens the storm window.
        guard.on_scale_change(1.25, confirmed, VISIBLE);
        // The maximized Resized (huge, deviating) is ignored, not corrected.
        assert_eq!(
            guard.observe_resize(
                (1458, 2518),
                confirmed + Duration::from_millis(5),
                MAXIMIZED
            ),
            None
        );
        // stable_logical is untouched: a non-maximized deviating resize inside the
        // same storm still snaps back to 860x691 @1.25 = 1075x864.
        assert_eq!(
            guard.observe_resize((714, 570), confirmed + Duration::from_millis(10), VISIBLE),
            Some((1075, 864))
        );
    }

    #[test]
    fn maximized_calm_resize_never_becomes_tracked_size() {
        // A calm (outside-storm) Resized while maximized must not become pending
        // or stable — otherwise a later scale change would request the maximized
        // size. The pre-maximize stable size is what a later transition uses.
        let t0 = Instant::now();
        // Seeded stable = 860x691 @1.5.
        let (mut guard, confirmed) = seeded(1.5, t0);
        // Maximized calm resize (huge): ignored, does not become pending/stable.
        assert_eq!(
            guard.observe_resize((1458, 2518), confirmed, MAXIMIZED),
            None
        );
        // A later scale change still requests the PRE-maximize stable size.
        assert_eq!(
            guard.on_scale_change(1.25, confirmed + Duration::from_secs(1), VISIBLE),
            Some((1075, 864))
        );
    }

    #[test]
    fn maximized_scale_change_records_scale_but_requests_nothing() {
        // ScaleFactorChanged while maximized returns None (never resize a
        // maximized window) but still records the scale + storm timing, so a
        // subsequent non-maximized deviating resize is judged against the new
        // scale (1.25) rather than the seeded one (1.5).
        let now = Instant::now();
        let (mut guard, confirmed) = seeded(1.5, now);
        assert_eq!(guard.on_scale_change(1.25, confirmed, MAXIMIZED), None);
        // The scale was recorded: an off-size resize inside the storm snaps to
        // 860x691 @1.25 = 1075x864 (would be 1290x1037 if scale stayed 1.5).
        assert_eq!(
            guard.observe_resize((700, 560), confirmed + Duration::from_millis(5), VISIBLE),
            Some((1075, 864))
        );
    }

    #[test]
    fn hidden_storm_resize_ignored_and_leaves_seeded_stable_intact() {
        // The real race: pre-reveal, the window is still HIDDEN. A storm-window
        // Resized that deviates from the seeded size must NOT emit a correction
        // (a queued request_inner_size can land on the post-reveal maximized
        // window and clobber rcNormalPosition). It returns None and leaves the
        // seeded stable size intact: once visible, corrections use it again.
        let now = Instant::now();
        let (mut guard, confirmed) = seeded(1.5, now);
        // Storm opens while hidden.
        guard.on_scale_change(1.25, confirmed, HIDDEN);
        // Hidden deviating Resized: no correction emitted.
        assert_eq!(
            guard.observe_resize((1425, 1230), confirmed + Duration::from_millis(5), HIDDEN),
            None
        );
        // Once visible, the SAME deviating size inside the storm is corrected to
        // the seeded 860x691 @1.25 = 1075x864 (seeded stable was untouched).
        assert_eq!(
            guard.observe_resize((1425, 1230), confirmed + Duration::from_millis(10), VISIBLE),
            Some((1075, 864))
        );
    }

    #[test]
    fn hidden_calm_resize_never_becomes_tracked_size() {
        // A calm Resized while hidden is only creation-hint churn: it must not
        // become pending/stable, so a later visible scale change still requests
        // the seeded pre-hidden-churn size, not the churned one.
        let t0 = Instant::now();
        // Seeded stable = 860x691 @1.5.
        let (mut guard, confirmed) = seeded(1.5, t0);
        // Hidden calm resize (different size): ignored, not learned.
        assert_eq!(guard.observe_resize((1520, 840), confirmed, HIDDEN), None);
        // A later visible scale change still requests the seeded stable size.
        assert_eq!(
            guard.on_scale_change(1.25, confirmed + Duration::from_secs(1), VISIBLE),
            Some((1075, 864))
        );
    }

    #[test]
    fn hidden_scale_change_records_scale_but_requests_nothing() {
        // ScaleFactorChanged while hidden returns None (never emit a correction
        // that could land post-reveal) but still records the scale + storm
        // timing, so a subsequent visible deviating resize is judged against the
        // new scale (1.25) rather than the seeded one (1.5).
        let now = Instant::now();
        let (mut guard, confirmed) = seeded(1.5, now);
        assert_eq!(guard.on_scale_change(1.25, confirmed, HIDDEN), None);
        // The scale was recorded: a visible off-size resize inside the storm snaps
        // to 860x691 @1.25 = 1075x864 (would be 1290x1037 if scale stayed 1.5).
        assert_eq!(
            guard.observe_resize((700, 560), confirmed + Duration::from_millis(5), VISIBLE),
            Some((1075, 864))
        );
    }
}
