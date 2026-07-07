use super::*;
use crate::core::state::FitMode;
use egui::{pos2, vec2};

fn viewport(height: f32) -> Rect {
    Rect::from_min_max(pos2(0.0, 0.0), pos2(100.0, height))
}

/// Full-width column matching the 100pt-wide test viewport, i.e. the FitWidth
/// column that reproduces the pre-column-model layout.
fn full_width_layout(
    anchor_index: usize,
    offset_frac: f32,
    viewport: Rect,
    page_count: usize,
    height_of: &impl Fn(usize) -> f32,
) -> Vec<StripPlacement> {
    layout_visible(
        anchor_index,
        offset_frac,
        viewport,
        viewport.left(),
        viewport.width(),
        page_count,
        height_of,
    )
}

fn uniform(height: f32) -> impl Fn(usize) -> f32 {
    move |_| height
}

#[test]
fn display_height_is_fit_width() {
    assert_eq!(
        display_height(StripPageDims::Exact([100, 200]), 50.0, 999.0),
        100.0
    );
    assert_eq!(
        display_height(StripPageDims::Hint([100, 200]), 50.0, 999.0),
        100.0
    );
}

#[test]
fn display_height_falls_back_on_unknown_and_degenerate() {
    assert_eq!(display_height(StripPageDims::Unknown, 50.0, 999.0), 999.0);
    // Zero width is degenerate; the fit-width ratio is undefined, so fall back.
    assert_eq!(
        display_height(StripPageDims::Exact([0, 200]), 50.0, 999.0),
        999.0
    );
}

#[test]
fn median_known_height_handles_parity_and_empty() {
    assert_eq!(median_known_height(std::iter::empty()), None);
    assert_eq!(median_known_height([100.0].into_iter()), Some(100.0));
    assert_eq!(
        median_known_height([300.0, 100.0, 200.0].into_iter()),
        Some(200.0)
    );
    assert_eq!(
        median_known_height([400.0, 100.0, 300.0, 200.0].into_iter()),
        Some(250.0)
    );
}

#[test]
fn layout_places_anchor_exactly_and_walks_down() {
    let height_of = uniform(100.0);
    let placements = full_width_layout(2, 0.5, viewport(300.0), 10, &height_of);
    // Anchor is topmost (offset in range), so no page is prepended above it.
    assert_eq!(
        placements.iter().map(|p| p.index).collect::<Vec<_>>(),
        vec![2, 3, 4, 5, 6]
    );
    // Anchor top = viewport.top() - offset * height = 0 - 0.5 * 100 = -50.
    assert_eq!(placements[0].rect.top(), -50.0);
    assert_eq!(placements[0].rect.bottom(), 50.0);
    assert_eq!(placements[0].rect.left(), 0.0);
    assert_eq!(placements[0].rect.right(), 100.0);
    // Pages below stack with zero gap, accumulated from the anchor downward.
    assert_eq!(placements[1].rect.top(), 50.0);
    assert_eq!(placements[2].rect.top(), 150.0);
    assert_eq!(placements[3].rect.top(), 250.0);
    assert_eq!(placements[4].rect.top(), 350.0);
}

#[test]
fn layout_walks_up_when_anchor_is_not_topmost() {
    // A negative offset is outside the documented [0,1) domain, but exercises
    // the defensive walk-up: the anchor's top sits below the viewport top, so
    // the page above peeks in and must be prepended.
    let height_of = uniform(100.0);
    let placements = full_width_layout(3, -0.5, viewport(300.0), 10, &height_of);
    assert_eq!(placements[0].index, 2);
    assert_eq!(placements[0].rect.top(), -50.0);
    assert_eq!(placements[1].index, 3);
    assert_eq!(placements[1].rect.top(), 50.0);
}

