use super::super::QueuedPageTurns;
use super::{
    normalize_sibling_book_direction, plain_forward_step, push_queued_page_turn,
    push_queued_sibling_book_turn, reserve_sibling_book_turn, should_open_edge_prompt,
    skip_missing_target, take_sibling_book_turn, toggled_double_mode, zoom_motion_active,
    EdgePrompt, MAX_QUEUED_PAGE_TURNS, MAX_QUEUED_SIBLING_BOOK_TURNS, ZOOM_SETTLE_MS,
};
use crate::app::ViewMode;
use crate::core::state::ReadingDirection;
use crate::core::worker::NavigationDirection;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

#[test]
fn zoom_motion_is_active_only_inside_the_settle_window() {
    let now = Instant::now();
    let settle = Duration::from_millis(ZOOM_SETTLE_MS);

    // No motion ever recorded is never in motion.
    assert!(!zoom_motion_active(None, now));
    // A change at `now` is in motion, and stays so partway through the window.
    assert!(zoom_motion_active(Some(now), now));
    assert!(zoom_motion_active(Some(now), now + settle / 2));
    // At and past the window boundary the gesture has settled.
    assert!(!zoom_motion_active(Some(now), now + settle));
    assert!(!zoom_motion_active(
        Some(now),
        now + settle + Duration::from_millis(1)
    ));
}

#[test]
fn plain_forward_step_ends_the_book_on_a_full_final_spread() {
    // 10 pages, two-page mode: the [8, 9] spread already shows the last page, so
    // the turn must end the book instead of re-showing page 9 alone.
    assert_eq!(plain_forward_step(8, 2, 9), None);
    assert_eq!(plain_forward_step(6, 2, 9), Some(8));
}

#[test]
fn plain_forward_step_still_reaches_a_lone_trailing_page() {
    // 9 pages, two-page mode: the [6, 7] spread leaves page 8 unseen.
    assert_eq!(plain_forward_step(6, 2, 8), Some(8));
    assert_eq!(plain_forward_step(8, 2, 8), None);
}

#[test]
fn plain_forward_step_is_unchanged_for_single_page_mode() {
    assert_eq!(plain_forward_step(0, 1, 2), Some(1));
    assert_eq!(plain_forward_step(1, 1, 2), Some(2));
    assert_eq!(plain_forward_step(2, 1, 2), None);
}

#[test]
fn toggle_double_uses_reading_direction_when_entering_double_mode() {
    for current in [ViewMode::Single, ViewMode::VerticalStrip] {
        assert_eq!(
            toggled_double_mode(current, ReadingDirection::LeftToRight),
            ViewMode::DoubleLeftToRight
        );
        assert_eq!(
            toggled_double_mode(current, ReadingDirection::RightToLeft),
            ViewMode::DoubleRightToLeft
        );
    }
}

#[test]
fn toggle_double_returns_every_double_mode_to_single() {
    for current in [
        ViewMode::DoubleLeftToRight,
        ViewMode::DoubleRightToLeft,
        ViewMode::SmartDoubleLeftToRight,
        ViewMode::SmartDoubleRightToLeft,
    ] {
        assert_eq!(
            toggled_double_mode(current, ReadingDirection::LeftToRight),
            ViewMode::Single
        );
    }
}

#[test]
fn sibling_book_direction_normalizes_to_step() {
    assert_eq!(normalize_sibling_book_direction(-4), -1);
    assert_eq!(normalize_sibling_book_direction(0), 1);
    assert_eq!(normalize_sibling_book_direction(3), 1);
}

#[test]
fn queued_sibling_book_turns_keep_single_reserved_turn() {
    let mut queue = VecDeque::new();

    push_queued_sibling_book_turn(&mut queue, 1);
    push_queued_sibling_book_turn(&mut queue, -1);
    push_queued_sibling_book_turn(&mut queue, 1);

    assert_eq!(queue.into_iter().collect::<Vec<_>>(), vec![1]);
}

#[test]
fn queued_sibling_book_turns_are_capped() {
    let mut queue = VecDeque::new();

    for _ in 0..MAX_QUEUED_SIBLING_BOOK_TURNS + 8 {
        push_queued_sibling_book_turn(&mut queue, 1);
    }

    assert_eq!(queue.len(), MAX_QUEUED_SIBLING_BOOK_TURNS);
}

