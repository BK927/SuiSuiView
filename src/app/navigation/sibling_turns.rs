use super::sibling_turn_log;
use crate::app::{
    perf, OpenOrigin, QueuedSiblingBookTurn, SuiSuiViewApp, SIBLING_BOOK_TURN_REPAINT_DELAY,
};
use crate::core::worker::NavigationDirection;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const MAX_QUEUED_SIBLING_BOOK_REPEATS: usize = 1;

impl SuiSuiViewApp {
    pub(in crate::app) fn clear_pending_sibling_book_turns(&mut self) {
        let was_reserved = self.sibling_book_turn_reserved();
        if was_reserved {
            sibling_turn_log(|| {
                format!(
                    "clear_pending drops pending={:?} queued={}",
                    self.pending_sibling_book_turn,
                    self.queued_sibling_book_turns.len(),
                )
            });
        }
        self.pending_sibling_book_turn = None;
        self.queued_sibling_book_turns.clear();
        self.sibling_book_wgpu_present_wait = None;
        self.sibling_book_visual_hold_until = None;
        if was_reserved {
            self.refresh_worker_prefetch();
        }
    }

    /// Key-release half of the held-key run: drop only the turns auto-repeat
    /// reserved. The committed turn in `pending_sibling_book_turn` and queued
    /// discrete taps stand for presses the reader made deliberately and outlive
    /// the release -- clearing them here is what used to swallow presses
    /// landing mid-transition.
    pub(in crate::app) fn clear_queued_sibling_book_turns(&mut self) {
        if release_sibling_book_repeats(&mut self.queued_sibling_book_turns) {
            self.refresh_worker_prefetch();
            sibling_turn_log(|| "key release drops one repeat reservation".to_owned());
        }
    }

    pub(in crate::app) fn mark_current_book_visual_painted(&mut self) {
        self.sibling_book_visual_pending = false;
        self.sibling_book_wgpu_present_wait = None;
        self.sibling_book_visual_hold_until = None;
    }

    pub(in crate::app) fn mark_current_book_visual_painted_with_hold(&mut self, hold: Duration) {
        self.sibling_book_visual_pending = false;
        self.sibling_book_wgpu_present_wait = None;
        self.sibling_book_visual_hold_until = Some(Instant::now() + hold);
    }

    /// A deliberate request: a discrete key tap, a click on the edge prompt or
    /// context menu, or an edge-page action. Reserved turns from this path are
    /// never taken back by a key release.
    pub(in crate::app) fn open_sibling_book(&mut self, direction: isize) {
        self.open_sibling_book_from(direction, false);
    }

    /// An auto-repeat press under a held key. Reserved turns from this path are
    /// dropped when the key is released, so letting go ends the run.
    pub(in crate::app) fn open_sibling_book_repeat(&mut self, direction: isize) {
        self.open_sibling_book_from(direction, true);
    }

    fn open_sibling_book_from(&mut self, direction: isize, cancellable: bool) {
        let direction = normalize_sibling_book_direction(direction);
        sibling_turn_log(|| {
            format!(
                "request dir={direction} cancellable={cancellable} pending={:?} queued={} loader_pending={} visual_pending={}",
                self.pending_sibling_book_turn,
                self.queued_sibling_book_turns.len(),
                self.loader_pending,
                self.sibling_book_visual_pending,
            )
        });
        if self.should_queue_sibling_book_turn() {
            self.queue_sibling_book_turn(direction, cancellable);
            return;
        }
        self.open_sibling_book_now(direction);
    }

    fn should_queue_sibling_book_turn(&self) -> bool {
        self.sibling_book_turn_reserved() || self.loader_pending || self.sibling_book_visual_pending
    }

    pub(in crate::app) fn sibling_book_turn_reserved(&self) -> bool {
        self.pending_sibling_book_turn.is_some() || !self.queued_sibling_book_turns.is_empty()
    }

    fn sibling_book_turn_in_progress(&self) -> bool {
        self.loader_pending || self.sibling_book_visual_pending
    }

    pub(in crate::app) fn sibling_book_hold_active(&self) -> bool {
        self.sibling_book_visual_hold_until
            .is_some_and(|until| Instant::now() < until)
    }