#[test]
fn layout_on_screen_rects_ignore_far_page_heights() {
    let viewport_width = 100.0;
    let fallback = 150.0;
    let mut dims = vec![
        StripPageDims::Exact([100, 120]),
        StripPageDims::Hint([100, 90]),
        StripPageDims::Unknown,
        StripPageDims::Exact([100, 100]),
        StripPageDims::Hint([100, 110]),
    ];
    dims.extend(std::iter::repeat_n(StripPageDims::Exact([100, 100]), 10));

    let layout = |dims: &[StripPageDims]| {
        let dims = dims.to_vec();
        let len = dims.len();
        let height_of = move |index: usize| display_height(dims[index], viewport_width, fallback);
        full_width_layout(0, 0.0, viewport(250.0), len, &height_of)
    };
    let before = layout(&dims);

    // Mutating a page far below the visible window (never visited by the
    // walk) must leave every on-screen rect byte-identical.
    dims[12] = StripPageDims::Exact([100, 9000]);
    let after = layout(&dims);
    assert_eq!(before, after);
    // Sanity: the visible window really is only the near pages plus margin.
    assert!(before.iter().all(|p| p.index <= 4));
}

#[test]
fn scroll_renormalizes_forward_across_boundaries() {
    let height_of = uniform(100.0);
    let (index, offset, edge) = scroll_by(0, 0.0, 250.0, 300.0, 10, &height_of);
    assert_eq!(index, 2);
    assert_eq!(offset, 0.5);
    assert_eq!(edge, None);
}

#[test]
fn scroll_renormalizes_backward_across_boundaries() {
    let height_of = uniform(100.0);
    let (index, offset, edge) = scroll_by(5, 0.0, -250.0, 300.0, 10, &height_of);
    assert_eq!(index, 2);
    assert_eq!(offset, 0.5);
    assert_eq!(edge, None);
}

#[test]
fn scroll_clamps_at_top_and_reports_only_when_beyond() {
    let height_of = uniform(100.0);
    // Tried to scroll above page 0: clamp and report the Backward edge.
    let beyond = scroll_by(0, 0.2, -100.0, 300.0, 10, &height_of);
    assert_eq!(beyond, (0, 0.0, Some(NavigationDirection::Backward)));
    // Landed exactly on the top without overshooting: no edge report.
    let exact = scroll_by(1, 0.0, -100.0, 300.0, 10, &height_of);
    assert_eq!(exact, (0, 0.0, None));
}

#[test]
fn scroll_clamps_at_bottom_pinning_last_page() {
    let height_of = uniform(100.0);
    // Overshoot far past the end; the last three 100px pages exactly fill the
    // 300px viewport, so the bottom-pinned anchor is page 7 at offset 0.
    let clamped = scroll_by(8, 0.5, 500.0, 300.0, 10, &height_of);
    assert_eq!(clamped, (7, 0.0, Some(NavigationDirection::Forward)));
}

#[test]
fn scroll_book_shorter_than_viewport_clamps_to_top() {
    let height_of = uniform(100.0);
    // Two 100px pages, 300px viewport: the book cannot fill the viewport.
    let down = scroll_by(0, 0.0, 50.0, 300.0, 2, &height_of);
    assert_eq!(down, (0, 0.0, Some(NavigationDirection::Forward)));
    let up = scroll_by(0, 0.0, -50.0, 300.0, 2, &height_of);
    assert_eq!(up, (0, 0.0, Some(NavigationDirection::Backward)));
}

#[test]
fn page_at_center_prefers_containing_placement() {
    let height_of = uniform(100.0);
    let placements = full_width_layout(0, 0.0, viewport(300.0), 10, &height_of);
    // Center y = 150 falls inside page 1 (100..200).
    assert_eq!(
        page_at_viewport_center(&placements, viewport(300.0)),
        Some(1)
    );
}

#[test]
fn page_at_center_falls_back_to_nearest_over_a_gap() {
    let placements = vec![
        StripPlacement {
            index: 5,
            rect: Rect::from_min_max(pos2(0.0, 0.0), pos2(100.0, 40.0)),
        },
        StripPlacement {
            index: 6,
            rect: Rect::from_min_max(pos2(0.0, 200.0), pos2(100.0, 300.0)),
        },
    ];
    // Center y = 150 is over neither rect; page 6's center (250) is nearer.
    assert_eq!(
        page_at_viewport_center(&placements, viewport(300.0)),
        Some(6)
    );
    assert_eq!(page_at_viewport_center(&[], viewport(300.0)), None);
}