/// A single press during a book transition must survive its own key release.
/// The open outlasts the ~100ms the key is down, so the release always arrives
/// while the turn is still reserved; clearing the reservation there dropped the
/// press with no status and left the reader on the old book.
#[test]
fn a_committed_sibling_book_turn_outlives_the_key_release() {
    let mut pending = None;
    let mut queue = VecDeque::new();

    reserve_sibling_book_turn(&mut pending, &mut queue, 1);
    // The key release clears only the auto-repeat queue.
    queue.clear();

    assert_eq!(take_sibling_book_turn(&mut pending, &mut queue), Some(1));
}

/// Everything past the committed turn is auto-repeat under a held key, so the
/// release still ends the run instead of coasting on through unseen books.
#[test]
fn releasing_a_held_key_drops_only_the_repeat_reservations() {
    let mut pending = None;
    let mut queue = VecDeque::new();

    reserve_sibling_book_turn(&mut pending, &mut queue, 1);
    reserve_sibling_book_turn(&mut pending, &mut queue, 1);
    assert_eq!(queue.len(), 1);

    queue.clear();

    assert_eq!(take_sibling_book_turn(&mut pending, &mut queue), Some(1));
    assert_eq!(take_sibling_book_turn(&mut pending, &mut queue), None);
}

#[test]
fn sibling_book_reservations_normalize_and_run_committed_first() {
    let mut pending = None;
    let mut queue = VecDeque::new();

    reserve_sibling_book_turn(&mut pending, &mut queue, -4);
    reserve_sibling_book_turn(&mut pending, &mut queue, 3);

    assert_eq!(pending, Some(-1));
    assert_eq!(queue.iter().copied().collect::<Vec<_>>(), vec![1]);
    assert_eq!(take_sibling_book_turn(&mut pending, &mut queue), Some(-1));
    assert_eq!(take_sibling_book_turn(&mut pending, &mut queue), Some(1));
    assert_eq!(take_sibling_book_turn(&mut pending, &mut queue), None);
}

#[test]
fn queued_page_turns_are_capped() {
    let mut queue = None;

    for _ in 0..MAX_QUEUED_PAGE_TURNS + 8 {
        push_queued_page_turn(&mut queue, NavigationDirection::Forward);
    }

    assert_eq!(
        queue,
        Some(QueuedPageTurns {
            direction: NavigationDirection::Forward,
            remaining: MAX_QUEUED_PAGE_TURNS,
        })
    );
}

#[test]
fn opposite_page_turn_clears_single_queued_turn() {
    let mut queue = Some(QueuedPageTurns {
        direction: NavigationDirection::Forward,
        remaining: 1,
    });

    push_queued_page_turn(&mut queue, NavigationDirection::Backward);

    assert_eq!(queue, None);
}

#[test]
fn edge_prompt_reuses_same_direction_timer() {
    let prompt = EdgePrompt::new(NavigationDirection::Backward);

    assert!(!should_open_edge_prompt(
        Some(prompt),
        NavigationDirection::Backward
    ));
    assert!(should_open_edge_prompt(
        Some(prompt),
        NavigationDirection::Forward
    ));
}

fn forward_step(page: usize, _dir: NavigationDirection) -> Option<usize> {
    (page < 10).then(|| page + 1)
}

fn backward_step(page: usize, _dir: NavigationDirection) -> Option<usize> {
    (page > 0).then(|| page - 1)
}

#[test]
fn skip_missing_returns_start_when_present() {
    let target = skip_missing_target(5, NavigationDirection::Forward, |_| true, forward_step, 10);
    assert_eq!(target, Some(5));
}

#[test]
fn skip_missing_slides_over_single_gap_forward() {
    let target = skip_missing_target(
        3,
        NavigationDirection::Forward,
        |i| i != 3,
        forward_step,
        10,
    );
    assert_eq!(target, Some(4));
}

#[test]
fn skip_missing_slides_over_run_forward() {
    let target = skip_missing_target(
        2,
        NavigationDirection::Forward,
        |i| i >= 6,
        forward_step,
        10,
    );
    assert_eq!(target, Some(6));
}

#[test]
fn skip_missing_all_forward_gone_returns_none() {
    let target = skip_missing_target(
        0,
        NavigationDirection::Forward,
        |_| false,
        forward_step,
        100,
    );
    assert_eq!(target, None);
}

#[test]
fn skip_missing_backward_hits_boundary() {
    let target = skip_missing_target(
        3,
        NavigationDirection::Backward,
        |_| false,
        backward_step,
        100,
    );
    assert_eq!(target, None);
}

#[test]
fn skip_missing_respects_max_steps() {
    let target = skip_missing_target(
        0,
        NavigationDirection::Forward,
        |_| false,
        |page, _dir| Some(page + 1),
        4,
    );
    assert_eq!(target, None);
}
