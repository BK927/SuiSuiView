use super::cache::PublishedAppCacheHints;
use super::decode_policy::DecodeAheadCandidate;
use super::prepare::{prepare_page_with_perf, PreparedPageWithTiming};
use super::read_ahead::{next_job, record_page_read};
use super::scheduler::PageJob;
use super::{DecodeOptions, PreparedPage, WorkerCommand, WorkerOptions};
use crate::core::source::{PageId, SharedSource};
#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
use crate::core::{perf_trace, perf_trace::PerfField};
use crossbeam_channel::Receiver;
use lru::LruCache;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const DECODE_AHEAD_STACK_BYTES: usize = 1024 * 1024;
const DECODE_AHEAD_CANCELLED: &str = "Page decode-ahead was cancelled";

pub(super) struct DecodeAhead {
    book_id: String,
    book_epoch: usize,
    page_id: PageId,
    index: usize,
    target_long_edge: u32,
    decode: DecodeOptions,
    cancel: Arc<AtomicBool>,
    cancel_recorded: bool,
    handle: Option<JoinHandle<DecodeAheadResult>>,
}

struct DecodeAheadResult {
    result: Result<PreparedPageWithTiming, String>,
}

impl DecodeAhead {
    fn start(
        source: SharedSource,
        book_id: String,
        book_epoch: usize,
        page_id: PageId,
        job: PageJob,
        decode: DecodeOptions,
        measure_prepare_timing: bool,
    ) -> Self {
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_for_thread = cancel.clone();
        let handle = thread::Builder::new()
            .name("suisuiview-page-decode-ahead".to_owned())
            .stack_size(DECODE_AHEAD_STACK_BYTES)
            .spawn(move || {
                let read_hint = source.page_read_hint(job.index);
                let read_started = Instant::now();
                let result = source
                    .read_page(job.index)
                    .map_err(|error| error.to_string());
                record_page_read(
                    job.index,
                    book_epoch,
                    true,
                    true,
                    read_started.elapsed(),
                    result.is_ok(),
                    read_hint,
                );
                if cancel_for_thread.load(Ordering::Acquire) {
                    return DecodeAheadResult {
                        result: Err(DECODE_AHEAD_CANCELLED.to_owned()),
                    };
                }
                let result = result.and_then(|bytes| {
                    prepare_page_with_perf(
                        &bytes,
                        job,
                        book_epoch,
                        decode,
                        true,
                        measure_prepare_timing,
                    )
                });
                let result = if cancel_for_thread.load(Ordering::Acquire) {
                    Err(DECODE_AHEAD_CANCELLED.to_owned())
                } else {
                    result
                };
                DecodeAheadResult { result }
            })
            .expect("page decode-ahead thread should start");

        Self {
            book_id,
            book_epoch,
            page_id,
            index: job.index,
            target_long_edge: job.target_long_edge,
            decode,
            cancel,
            cancel_recorded: false,
            handle: Some(handle),
        }
    }

    fn matches(
        &self,
        book_id: &str,
        book_epoch: usize,
        page_id: PageId,
        target_long_edge: u32,
        decode: DecodeOptions,
    ) -> bool {
        !self.is_cancelled()
            && self.book_id == book_id
            && self.book_epoch == book_epoch
            && self.page_id == page_id
            && self.target_long_edge == target_long_edge
            && self.decode == decode
    }

    fn finish(mut self, reason: &'static str) -> Result<PreparedPageWithTiming, String> {
        self.join(reason)
    }

    fn detach(mut self, reason: &'static str) {
        self.cancel(reason);
        let _ = self.handle.take();
    }

    fn cancel(&mut self, reason: &'static str) {
        self.cancel.store(true, Ordering::Release);
        if !self.cancel_recorded {
            record_decode_detach(self.index, self.book_epoch, reason);
            self.cancel_recorded = true;
        }
    }

    fn is_finished(&self) -> bool {
        self.handle
            .as_ref()
            .is_some_and(std::thread::JoinHandle::is_finished)
    }

    fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Acquire)
    }

    fn matches_job(&self, page_id: PageId, target_long_edge: u32) -> bool {
        self.page_id == page_id && self.target_long_edge == target_long_edge
    }

    fn join(&mut self, reason: &'static str) -> Result<PreparedPageWithTiming, String> {
        let Some(handle) = self.handle.take() else {
            return Err("Page decode-ahead thread was unavailable".to_owned());
        };
        let started = Instant::now();
        let joined = handle
            .join()
            .map(|output| output.result)
            .unwrap_or_else(|_| Err("Page decode-ahead thread panicked".to_owned()));
        record_decode_join_wait(
            self.index,
            self.book_epoch,
            reason,
            started.elapsed(),
            joined.is_ok(),
        );
        joined
    }
}

impl Drop for DecodeAhead {
    fn drop(&mut self) {
        if self.handle.is_some() {
            self.cancel("drop");
            let _ = self.handle.take();
        }
    }
}

#[allow(clippy::too_many_arguments)] // established call surface; a params struct would be pure boilerplate
pub(super) fn maybe_start_decode(
    decode_ahead: &mut Option<DecodeAhead>,
    command_rx: &Receiver<WorkerCommand>,
    source: &SharedSource,
    book_id: &str,
    book_epoch: usize,
    jobs: &[PageJob],
    start_position: usize,
    center: usize,
    visible_pages: usize,
    options: &WorkerOptions,
    cache: &LruCache<String, Arc<PreparedPage>>,
    published_app_cache_hints: &PublishedAppCacheHints,
    candidate: DecodeAheadCandidate,
    measure_prepare_timing: bool,
) -> bool {
    discard_finished_decode(decode_ahead, "discard_finished");
    if !source.supports_concurrent_page_reads() {
        clear_pending_decode(decode_ahead, "serialized_source");
        return false;
    }
    if decode_ahead.is_some() {
        return true;
    }
    if !command_rx.is_empty() {
        return true;
    }

    let Some(job) = next_job(
        source,
        book_id,
        jobs,
        start_position,
        center,
        visible_pages,
        options,
        cache,
        published_app_cache_hints,
    ) else {
        return false;
    };
    if !candidate.matches_job(source, job.index) {
        return false;
    }
    let Some(page_id) = source.page_id(job.index) else {
        return false;
    };

    *decode_ahead = Some(DecodeAhead::start(
        source.clone(),
        book_id.to_owned(),
        book_epoch,
        page_id,
        job,
        options.decode,
        measure_prepare_timing,
    ));
    true
}

pub(super) fn consume_matching_decode(
    pending: &mut Option<DecodeAhead>,
    book_id: &str,
    book_epoch: usize,
    page_id: PageId,
    target_long_edge: u32,
    decode: DecodeOptions,
) -> Option<Result<PreparedPageWithTiming, String>> {
    if pending.as_ref().is_some_and(|decode_ahead| {
        decode_ahead.matches(book_id, book_epoch, page_id, target_long_edge, decode)
    }) {
        return pending
            .take()
            .map(|decode_ahead| decode_ahead.finish("consume"));
    }

    cancel_pending_decode(pending, "stale");
    None
}

pub(super) fn clear_pending_decode(pending: &mut Option<DecodeAhead>, reason: &'static str) {
    if let Some(decode_ahead) = pending.take() {
        decode_ahead.detach(reason);
    }
}

pub(super) fn cancel_pending_decode(pending: &mut Option<DecodeAhead>, reason: &'static str) {
    if let Some(decode_ahead) = pending.as_mut() {
        decode_ahead.cancel(reason);
    }
    discard_finished_decode(pending, "discard_cancelled");
}

pub(super) fn clear_pending_decode_if_context_changed(
    pending: &mut Option<DecodeAhead>,
    book_id: Option<&str>,
    book_epoch: usize,
    target_long_edge: u32,
    decode: DecodeOptions,
) {
    if pending.as_ref().is_some_and(|decode_ahead| {
        Some(decode_ahead.book_id.as_str()) == book_id
            && decode_ahead.book_epoch == book_epoch
            && decode_ahead.target_long_edge == target_long_edge
            && decode_ahead.decode == decode
    }) {
        return;
    }

    cancel_pending_decode(pending, "context");
}