#[test]
fn jump_to_page_lands_at_page_top() {
    assert_eq!(jump_to_page(7), (7, 0.0));
}

#[test]
fn recenter_target_fires_only_on_integer_change() {
    assert_eq!(recenter_target(3, 3), None);
    assert_eq!(recenter_target(3, 4), Some(4));
    assert_eq!(recenter_target(4, 2), Some(2));
}

#[test]
fn overscroll_below_threshold_does_not_fire() {
    let now = Instant::now();
    let mut px = 0.0;
    let mut at = None;
    assert!(!accumulate_edge_overscroll(
        &mut px,
        &mut at,
        NavigationDirection::Forward,
        100.0,
        now
    ));
    assert!(!accumulate_edge_overscroll(
        &mut px,
        &mut at,
        NavigationDirection::Forward,
        100.0,
        now
    ));
    // 200 < 240: still short of a page turn, running total preserved.
    assert_eq!(px, 200.0);
}

#[test]
fn overscroll_crossing_threshold_fires_once_then_resets() {
    let now = Instant::now();
    let mut px = 0.0;
    let mut at = None;
    assert!(!accumulate_edge_overscroll(
        &mut px,
        &mut at,
        NavigationDirection::Forward,
        200.0,
        now
    ));
    // 400 >= 240: fires and resets so the next push starts fresh.
    assert!(accumulate_edge_overscroll(
        &mut px,
        &mut at,
        NavigationDirection::Forward,
        200.0,
        now
    ));
    assert_eq!(px, 0.0);
    assert_eq!(at, None);
    assert!(!accumulate_edge_overscroll(
        &mut px,
        &mut at,
        NavigationDirection::Forward,
        100.0,
        now
    ));
}

#[test]
fn overscroll_window_expiry_forgets_partial_accumulation() {
    let now = Instant::now();
    let mut px = 0.0;
    let mut at = None;
    assert!(!accumulate_edge_overscroll(
        &mut px,
        &mut at,
        NavigationDirection::Forward,
        200.0,
        now
    ));
    // A push after the window lapsed drops the stale 200 before adding 200,
    // so 200 < 240 does not fire.
    let later = now + Duration::from_millis(600);
    assert!(!accumulate_edge_overscroll(
        &mut px,
        &mut at,
        NavigationDirection::Forward,
        200.0,
        later
    ));
    assert_eq!(px, 200.0);
}

#[test]
fn overscroll_direction_reversal_resets_before_accumulating() {
    let now = Instant::now();
    let mut px = 0.0;
    let mut at = None;
    assert!(!accumulate_edge_overscroll(
        &mut px,
        &mut at,
        NavigationDirection::Forward,
        200.0,
        now
    ));
    // Reversing to the top edge abandons the forward total, so |-200| < 240.
    assert!(!accumulate_edge_overscroll(
        &mut px,
        &mut at,
        NavigationDirection::Backward,
        200.0,
        now
    ));
    assert_eq!(px, -200.0);
}

#[test]
fn smooth_scroll_step_drains_monotonically_and_keeps_sign() {
    let (step, remaining) = smooth_scroll_step(160.0, 1.0 / 60.0, STRIP_SCROLL_DECAY_PER_SEC);
    assert!(step > 0.0 && step < 160.0);
    assert!((step + remaining - 160.0).abs() < 1e-3);

    let (neg_step, neg_remaining) =
        smooth_scroll_step(-160.0, 1.0 / 60.0, STRIP_SCROLL_DECAY_PER_SEC);
    assert!(neg_step < 0.0 && neg_remaining < 0.0);
    assert!((neg_step - -step).abs() < 1e-3);
}

