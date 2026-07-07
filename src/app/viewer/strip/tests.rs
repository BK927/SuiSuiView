use super::*;
use egui::pos2;

fn viewport(height: f32) -> Rect {
    Rect::from_min_max(pos2(0.0, 0.0), pos2(100.0, height))
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
    let placements = layout_visible(2, 0.5, viewport(300.0), 10, &height_of);
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
    let placements = layout_visible(3, -0.5, viewport(300.0), 10, &height_of);
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
        layout_visible(0, 0.0, viewport(250.0), len, &height_of)
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
    let placements = layout_visible(0, 0.0, viewport(300.0), 10, &height_of);
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