#[allow(clippy::too_many_arguments)]
pub(super) fn cancel_pending_decode_if_not_scheduled(
    pending: &mut Option<DecodeAhead>,
    source: &SharedSource,
    book_id: &str,
    book_epoch: usize,
    jobs: &[PageJob],
    center: usize,
    visible_pages: usize,
    options: &WorkerOptions,
    cache: &LruCache<String, Arc<PreparedPage>>,
    published_app_cache_hints: &PublishedAppCacheHints,
) {
    if pending.as_ref().is_some_and(|decode_ahead| {
        if decode_ahead.is_cancelled()
            || decode_ahead.book_id != book_id
            || decode_ahead.book_epoch != book_epoch
            || decode_ahead.decode != options.decode
        {
            return false;
        }

        jobs.iter()
            .position(|job| {
                source
                    .page_id(job.index)
                    .is_some_and(|page_id| decode_ahead.matches_job(page_id, job.target_long_edge))
            })
            .and_then(|position| {
                next_job(
                    source,
                    book_id,
                    jobs,
                    position,
                    center,
                    visible_pages,
                    options,
                    cache,
                    published_app_cache_hints,
                )
            })
            .is_some_and(|job| {
                source
                    .page_id(job.index)
                    .is_some_and(|page_id| decode_ahead.matches_job(page_id, job.target_long_edge))
            })
    }) {
        return;
    }

    cancel_pending_decode(pending, "unscheduled");
}

fn discard_finished_decode(pending: &mut Option<DecodeAhead>, reason: &'static str) {
    if !pending.as_ref().is_some_and(DecodeAhead::is_finished) {
        return;
    }
    let Some(decode_ahead) = pending.take() else {
        return;
    };
    let _ = decode_ahead.finish(reason);
}

#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
fn record_decode_join_wait(
    index: usize,
    book_epoch: usize,
    reason: &'static str,
    duration: Duration,
    success: bool,
) {
    perf_trace::record_duration_if_at_least(
        "page_decode_ahead_join_wait",
        duration,
        Duration::from_millis(5),
        &[
            PerfField::Usize("page", index),
            PerfField::Usize("book_epoch", book_epoch),
            PerfField::Str("reason", reason),
            PerfField::Bool("success", success),
        ],
    );
}

#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
fn record_decode_detach(index: usize, book_epoch: usize, reason: &'static str) {
    perf_trace::record_duration(
        "page_decode_ahead_detach",
        Duration::ZERO,
        &[
            PerfField::Usize("page", index),
            PerfField::Usize("book_epoch", book_epoch),
            PerfField::Str("reason", reason),
        ],
    );
}

