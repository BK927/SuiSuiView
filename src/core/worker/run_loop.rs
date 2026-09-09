#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
use super::cache::record_worker_cache_snapshot;
use super::cache::{
    insert_worker_cache_with_budget, page_cache_key, prune_worker_cache,
    remember_published_app_cache_hint, should_skip_published_app_cache_hint,
};
use super::decode_ahead::{self, consume_matching_decode};
use super::delivery::{DeliveryOutcome, EventSender};
use super::prepare::prepare_page_with_perf;
use super::read_ahead;
use super::scheduler::{
    is_visible_page_index, prioritized_jobs, should_skip_ai_preview_or_prefetch, PageJob,
};
use super::session::WorkerSession;
use super::source_bytes::read_source_bytes;
use super::{CachedPageKey, WorkerCommand, WorkerEvent};
#[cfg(test)]
use super::{NavigationDirection, WorkerOptions};
#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
use crate::core::perf_trace::{self, PerfField};
use crate::core::source::SharedSource;
use crossbeam_channel::Receiver;
use egui::Context;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
use std::time::Duration;

pub(in crate::core::worker) fn run_worker(
    command_rx: Receiver<WorkerCommand>,
    event_tx: EventSender,
    ctx: Context,
    shutdown_requested: Arc<AtomicBool>,
) {
    let mut state = WorkerSession::default();
    while !shutdown_requested.load(Ordering::Acquire) {
        let Ok(command) = command_rx.recv() else {
            break;
        };
        if !state.apply(command) {
            break;
        }

        'work: loop {
            if shutdown_requested.load(Ordering::Acquire) {
                return;
            }
            let Some(source) = state.source.clone() else {
                break;
            };
            let book_id = source.book_id().to_owned();
            let jobs = prioritized_jobs(
                state.center,
                source.page_count(),
                state.direction,
                state.target_long_edge,
                state.options.target_intent,
                state.visible_pages,
                state.options.prefetch_enabled,
                state.options.progressive_preview_enabled,
            );
            state.reconcile(&source, &jobs);

            for (position, job) in jobs.iter().copied().enumerate() {
                if shutdown_requested.load(Ordering::Acquire) {
                    return;
                }
                if let Some(command) = drain_latest_command(&command_rx) {
                    if !state.apply(command) {
                        return;
                    }
                    continue 'work;
                }
                let Some(page_id) = source.page_id(job.index) else {
                    continue;
                };
                if should_skip_ai_preview_or_prefetch(
                    source.page_name(job.index),
                    state.center,
                    state.visible_pages,
                    job.index,
                    job.target_long_edge,
                ) {
                    continue;
                }

                let visible = is_visible_page_index(job.index, state.center, state.visible_pages);
                let key = page_cache_key(
                    &book_id,
                    source.source_cache_id(),
                    page_id,
                    job.target_long_edge,
                    state.options.decode,
                );
                #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
                perf_trace::record_duration(
                    "page_worker_job_start",
                    Duration::ZERO,
                    &[
                        PerfField::Usize("page", job.index),
                        PerfField::Usize("book_epoch", state.book_epoch),
                        PerfField::U32("target_long_edge", job.target_long_edge),
                    ],
                );

                // Weak observations avoid redundant prefetch. They do not prove
                // the UI still owns a page: the worker may be its last owner.
                // Always satisfy visible requests, reusing worker pixels below.
                if !visible
                    && state
                        .options
                        .app_cache_covers(page_id, job.target_long_edge)
                {
                    continue;
                }
                let cached = state
                    .take_page(page_id, job.target_long_edge)
                    .or_else(|| state.cache.get(&key).cloned());
                let cache_hit = cached.is_some();
                let page = if let Some(page) = cached {
                    page
                } else {
                    if should_skip_published_app_cache_hint(
                        &state.published_app_cache_hints,
                        visible,
                        page_id,
                        job.target_long_edge,
                        state.options.decode,
                    ) {
                        continue;
                    }

                    let prepared = if let Some(result) = consume_matching_decode(
                        &mut state.decode_ahead,
                        &book_id,
                        state.book_epoch,
                        page_id,
                        job.target_long_edge,
                        state.options.decode,
                    ) {
                        result
                    } else {
                        let bytes = state.take_read(page_id).unwrap_or_else(|| {
                            read_source_bytes(
                                state.source_bytes_cache.as_mut(),
                                &mut state.read_ahead,
                                &source,
                                &book_id,
                                state.book_epoch,
                                page_id,
                                job.index,
                            )
                        });
                        if shutdown_requested.load(Ordering::Acquire) {
                            return;
                        }
                        if !visible && !command_rx.is_empty() {
                            // Replan first, but keep useful bytes in the same
                            // snapshot instead of repeating the completed read.
                            state.keep_read(page_id, bytes);
                            if let Some(command) = drain_latest_command(&command_rx) {
                                if !state.apply(command) {
                                    return;
                                }
                            }
                            continue 'work;
                        }
                        if bytes.is_ok() {
                            start_overlap(
                                &mut state,
                                &source,
                                &jobs,
                                position + 1,
                                job,
                                &command_rx,
                            );
                        }
                        bytes.and_then(|bytes| {
                            prepare_page_with_perf(
                                bytes.as_ref(),
                                job,
                                state.book_epoch,
                                state.options.decode,
                                false,
                                state
                                    .decode_ahead_policy
                                    .needs_prepare_timing_for(&source, job.index),
                            )
                        })
                    };
                    if shutdown_requested.load(Ordering::Acquire) {
                        return;
                    }
                    match prepared {
                        Ok(prepared) => {
                            let page = Arc::new(prepared.page);
                            state.decode_ahead_policy.observe_prepare(
                                &source,
                                job.index,
                                page.as_ref(),
                                prepared.prepare_duration,
                            );
                            if state.decode_ahead_policy.candidate().is_none() {
                                decode_ahead::cancel_pending_decode(
                                    &mut state.decode_ahead,
                                    "policy",
                                );
                            }
                            if insert_worker_cache_with_budget(
                                &mut state.cache,
                                &mut state.cache_bytes,
                                key,
                                &page,
                                state.options.cache_bytes,
                            ) {
                                prune_worker_cache(
                                    &mut state.cache,
                                    &mut state.cache_bytes,
                                    state.options.cache_bytes,
                                );
                            }
                            page
                        }
                        Err(message) => {
                            let event = WorkerEvent::PageFailed {
                                book_id: book_id.clone(),
                                source_instance_id: source.source_instance_id(),
                                page_id,
                                target_long_edge: job.target_long_edge,
                                decode: state.options.decode,
                                message,
                            };
                            match publish(&mut state, &event_tx, event, &command_rx, &ctx) {
                                Publish::Sent => continue,
                                Publish::Restart => continue 'work,
                                Publish::Stopped => return,
                            }
                        }
                    }
                };
                remember_published_app_cache_hint(
                    &mut state.published_app_cache_hints,
                    CachedPageKey::new(page_id, job.target_long_edge, state.options.decode),
                    &page,
                );
                let event = WorkerEvent::PageReady {
                    book_id: book_id.clone(),
                    source_instance_id: source.source_instance_id(),
                    page_id,
                    decode: state.options.decode,
                    page,
                };
                match publish(&mut state, &event_tx, event, &command_rx, &ctx) {
                    Publish::Sent => {}
                    Publish::Restart => continue 'work,
                    Publish::Stopped => return,
                }
                #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
                {
                    perf_trace::record_duration(
                        "page_worker_publish",
                        Duration::ZERO,
                        &[
                            PerfField::Usize("page", job.index),
                            PerfField::Usize("book_epoch", state.book_epoch),
                            PerfField::U32("target_long_edge", job.target_long_edge),
                            PerfField::Bool("cache_hit", cache_hit),
                        ],
                    );
                    record_worker_cache_snapshot(
                        if cache_hit {
                            "publish_hit"
                        } else {
                            "publish_miss"
                        },
                        job.index,
                        job.target_long_edge,
                        state.cache.len(),
                        state.cache_bytes,
                        state.options.cache_bytes,
                        cache_hit,
                    );
                }
                #[cfg(not(any(feature = "perf-dev", feature = "perf-diagnostics")))]
                let _ = cache_hit;
            }
            state.finish_schedule();
            break;
        }
    }
}