    pub(in crate::app) fn sibling_book_transition_stabilizing(&self) -> bool {
        self.loader_pending
            || self.sibling_book_visual_pending
            || self.sibling_book_hold_active()
            || self.sibling_book_turn_reserved()
    }

    fn queue_sibling_book_turn(&mut self, direction: isize, cancellable: bool) {
        self.edge_prompt = None;
        let was_reserved = self.sibling_book_turn_reserved();
        reserve_sibling_book_turn(
            &mut self.pending_sibling_book_turn,
            &mut self.queued_sibling_book_turns,
            direction,
            cancellable,
        );
        if !was_reserved {
            self.refresh_worker_prefetch();
        }
        self.egui_ctx
            .request_repaint_after(SIBLING_BOOK_TURN_REPAINT_DELAY);
    }

    pub(in crate::app) fn drive_queued_sibling_book_turn(&mut self, ctx: &egui::Context) {
        if !self.sibling_book_turn_reserved() {
            return;
        }
        if self.sibling_book_turn_in_progress() {
            ctx.request_repaint_after(SIBLING_BOOK_TURN_REPAINT_DELAY);
            return;
        }
        let Some(direction) = take_sibling_book_turn(
            &mut self.pending_sibling_book_turn,
            &mut self.queued_sibling_book_turns,
        ) else {
            return;
        };
        sibling_turn_log(|| format!("drive takes reserved dir={direction}"));
        self.open_sibling_book_now(direction);
        if self.sibling_book_turn_reserved() || self.sibling_book_turn_in_progress() {
            ctx.request_repaint_after(SIBLING_BOOK_TURN_REPAINT_DELAY);
        }
    }

    fn open_sibling_book_now(&mut self, direction: isize) {
        let _stall_scope =
            crate::core::stall_trace::scope(crate::core::stall_trace::Stage::SiblingNavigation);
        let Some(current) = self.current_book_reference_path() else {
            sibling_turn_log(|| "open_now aborted: no current book".to_owned());
            self.set_status(self.i18n().text("status.no_current_book"));
            return;
        };
        if perf::adjacent_seed_prefetch_enabled() {
            if let Some(cache) = self.take_adjacent_seed_for_direction(direction) {
                sibling_turn_log(|| format!("open_now dir={direction} via prefetched seed"));
                self.install_adjacent_seed_cache(
                    cache,
                    navigation_direction_for_sibling(direction),
                    self.open_view_fallback(),
                    None,
                    crate::app::opening::OpenFailureAction::KeepCurrent,
                );
                return;
            }
            perf::record_adjacent_seed_prefetch_hit(false, self.target_long_edge);
        }
        self.open_sibling_async(current, direction);
    }

    pub(in crate::app) fn current_book_reference_path(&self) -> Option<PathBuf> {
        let source = self.source.as_ref()?;
        match self.open_origin? {
            OpenOrigin::ZipCbz => Some(source.source_path().to_path_buf()),
            OpenOrigin::Folder | OpenOrigin::SingleImage => {
                Some(source.source_path().to_path_buf())
            }
        }
    }
}

fn navigation_direction_for_sibling(direction: isize) -> NavigationDirection {
    if direction < 0 {
        NavigationDirection::Backward
    } else {
        NavigationDirection::Forward
    }
}

fn normalize_sibling_book_direction(direction: isize) -> isize {
    if direction < 0 {
        -1
    } else {
        1
    }
}

/// Preserve discrete presses in order, with at most one speculative repeat.
fn push_queued_sibling_book_turn(
    queue: &mut std::collections::VecDeque<QueuedSiblingBookTurn>,
    direction: isize,
    cancellable: bool,
) {
    let turn = QueuedSiblingBookTurn {
        direction: normalize_sibling_book_direction(direction),
        cancellable,
    };
    // Only held-key repeats have a short reservation limit. Discrete presses
    // are small input records, not decoded pages or parallel source jobs.
    if cancellable && queue.len() >= MAX_QUEUED_SIBLING_BOOK_REPEATS {
        return;
    }
    if !cancellable {
        // A repeat can only enter an empty queue, so it can only be at the front.
        if let Some(slot) = queue.front_mut().filter(|queued| queued.cancellable) {
            *slot = turn;
            return;
        }
    }
    queue.push_back(turn);
}