#[test]
fn smooth_scroll_step_snaps_the_tail_and_terminates() {
    // A sub-snap debt is applied whole.
    assert_eq!(
        smooth_scroll_step(0.4, 1.0 / 60.0, STRIP_SCROLL_DECAY_PER_SEC),
        (0.4, 0.0)
    );
    assert_eq!(
        smooth_scroll_step(0.0, 1.0 / 60.0, STRIP_SCROLL_DECAY_PER_SEC),
        (0.0, 0.0)
    );

    // A full wheel notch drains to exactly zero within a bounded frame count.
    let mut pending = 160.0;
    let mut frames = 0;
    while pending != 0.0 {
        let (_, remaining) = smooth_scroll_step(pending, 1.0 / 60.0, STRIP_SCROLL_DECAY_PER_SEC);
        pending = remaining;
        frames += 1;
        assert!(frames < 120, "animation must terminate");
    }
    assert!(frames > 3, "a notch should glide over several frames");
}

#[test]
fn column_width_maps_each_fit_mode_for_a_tall_median() {
    let viewport = vec2(1000.0, 800.0);
    let tall = Some([690, 1600]);
    let width = |mode, zoom| column_width(mode, zoom, viewport, tall, 1.0);
    assert_eq!(width(FitMode::FitWidth, 1.0), 1000.0);
    // FitHeight = viewport_h * median_w / median_h = 800 * 690 / 1600.
    assert_eq!(width(FitMode::FitHeight, 1.0), 345.0);
    // FitPage picks the narrower of the two (FitHeight here).
    assert_eq!(width(FitMode::FitPage, 1.0), 345.0);
    // Original = median native width in points (median_w / ppp).
    assert_eq!(width(FitMode::Original, 1.0), 690.0);
    // Manual = manual_zoom * Original column.
    assert_eq!(width(FitMode::Manual, 2.0), 1380.0);
}

#[test]
fn column_width_maps_each_fit_mode_for_a_wide_median() {
    let viewport = vec2(1000.0, 800.0);
    let wide = Some([2000, 800]);
    let width = |mode, zoom| column_width(mode, zoom, viewport, wide, 1.0);
    assert_eq!(width(FitMode::FitWidth, 1.0), 1000.0);
    // FitHeight = 800 * 2000 / 800 = 2000 (a full wide page spans one viewport
    // height, so its column overflows the viewport and pans horizontally).
    assert_eq!(width(FitMode::FitHeight, 1.0), 2000.0);
    // FitPage picks the narrower (FitWidth here).
    assert_eq!(width(FitMode::FitPage, 1.0), 1000.0);
    assert_eq!(width(FitMode::Original, 1.0), 2000.0);
    assert_eq!(width(FitMode::Manual, 0.5), 1000.0);
}

#[test]
fn column_width_degrades_to_viewport_width_without_median() {
    let viewport = vec2(1000.0, 800.0);
    for mode in [
        FitMode::FitWidth,
        FitMode::FitHeight,
        FitMode::FitPage,
        FitMode::Original,
        FitMode::Manual,
    ] {
        assert_eq!(column_width(mode, 3.0, viewport, None, 1.0), 1000.0);
        // A zero-sized median is treated as unknown, too.
        assert_eq!(column_width(mode, 3.0, viewport, Some([0, 0]), 1.0), 1000.0);
    }
}

#[test]
fn column_width_clamps_to_a_sane_span() {
    let viewport = vec2(1000.0, 800.0);
    // A tiny native page clamps up to the readable minimum.
    assert_eq!(
        column_width(FitMode::Original, 1.0, viewport, Some([10, 5000]), 1.0),
        64.0
    );
    // A pathologically tall-narrow median under FitHeight clamps to 16x the
    // viewport width rather than an absurd column.
    assert_eq!(
        column_width(FitMode::FitHeight, 1.0, viewport, Some([10_000, 100]), 1.0),
        16_000.0
    );
    // The Original-column scales with device pixels: half the points at 2x ppp.
    assert_eq!(
        column_width(FitMode::Original, 1.0, viewport, Some([690, 1600]), 2.0),
        345.0
    );
}

