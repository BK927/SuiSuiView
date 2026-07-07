use super::super::QueuedPageTurns;
use super::{
    normalize_sibling_book_direction, push_queued_page_turn, push_queued_sibling_book_turn,
    should_open_edge_prompt, skip_missing_target, zoom_motion_active, EdgePrompt,
    MAX_QUEUED_PAGE_TURNS, MAX_QUEUED_SIBLING_BOOK_TURNS, ZOOM_SETTLE_MS,
};
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