enum Publish {
    Sent,
    Restart,
    Stopped,
}

fn publish(
    state: &mut WorkerSession,
    sender: &EventSender,
    event: WorkerEvent,
    commands: &Receiver<WorkerCommand>,
    ctx: &Context,
) -> Publish {
    match sender.send(event, state.options.cache_bytes, commands) {
        DeliveryOutcome::Sent => {
            ctx.request_repaint();
            Publish::Sent
        }
        DeliveryOutcome::Interrupted(command, event) => {
            // A completed oversize page may not be in the worker LRU. Preserve
            // it across compatible requests while waiting for UI admission.
            if let WorkerEvent::PageReady {
                page_id,
                decode,
                page,
                ..
            } = event
            {
                state.keep_page(page_id, decode, page);
            }
            if state.apply(command) {
                Publish::Restart
            } else {
                Publish::Stopped
            }
        }
        DeliveryOutcome::Disconnected => Publish::Stopped,
    }
}

fn start_overlap(
    state: &mut WorkerSession,
    source: &SharedSource,
    jobs: &[PageJob],
    start: usize,
    current: PageJob,
    commands: &Receiver<WorkerCommand>,
) {
    // A cancelled OS read still occupies its lane until it actually finishes.
    // Keep both overlap strategies from starting duplicate work for that lane.
    // Reuse saved work before speculating: the overlap selector cannot claim
    // another read/decode of a page already waiting in a completed slot.
    if state.has_completed_work()
        || read_ahead::has_pending(&mut state.read_ahead)
        || decode_ahead::has_pending(&mut state.decode_ahead)
    {
        return;
    }
    if let Some(candidate) = state.decode_ahead_policy.candidate() {
        if decode_ahead::maybe_start_decode(
            &mut state.decode_ahead,
            commands,
            source,
            source.book_id(),
            state.book_epoch,
            jobs,
            start,
            state.center,
            state.visible_pages,
            &state.options,
            &state.cache,
            &state.published_app_cache_hints,
            candidate,
            state
                .decode_ahead_policy
                .needs_prepare_timing_for(source, current.index),
        ) {
            return;
        }
    }
    read_ahead::maybe_start(
        &mut state.read_ahead,
        commands,
        source,
        source.book_id(),
        state.book_epoch,
        jobs,
        start,
        state.center,
        state.visible_pages,
        &state.options,
        &state.cache,
        &state.published_app_cache_hints,
    );
}
/// Collapse everything queued into the one command with the same net effect.
///
/// Only *page parameters* may be superseded. A `LoadBook`'s source and a
/// `ClearBook`'s acknowledgement have to survive the collapse: keeping just the
/// newest message drops the book install whenever a `SetPage` lands behind it —
/// which the app does within the same frame it opens a book (`install_source`
/// sends `load_book`, then the central panel's target/recenter pass sends
/// `set_page`). The worker would keep serving the previous book, every
/// `PageReady` would carry the stale `book_id` the app rejects, and no later
/// command could repair it because only `LoadBook` installs a source.
fn drain_latest_command(command_rx: &Receiver<WorkerCommand>) -> Option<WorkerCommand> {
    let mut folded = None;
    while let Ok(command) = command_rx.try_recv() {
        folded = Some(fold_command(folded, command));
    }
    folded
}