#[test]
fn clamp_pan_x_pins_a_fitting_column_and_bounds_an_overflowing_one() {
    // Column no wider than the viewport never pans.
    assert_eq!(clamp_pan_x(500.0, 800.0, 1000.0), 0.0);
    assert_eq!(clamp_pan_x(-500.0, 1000.0, 1000.0), 0.0);
    // Overflowing column pans up to +/- half the overflow.
    assert_eq!(clamp_pan_x(500.0, 1400.0, 1000.0), 200.0);
    assert_eq!(clamp_pan_x(-500.0, 1400.0, 1000.0), -200.0);
    assert_eq!(clamp_pan_x(120.0, 1400.0, 1000.0), 120.0);
}

#[test]
fn layout_centers_narrower_column_and_scales_heights() {
    let dims = StripPageDims::Exact([100, 200]); // aspect 2.0
    let vp = viewport(300.0);
    let wide = layout_visible(0, 0.0, vp, 0.0, 100.0, 1, &|_| {
        display_height(dims, 100.0, 0.0)
    });
    let narrow_left = vp.center().x - 40.0 / 2.0; // 50 - 20 = 30
    let narrow = layout_visible(0, 0.0, vp, narrow_left, 40.0, 1, &|_| {
        display_height(dims, 40.0, 0.0)
    });
    // Full-width column spans the viewport, height = 100 * 200 / 100 = 200.
    assert_eq!(wide[0].rect.left(), 0.0);
    assert_eq!(wide[0].rect.right(), 100.0);
    assert_eq!(wide[0].rect.height(), 200.0);
    // Narrow column is centered (30..70) with height scaled to it: 40*200/100.
    assert_eq!(narrow[0].rect.left(), 30.0);
    assert_eq!(narrow[0].rect.right(), 70.0);
    assert_eq!(narrow[0].rect.height(), 80.0);
    assert_eq!(narrow[0].rect.center().x, vp.center().x);
}

#[test]
fn anchor_offset_frac_is_preserved_across_column_change() {
    let dims = StripPageDims::Exact([100, 200]); // aspect 2.0
    let vp = viewport(300.0);
    let offset = 0.25;
    // Fraction of the anchor page's height sitting above the viewport top.
    let fraction_above = |column: f32| {
        let height = display_height(dims, column, 0.0);
        let placements = layout_visible(
            0,
            offset,
            vp,
            vp.center().x - column / 2.0,
            column,
            1,
            &|_| height,
        );
        (vp.top() - placements[0].rect.top()) / placements[0].rect.height()
    };
    // Because the offset is a fraction of the page height, both columns keep the
    // same fractional reading position regardless of how tall the page renders.
    assert!((fraction_above(100.0) - offset).abs() < 1e-6);
    assert!((fraction_above(40.0) - offset).abs() < 1e-6);
}

#[test]
fn flick_debt_thresholds_scales_and_caps() {
    // A slow positioning release adds no inertia.
    assert_eq!(flick_debt(100.0, 800.0), 0.0);
    // A real flick coasts v / flick-decay in the release direction.
    let debt = flick_debt(2000.0, 800.0);
    assert!((debt - 2000.0 / STRIP_FLICK_DECAY_PER_SEC).abs() < 1e-3);
    assert!(flick_debt(-2000.0, 800.0) < 0.0);
    // A spurious velocity spike is capped to a few viewports.
    assert_eq!(flick_debt(1_000_000.0, 800.0), 3.0 * 800.0);
}

#[test]
fn flick_decay_coasts_longer_than_wheel_decay() {
    let frames_to_drain = |decay: f32| {
        let mut pending = 500.0;
        let mut frames = 0;
        while pending != 0.0 {
            let (_, remaining) = smooth_scroll_step(pending, 1.0 / 60.0, decay);
            pending = remaining;
            frames += 1;
            assert!(frames < 600, "must terminate");
        }
        frames
    };
    assert!(
        frames_to_drain(STRIP_FLICK_DECAY_PER_SEC) > frames_to_drain(STRIP_SCROLL_DECAY_PER_SEC)
    );
}