#[cfg(not(any(feature = "perf-dev", feature = "perf-diagnostics")))]
fn record_decode_detach(_index: usize, _book_epoch: usize, _reason: &'static str) {}

#[cfg(not(any(feature = "perf-dev", feature = "perf-diagnostics")))]
fn record_decode_join_wait(
    _index: usize,
    _book_epoch: usize,
    _reason: &'static str,
    _duration: Duration,
    _success: bool,
) {
}

#[cfg(test)]
mod tests {
    use super::{
        cancel_pending_decode, cancel_pending_decode_if_not_scheduled, clear_pending_decode,
        clear_pending_decode_if_context_changed, consume_matching_decode, maybe_start_decode,
        DecodeAhead,
    };
    use crate::core::source::{BookSource, PageId, SharedSource, SourceError};
    use crate::core::worker::cache::PublishedAppCacheHints;
    use crate::core::worker::decode_policy::DecodeAheadCandidate;
    use crate::core::worker::scheduler::PageJob;
    use crate::core::worker::{CachedPageKey, DecodeOptions, WorkerCommand, WorkerOptions};
    use crossbeam_channel::unbounded;
    use lru::LruCache;
    use std::num::NonZeroUsize;
    use std::path::{Path, PathBuf};
    use std::sync::{mpsc, Arc, Mutex};
    use std::thread;
    use std::time::{Duration, Instant};

    #[test]
    fn maybe_start_decode_reads_on_decode_ahead_thread() {
        let (_command_tx, command_rx) = unbounded::<WorkerCommand>();
        let read_log = Arc::new(Mutex::new(Vec::new()));
        let source: SharedSource = Arc::new(ThreadRecordingSource {
            path: PathBuf::from("thread-recording-source"),
            bytes: vec![1, 2, 3, 4],
            read_log: read_log.clone(),
            supports_concurrent_reads: true,
        });
        let options = WorkerOptions {
            progressive_preview_enabled: false,
            ..WorkerOptions::default()
        };
        let cache = LruCache::new(NonZeroUsize::new(4).unwrap());
        let hints = PublishedAppCacheHints::new();
        let jobs = [
            PageJob {
                index: 0,
                target_long_edge: 2048,
            },
            PageJob {
                index: 1,
                target_long_edge: 2048,
            },
        ];
        let mut pending = None;

        maybe_start_decode(
            &mut pending,
            &command_rx,
            &source,
            "book",
            7,
            &jobs,
            1,
            0,
            1,
            &options,
            &cache,
            &hints,
            DecodeAheadCandidate::Any,
            false,
        );

        assert!(
            consume_matching_decode(&mut pending, "book", 7, PageId(1), 2048, options.decode)
                .unwrap()
                .is_err()
        );
        let read_log = read_log.lock().unwrap();
        assert!(read_log.iter().any(|(index, thread_name)| {
            *index == 1 && thread_name.as_deref() == Some("suisuiview-page-decode-ahead")
        }));
    }

    #[test]
    fn serialized_source_does_not_start_speculative_decode() {
        let (_command_tx, command_rx) = unbounded::<WorkerCommand>();
        let read_log = Arc::new(Mutex::new(Vec::new()));
        let source: SharedSource = Arc::new(ThreadRecordingSource {
            path: PathBuf::from("serialized-source"),
            bytes: vec![1, 2, 3, 4],
            read_log: read_log.clone(),
            supports_concurrent_reads: false,
        });
        let options = WorkerOptions::default();
        let cache = LruCache::new(NonZeroUsize::new(4).unwrap());
        let hints = PublishedAppCacheHints::new();
        let jobs = [PageJob {
            index: 1,
            target_long_edge: 2048,
        }];
        let mut pending = None;

        let reserved = maybe_start_decode(
            &mut pending,
            &command_rx,
            &source,
            "book",
            7,
            &jobs,
            0,
            0,
            1,
            &options,
            &cache,
            &hints,
            DecodeAheadCandidate::Any,
            false,
        );

        assert!(!reserved);
        assert!(pending.is_none());
        assert!(read_log.lock().unwrap().is_empty());
    }

    #[test]
    fn adaptive_candidate_does_not_skip_nearer_non_matching_job() {
        let (_command_tx, command_rx) = unbounded::<WorkerCommand>();
        let source: SharedSource = Arc::new(NamedSource {
            path: PathBuf::from("named-source"),
            names: vec!["page-0000.jpg", "page-0001.webp", "page-0002.webp"],
            bytes: vec![1, 2, 3, 4],
        });
        let options = WorkerOptions {
            progressive_preview_enabled: false,
            ..WorkerOptions::default()
        };
        let cache = LruCache::new(NonZeroUsize::new(4).unwrap());
        let hints = PublishedAppCacheHints::new();
        let jobs = [
            PageJob {
                index: 0,
                target_long_edge: 2048,
            },
            PageJob {
                index: 1,
                target_long_edge: 2048,
            },
            PageJob {
                index: 2,
                target_long_edge: 2048,
            },
        ];
        let mut pending = None;

        let reserved = maybe_start_decode(
            &mut pending,
            &command_rx,
            &source,
            "book",
            7,
            &jobs,
            0,
            0,
            1,
            &options,
            &cache,
            &hints,
            DecodeAheadCandidate::WebpCluster,
            false,
        );

        assert!(!reserved);
        assert!(pending.is_none());
    }

    #[test]
    fn decode_ahead_consume_matches_page_id_not_index_and_rejects_post_swap_epoch() {
        // A decode spawned pre-swap carries page_id 5 while sitting at index 0.
        // consume_matching_decode keys on page_id (not index), and a post-swap
        // request (bumped epoch) no longer matches even at the same page_id.
        let decode = DecodeOptions::default();
        let spawn = || {
            let source: SharedSource = Arc::new(StaticSource {
                path: PathBuf::from("static-source"),
                bytes: vec![1, 2, 3, 4],
            });
            Some(DecodeAhead::start(
                source,
                "book".to_owned(),
                7,
                PageId(5),
                PageJob {
                    index: 0,
                    target_long_edge: 2048,
                },
                decode,
                false,
            ))
        };

        // Wrong page_id (the index value) does not match.
        let mut pending = spawn();
        assert!(
            consume_matching_decode(&mut pending, "book", 7, PageId(0), 2048, decode).is_none()
        );
        clear_pending_decode(&mut pending, "test");

        // Post-swap epoch (bumped) does not match the same page_id.
        let mut pending = spawn();
        assert!(
            consume_matching_decode(&mut pending, "book", 8, PageId(5), 2048, decode).is_none()
        );
        clear_pending_decode(&mut pending, "test");
    }

    #[test]
    fn clear_pending_decode_detaches_without_waiting_for_slow_decode_read() {
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let source: SharedSource = Arc::new(BlockingSource {
            path: PathBuf::from("blocking-source"),
            bytes: vec![1, 2, 3, 4],
            started_tx: Mutex::new(Some(started_tx)),
            release_rx: Mutex::new(release_rx),
            done_tx,
        });
        let mut pending = Some(DecodeAhead::start(
            source,
            "book".to_owned(),
            7,
            PageId(1),
            PageJob {
                index: 1,
                target_long_edge: 2048,
            },
            DecodeOptions::default(),
            false,
        ));
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();

        let started = Instant::now();
        clear_pending_decode(&mut pending, "test");
        assert!(started.elapsed() < Duration::from_millis(100));

        release_tx.send(()).unwrap();
        done_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    }

    #[test]
    fn clear_pending_decode_keeps_matching_context() {
        let source: SharedSource = Arc::new(StaticSource {
            path: PathBuf::from("static-source"),
            bytes: vec![1, 2, 3, 4],
        });
        let mut pending = Some(DecodeAhead::start(
            source,
            "book".to_owned(),
            7,
            PageId(1),
            PageJob {
                index: 1,
                target_long_edge: 2048,
            },
            DecodeOptions::default(),
            false,
        ));

        clear_pending_decode_if_context_changed(
            &mut pending,
            Some("book"),
            7,
            2048,
            DecodeOptions::default(),
        );

        assert!(pending.is_some());
        clear_pending_decode(&mut pending, "test");
    }

    #[test]
    fn cancelled_decode_is_not_consumed_as_page_result() {
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let source: SharedSource = Arc::new(BlockingSource {
            path: PathBuf::from("blocking-source"),
            bytes: vec![1, 2, 3, 4],
            started_tx: Mutex::new(Some(started_tx)),
            release_rx: Mutex::new(release_rx),
            done_tx,
        });
        let mut pending = Some(DecodeAhead::start(
            source,
            "book".to_owned(),
            7,
            PageId(1),
            PageJob {
                index: 1,
                target_long_edge: 2048,
            },
            DecodeOptions::default(),
            false,
        ));
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();

        cancel_pending_decode(&mut pending, "test");
        assert!(consume_matching_decode(
            &mut pending,
            "book",
            7,
            PageId(1),
            2048,
            DecodeOptions::default()
        )
        .is_none());

        release_tx.send(()).unwrap();
        done_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        clear_pending_decode(&mut pending, "test_cleanup");
    }

    #[test]
    fn cancel_pending_decode_if_not_scheduled_rechecks_app_cache() {
        let source: SharedSource = Arc::new(StaticSource {
            path: PathBuf::from("static-source"),
            bytes: vec![1, 2, 3, 4],
        });
        let mut pending = Some(DecodeAhead::start(
            source.clone(),
            "book".to_owned(),
            7,
            PageId(1),
            PageJob {
                index: 1,
                target_long_edge: 2048,
            },
            DecodeOptions::default(),
            false,
        ));
        let options = WorkerOptions {
            progressive_preview_enabled: false,
            app_cached_pages: vec![CachedPageKey::new(
                PageId(1),
                2048,
                DecodeOptions::default(),
            )],
            ..WorkerOptions::default()
        };
        let cache = LruCache::new(NonZeroUsize::new(4).unwrap());
        let hints = PublishedAppCacheHints::new();
        let jobs = [PageJob {
            index: 1,
            target_long_edge: 2048,
        }];

        cancel_pending_decode_if_not_scheduled(
            &mut pending,
            &source,
            "book",
            7,
            &jobs,
            1,
            1,
            &options,
            &cache,
            &hints,
        );

        assert!(pending.as_ref().is_none_or(DecodeAhead::is_cancelled));
        clear_pending_decode(&mut pending, "test");
    }

    struct StaticSource {
        path: PathBuf,
        bytes: Vec<u8>,
    }

    impl BookSource for StaticSource {
        fn title(&self) -> &str {
            "static"
        }

        fn source_path(&self) -> &Path {
            &self.path
        }

        fn book_id(&self) -> &str {
            "static-book"
        }

        fn page_count(&self) -> usize {
            2
        }

        fn page_name(&self, index: usize) -> Option<&str> {
            match index {
                0 => Some("page-0000.png"),
                1 => Some("page-0001.png"),
                _ => None,
            }
        }

        fn read_page(&self, index: usize) -> Result<Vec<u8>, SourceError> {
            if index < self.page_count() {
                Ok(self.bytes.clone())
            } else {
                Err(SourceError::InvalidPage {
                    index,
                    page_count: self.page_count(),
                })
            }
        }
    }

    struct BlockingSource {
        path: PathBuf,
        bytes: Vec<u8>,
        started_tx: Mutex<Option<mpsc::Sender<()>>>,
        release_rx: Mutex<mpsc::Receiver<()>>,
        done_tx: mpsc::Sender<()>,
    }

    impl BookSource for BlockingSource {
        fn title(&self) -> &str {
            "blocking"
        }

        fn source_path(&self) -> &Path {
            &self.path
        }

        fn book_id(&self) -> &str {
            "blocking-book"
        }

        fn page_count(&self) -> usize {
            2
        }

        fn page_name(&self, index: usize) -> Option<&str> {
            match index {
                0 => Some("page-0000.png"),
                1 => Some("page-0001.png"),
                _ => None,
            }
        }

        fn read_page(&self, index: usize) -> Result<Vec<u8>, SourceError> {
            if index >= self.page_count() {
                return Err(SourceError::InvalidPage {
                    index,
                    page_count: self.page_count(),
                });
            }

            if let Some(started_tx) = self.started_tx.lock().unwrap().take() {
                let _ = started_tx.send(());
            }
            self.release_rx
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(5))
                .unwrap();
            let _ = self.done_tx.send(());
            Ok(self.bytes.clone())
        }
    }

    type ReadLog = Arc<Mutex<Vec<(usize, Option<String>)>>>;
    struct ThreadRecordingSource {
        path: PathBuf,
        bytes: Vec<u8>,
        read_log: ReadLog,
        supports_concurrent_reads: bool,
    }

    struct NamedSource {
        path: PathBuf,
        names: Vec<&'static str>,
        bytes: Vec<u8>,
    }

    impl BookSource for ThreadRecordingSource {
        fn title(&self) -> &str {
            "thread-recording"
        }

        fn source_path(&self) -> &Path {
            &self.path
        }

        fn book_id(&self) -> &str {
            "thread-recording-book"
        }

        fn page_count(&self) -> usize {
            2
        }

        fn page_name(&self, index: usize) -> Option<&str> {
            match index {
                0 => Some("page-0000.png"),
                1 => Some("page-0001.png"),
                _ => None,
            }
        }

        fn supports_concurrent_page_reads(&self) -> bool {
            self.supports_concurrent_reads
        }

        fn read_page(&self, index: usize) -> Result<Vec<u8>, SourceError> {
            if index >= self.page_count() {
                return Err(SourceError::InvalidPage {
                    index,
                    page_count: self.page_count(),
                });
            }

            let thread_name = thread::current().name().map(str::to_owned);
            self.read_log.lock().unwrap().push((index, thread_name));
            Ok(self.bytes.clone())
        }
    }

    impl BookSource for NamedSource {
        fn title(&self) -> &str {
            "named"
        }

        fn source_path(&self) -> &Path {
            &self.path
        }

        fn book_id(&self) -> &str {
            "named-book"
        }

        fn page_count(&self) -> usize {
            self.names.len()
        }

        fn page_name(&self, index: usize) -> Option<&str> {
            self.names.get(index).copied()
        }

        fn read_page(&self, index: usize) -> Result<Vec<u8>, SourceError> {
            if index >= self.page_count() {
                return Err(SourceError::InvalidPage {
                    index,
                    page_count: self.page_count(),
                });
            }
            Ok(self.bytes.clone())
        }
    }
}