fn fold_command(previous: Option<WorkerCommand>, next: WorkerCommand) -> WorkerCommand {
    let Some(previous) = previous else {
        return next;
    };
    match (previous, next) {
        // Shutdown is terminal; nothing queued behind it can matter.
        (WorkerCommand::Shutdown, _) => WorkerCommand::Shutdown,
        // A superseded ClearBook still owes `clear_book_blocking` its ack, or the
        // caller reports a timeout for a release that did happen. Whatever
        // supersedes it either installs another source or shuts down, so the old
        // one is dropped either way.
        (WorkerCommand::ClearBook { ack }, next) => {
            let _ = ack.send(());
            next
        }
        // The newest page parameters win, but the book install rides through.
        (
            WorkerCommand::LoadBook { source, .. },
            WorkerCommand::SetPage {
                center,
                direction,
                target_long_edge,
                visible_pages,
                options,
            },
        ) => WorkerCommand::LoadBook {
            source,
            center,
            direction,
            target_long_edge,
            visible_pages,
            options,
        },
        (_, next) => next,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::source::{BookSource, SourceError};
    use crossbeam_channel::{bounded, unbounded};
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    struct StubSource {
        book_id: String,
        path: PathBuf,
    }

    impl BookSource for StubSource {
        fn title(&self) -> &str {
            &self.book_id
        }
        fn source_path(&self) -> &Path {
            &self.path
        }
        fn book_id(&self) -> &str {
            &self.book_id
        }
        fn page_count(&self) -> usize {
            1
        }
        fn page_name(&self, _index: usize) -> Option<&str> {
            Some("001.png")
        }
        fn read_page(&self, _index: usize) -> Result<Vec<u8>, SourceError> {
            Ok(Vec::new())
        }
    }

    fn stub_source(book_id: &str) -> SharedSource {
        Arc::new(StubSource {
            book_id: book_id.to_owned(),
            path: PathBuf::from(book_id),
        })
    }

    fn load_book(book_id: &str, center: usize) -> WorkerCommand {
        WorkerCommand::LoadBook {
            source: stub_source(book_id),
            center,
            direction: NavigationDirection::Forward,
            target_long_edge: 2048,
            visible_pages: 1,
            options: WorkerOptions::default(),
        }
    }

    fn set_page(center: usize) -> WorkerCommand {
        WorkerCommand::SetPage {
            center,
            direction: NavigationDirection::Forward,
            target_long_edge: 4096,
            visible_pages: 2,
            options: WorkerOptions::default(),
        }
    }

    #[test]
    fn backpressure_replans_do_not_displace_the_uncached_result() {
        use super::super::{DecodeBackend, DecodeOptions, PagePixels, PreparedPage};
        use crate::core::source::PageId;
        let make_page = |height| {
            Arc::new(PreparedPage {
                pixels: PagePixels::Rgba(vec![0; 2048 * height * 4].into()),
                original_width: 2048,
                original_height: height,
                display_width: 2048,
                display_height: height,
                byte_size: 2048 * height * 4,
                target_long_edge: 2048,
                decode_backend: DecodeBackend::ImageCrate,
                notice: None,
            })
        };
        let cached = make_page(1024);
        let uncached = make_page(2048);
        let options = WorkerOptions {
            cache_bytes: cached.byte_size,
            ..WorkerOptions::default()
        };
        let mut state = WorkerSession::default();
        let source = stub_source("book");
        state.apply(WorkerCommand::LoadBook {
            source: source.clone(),
            center: 0,
            direction: NavigationDirection::Forward,
            target_long_edge: 2048,
            visible_pages: 1,
            options: options.clone(),
        });
        let key = page_cache_key(
            "book",
            source.source_cache_id(),
            PageId(0),
            2048,
            options.decode,
        );
        assert!(insert_worker_cache_with_budget(
            &mut state.cache,
            &mut state.cache_bytes,
            key,
            &cached,
            options.cache_bytes
        ));
        assert!(uncached.byte_size > options.cache_bytes);
        let event = |id, page: Arc<PreparedPage>| WorkerEvent::PageReady {
            book_id: "book".to_owned(),
            source_instance_id: source.source_instance_id(),
            page_id: PageId(id),
            decode: DecodeOptions::default(),
            page,
        };
        let (sender, _receiver) = super::super::delivery::event_channel();
        let (tx, rx) = unbounded();
        assert!(matches!(
            sender.send(event(0, cached.clone()), options.cache_bytes, &rx),
            DeliveryOutcome::Sent
        ));
        let ctx = Context::default();
        for (id, page) in [(1, uncached.clone()), (0, cached.clone()), (0, cached)] {
            tx.send(WorkerCommand::SetPage {
                center: 0,
                direction: NavigationDirection::Forward,
                target_long_edge: 2048,
                visible_pages: 1,
                options: options.clone(),
            })
            .unwrap();
            assert!(matches!(
                publish(&mut state, &sender, event(id, page), &rx, &ctx),
                Publish::Restart
            ));
        }
        assert!(Arc::ptr_eq(
            &state.take_page(PageId(1), 2048).unwrap(),
            &uncached
        ));
    }

    #[test]
    fn delivery_interruption_retains_pixels_before_budget_reduction_prunes_them() {
        use super::super::{DecodeBackend, DecodeOptions, PagePixels, PreparedPage};
        use crate::core::source::PageId;
        let page = Arc::new(PreparedPage {
            pixels: PagePixels::Rgba(vec![0; 12 * 1024 * 1024].into()),
            original_width: 2048,
            original_height: 1536,
            display_width: 2048,
            display_height: 1536,
            byte_size: 12 * 1024 * 1024,
            target_long_edge: 2048,
            decode_backend: DecodeBackend::ImageCrate,
            notice: None,
        });
        let mut state = WorkerSession::default();
        state.apply(load_book("book", 0));
        state.options.cache_bytes = 16 * 1024 * 1024;
        let source = state.source.clone().unwrap();
        let key = page_cache_key(
            "book",
            source.source_cache_id(),
            PageId(0),
            2048,
            state.options.decode,
        );
        assert!(insert_worker_cache_with_budget(
            &mut state.cache,
            &mut state.cache_bytes,
            key,
            &page,
            state.options.cache_bytes
        ));
        let event = || WorkerEvent::PageReady {
            book_id: "book".to_owned(),
            source_instance_id: source.source_instance_id(),
            page_id: PageId(0),
            decode: DecodeOptions::default(),
            page: page.clone(),
        };
        let (sender, _receiver) = super::super::delivery::event_channel();
        let (tx, rx) = unbounded();
        assert!(matches!(
            sender.send(event(), state.options.cache_bytes, &rx),
            DeliveryOutcome::Sent
        ));
        let options = WorkerOptions {
            cache_bytes: 8 * 1024 * 1024,
            ..WorkerOptions::default()
        };
        tx.send(WorkerCommand::SetPage {
            center: 0,
            direction: NavigationDirection::Forward,
            target_long_edge: 2048,
            visible_pages: 1,
            options,
        })
        .unwrap();
        assert!(matches!(
            publish(&mut state, &sender, event(), &rx, &Context::default()),
            Publish::Restart
        ));
        assert!(state.cache.is_empty());
        assert!(Arc::ptr_eq(
            &state.take_page(PageId(0), 2048).unwrap(),
            &page
        ));
    }

    #[test]
    fn drain_keeps_the_book_install_when_a_set_page_lands_behind_it() {
        let (tx, rx) = unbounded();
        tx.send(load_book("book-b", 0)).unwrap();
        tx.send(set_page(7)).unwrap();

        let folded = drain_latest_command(&rx).expect("a command");

        match folded {
            WorkerCommand::LoadBook {
                source,
                center,
                target_long_edge,
                visible_pages,
                ..
            } => {
                assert_eq!(source.book_id(), "book-b");
                // The newest page parameters still win.
                assert_eq!(center, 7);
                assert_eq!(target_long_edge, 4096);
                assert_eq!(visible_pages, 2);
            }
            _ => panic!("book install was dropped by the drain"),
        }
    }

    #[test]
    fn drain_keeps_the_newest_book_when_two_loads_are_queued() {
        let (tx, rx) = unbounded();
        tx.send(load_book("book-a", 0)).unwrap();
        tx.send(load_book("book-b", 3)).unwrap();

        match drain_latest_command(&rx).expect("a command") {
            WorkerCommand::LoadBook { source, center, .. } => {
                assert_eq!(source.book_id(), "book-b");
                assert_eq!(center, 3);
            }
            _ => panic!("newest book install was dropped by the drain"),
        }
    }

    #[test]
    fn drain_acknowledges_a_superseded_clear_book() {
        let (tx, rx) = unbounded();
        let (ack, done) = bounded(1);
        tx.send(WorkerCommand::ClearBook { ack }).unwrap();
        tx.send(load_book("book-b", 0)).unwrap();

        let folded = drain_latest_command(&rx).expect("a command");

        assert!(
            done.try_recv().is_ok(),
            "clear_book_blocking would report a timeout for a release that happened"
        );
        assert!(matches!(folded, WorkerCommand::LoadBook { .. }));
    }

    #[test]
    fn drain_never_supersedes_shutdown() {
        let (tx, rx) = unbounded();
        tx.send(WorkerCommand::Shutdown).unwrap();
        tx.send(set_page(2)).unwrap();

        assert!(matches!(
            drain_latest_command(&rx).expect("a command"),
            WorkerCommand::Shutdown
        ));
    }
}