fn release_sibling_book_repeats(
    queue: &mut std::collections::VecDeque<QueuedSiblingBookTurn>,
) -> bool {
    if queue.front().is_some_and(|turn| turn.cancellable) {
        queue.pop_front();
        true
    } else {
        false
    }
}

/// Reserve a sibling-book turn asked for while another one is still in flight.
///
/// The first reservation is committed. A book open spans the loader thread and
/// the new book's first paint -- far longer than the ~100ms a key spends down --
/// so a press landing in that window is always still reserved when the release
/// arrives. Letting the release take it back dropped the press silently: the
/// reader saw the old book, no status, and had to press again.
///
/// Reservations behind the committed one ride the queue, each remembering
/// whether it may be cancelled: auto-repeat under a held key is dropped on
/// release so letting go ends the run, while a discrete tap is kept -- rapid
/// tap-tap flipping used to lose every tap after the first to the interleaved
/// releases. Pure for testing.
fn reserve_sibling_book_turn(
    pending: &mut Option<isize>,
    queue: &mut std::collections::VecDeque<QueuedSiblingBookTurn>,
    direction: isize,
    cancellable: bool,
) {
    if pending.is_none() && queue.is_empty() {
        *pending = Some(normalize_sibling_book_direction(direction));
        return;
    }
    push_queued_sibling_book_turn(queue, direction, cancellable);
}

/// Committed turn first, then the queue. A queued turn is promoted only as it
/// is taken, so an auto-repeat reservation stays cancellable right up until it
/// runs. Pure for testing.
fn take_sibling_book_turn(
    pending: &mut Option<isize>,
    queue: &mut std::collections::VecDeque<QueuedSiblingBookTurn>,
) -> Option<isize> {
    pending
        .take()
        .or_else(|| queue.pop_front().map(|turn| turn.direction))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    #[test]
    fn sibling_book_direction_normalizes_to_step() {
        assert_eq!(normalize_sibling_book_direction(-4), -1);
        assert_eq!(normalize_sibling_book_direction(0), 1);
        assert_eq!(normalize_sibling_book_direction(3), 1);
    }

    /// Drop only the cancellable (auto-repeat) reservations, exactly what the
    /// sibling-book key release does via `clear_queued_sibling_book_turns`.
    fn release_sibling_book_key(queue: &mut VecDeque<QueuedSiblingBookTurn>) {
        release_sibling_book_repeats(queue);
    }

    #[test]
    fn queued_sibling_book_turns_keep_single_reserved_turn() {
        let mut queue = VecDeque::new();

        push_queued_sibling_book_turn(&mut queue, 1, true);
        push_queued_sibling_book_turn(&mut queue, -1, true);

        assert_eq!(
            queue.into_iter().collect::<Vec<_>>(),
            vec![QueuedSiblingBookTurn {
                direction: 1,
                cancellable: true,
            }]
        );
    }

    #[test]
    fn thirty_discrete_sibling_turns_survive_interleaved_releases() {
        let mut pending = None;
        let mut queue = VecDeque::new();
        for _ in 0..30 {
            reserve_sibling_book_turn(&mut pending, &mut queue, 1, false);
            release_sibling_book_key(&mut queue);
        }
        for _ in 0..30 {
            assert_eq!(take_sibling_book_turn(&mut pending, &mut queue), Some(1));
        }
        assert_eq!(take_sibling_book_turn(&mut pending, &mut queue), None);
    }

    #[test]
    fn new_sibling_taps_do_not_overtake_older_queued_directions() {
        let mut pending = None;
        let mut queue = VecDeque::new();
        reserve_sibling_book_turn(&mut pending, &mut queue, 1, false);
        reserve_sibling_book_turn(&mut pending, &mut queue, -1, false);
        assert_eq!(take_sibling_book_turn(&mut pending, &mut queue), Some(1));
        // The committed slot is empty while that turn opens, but the queue is not.
        reserve_sibling_book_turn(&mut pending, &mut queue, 1, false);
        reserve_sibling_book_turn(&mut pending, &mut queue, -1, true);
        release_sibling_book_key(&mut queue);
        assert_eq!(take_sibling_book_turn(&mut pending, &mut queue), Some(-1));
        assert_eq!(take_sibling_book_turn(&mut pending, &mut queue), Some(1));
        assert_eq!(take_sibling_book_turn(&mut pending, &mut queue), None);
    }

    /// A discrete tap arriving at a full queue replaces an auto-repeat reservation
    /// instead of being dropped: the tap is deliberate, and the repeat was going to
    /// be cancelled by the release anyway.
    #[test]
    fn a_tap_replaces_a_queued_repeat_reservation_at_the_cap() {
        let mut queue = VecDeque::new();

        push_queued_sibling_book_turn(&mut queue, 1, true);
        push_queued_sibling_book_turn(&mut queue, -1, false);

        assert_eq!(
            queue.into_iter().collect::<Vec<_>>(),
            vec![QueuedSiblingBookTurn {
                direction: -1,
                cancellable: false,
            }]
        );
    }

    /// A single press during a book transition must survive its own key release.
    /// The open outlasts the ~100ms the key is down, so the release always arrives
    /// while the turn is still reserved; clearing the reservation there dropped the
    /// press with no status and left the reader on the old book.
    #[test]
    fn a_committed_sibling_book_turn_outlives_the_key_release() {
        let mut pending = None;
        let mut queue = VecDeque::new();

        reserve_sibling_book_turn(&mut pending, &mut queue, 1, false);
        release_sibling_book_key(&mut queue);

        assert_eq!(take_sibling_book_turn(&mut pending, &mut queue), Some(1));
    }

    /// Rapid tap-tap flipping: with the committed slot occupied, the second tap
    /// rides the queue -- and the first tap's release used to clear it, silently
    /// losing every tap after the first during one transition window.
    #[test]
    fn a_second_tap_behind_the_committed_turn_outlives_the_key_release() {
        let mut pending = None;
        let mut queue = VecDeque::new();

        reserve_sibling_book_turn(&mut pending, &mut queue, 1, false);
        reserve_sibling_book_turn(&mut pending, &mut queue, 1, false);
        release_sibling_book_key(&mut queue);
        release_sibling_book_key(&mut queue);

        assert_eq!(take_sibling_book_turn(&mut pending, &mut queue), Some(1));
        assert_eq!(take_sibling_book_turn(&mut pending, &mut queue), Some(1));
        assert_eq!(take_sibling_book_turn(&mut pending, &mut queue), None);
    }

    /// Reservations past the committed turn made by auto-repeat under a held key
    /// are dropped on release, so letting go ends the run instead of coasting on
    /// through unseen books.
    #[test]
    fn releasing_a_held_key_drops_only_the_repeat_reservations() {
        let mut pending = None;
        let mut queue = VecDeque::new();

        reserve_sibling_book_turn(&mut pending, &mut queue, 1, true);
        reserve_sibling_book_turn(&mut pending, &mut queue, 1, true);
        assert_eq!(queue.len(), 1);

        release_sibling_book_key(&mut queue);

        assert_eq!(take_sibling_book_turn(&mut pending, &mut queue), Some(1));
        assert_eq!(take_sibling_book_turn(&mut pending, &mut queue), None);
    }

    #[test]
    fn sibling_book_reservations_normalize_and_run_committed_first() {
        let mut pending = None;
        let mut queue = VecDeque::new();

        reserve_sibling_book_turn(&mut pending, &mut queue, -4, false);
        reserve_sibling_book_turn(&mut pending, &mut queue, 3, false);

        assert_eq!(pending, Some(-1));
        assert_eq!(take_sibling_book_turn(&mut pending, &mut queue), Some(-1));
        assert_eq!(take_sibling_book_turn(&mut pending, &mut queue), Some(1));
        assert_eq!(take_sibling_book_turn(&mut pending, &mut queue), None);
    }
}
