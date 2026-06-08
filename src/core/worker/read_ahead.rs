use super::cache::{page_cache_key, should_skip_published_app_cache_hint, PublishedAppCacheHints};
use super::scheduler::{is_visible_page_index, should_skip_ai_preview_or_prefetch, PageJob};
use super::{PreparedPage, WorkerCommand, WorkerOptions};
use crate::core::source::{PageReadHint, SharedSource};
#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
use crate::core::{perf_trace, perf_trace::PerfField};
use crossbeam_channel::Receiver;
use lru::LruCache;
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const READ_AHEAD_STACK_BYTES: usize = 256 * 1024;

pub(super) struct ReadAhead {
    book_id: String,
    book_epoch: usize,
    index: usize,
    handle: Option<JoinHandle<ReadAheadResult>>,
}

struct ReadAheadResult {
    result: Result<Vec<u8>, String>,
}

impl ReadAhead {
    pub(super) fn start(
        source: SharedSource,
        book_id: String,
        book_epoch: usize,
        index: usize,
    ) -> Self {
        let handle = thread::Builder::new()
            .name("suisuiview-page-read-ahead".to_owned())
            .stack_size(READ_AHEAD_STACK_BYTES)
            .spawn(move || {
                let read_hint = source.page_read_hint(index);
                let started = Instant::now();
                let result = source.read_page(index).map_err(|error| error.to_string());
                record_page_read(
                    index,
                    book_epoch,
                    true,
                    false,
                    started.elapsed(),
                    result.is_ok(),
                    read_hint,
                );
                ReadAheadResult { result }
            })
            .expect("page read-ahead thread should start");

        Self {
            book_id,
            book_epoch,
            index,
            handle: Some(handle),
        }
    }

    pub(super) fn matches(&self, book_id: &str, book_epoch: usize, index: usize) -> bool {
        self.book_id == book_id && self.book_epoch == book_epoch && self.index == index
    }

    pub(super) fn finish(mut self, reason: &'static str) -> Result<Vec<u8>, String> {
        self.join(reason)
    }

    pub(super) fn detach(mut self, reason: &'static str) {
        if self.handle.take().is_some() {
            record_detach(self.index, self.book_epoch, reason);
        }
    }

    fn join(&mut self, reason: &'static str) -> Result<Vec<u8>, String> {
        let Some(handle) = self.handle.take() else {
            return Err("Page read-ahead thread was unavailable".to_owned());
        };
        let started = Instant::now();
        let joined = handle
            .join()
            .map(|output| output.result)
            .unwrap_or_else(|_| Err("Page read-ahead thread panicked".to_owned()));
        record_join_wait(
            self.index,
            self.book_epoch,
            reason,
            started.elapsed(),
            joined.is_ok(),
        );
        joined
    }
}

impl Drop for ReadAhead {
    fn drop(&mut self) {
        if self.handle.take().is_some() {
            record_detach(self.index, self.book_epoch, "drop");
        }
    }
}

pub(super) fn maybe_start(
    read_ahead: &mut Option<ReadAhead>,
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
) {
    if read_ahead.is_some() {
        return;
    }
    if !command_rx.is_empty() {
        return;
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
        return;
    };

    *read_ahead = Some(ReadAhead::start(
        source.clone(),
        book_id.to_owned(),
        book_epoch,
        job.index,
    ));
}

pub(super) fn next_job(
    source: &SharedSource,
    book_id: &str,
    jobs: &[PageJob],
    start_position: usize,
    center: usize,
    visible_pages: usize,
    options: &WorkerOptions,
    cache: &LruCache<String, Arc<PreparedPage>>,
    published_app_cache_hints: &PublishedAppCacheHints,
) -> Option<PageJob> {
    jobs.iter().copied().skip(start_position).find(|job| {
        if should_skip_ai_preview_or_prefetch(
            source.page_name(job.index),
            center,
            visible_pages,
            job.index,
            job.target_long_edge,
        ) {
            return false;
        }
        let key = page_cache_key(book_id, job.index, job.target_long_edge, options.decode);
        if cache.peek(&key).is_some() {
            return false;
        }
        if options.app_cache_covers(job.index, job.target_long_edge) {
            return false;
        }
        !should_skip_published_app_cache_hint(
            published_app_cache_hints,
            is_visible_page_index(job.index, center, visible_pages),
            job.index,
            job.target_long_edge,
            options.decode,
        )
    })
}

pub(super) fn consume_matching(
    pending: &mut Option<ReadAhead>,
    book_id: &str,
    book_epoch: usize,
    index: usize,
) -> Option<Result<Vec<u8>, String>> {
    if pending
        .as_ref()
        .is_some_and(|read| read.matches(book_id, book_epoch, index))
    {
        return pending.take().map(|read| read.finish("consume"));
    }

    clear_pending(pending, "stale");
    None
}

pub(super) fn clear_matching(
    pending: &mut Option<ReadAhead>,
    book_id: &str,
    book_epoch: usize,
    index: usize,
    reason: &'static str,
) {
    if pending
        .as_ref()
        .is_some_and(|read| read.matches(book_id, book_epoch, index))
    {
        clear_pending(pending, reason);
    }
}

pub(super) fn clear_pending(pending: &mut Option<ReadAhead>, reason: &'static str) {
    if let Some(read) = pending.take() {
        read.detach(reason);
    }
}

#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
pub(super) fn record_page_read(
    index: usize,
    book_epoch: usize,
    read_ahead: bool,
    decode_ahead: bool,
    duration: Duration,
    success: bool,
    hint: Option<PageReadHint>,
) {
    let hint = hint.unwrap_or_else(PageReadHint::unknown);
    perf_trace::record_duration_if_at_least(
        "page_read",
        duration,
        Duration::from_millis(25),
        &[
            PerfField::Usize("page", index),
            PerfField::Usize("book_epoch", book_epoch),
            PerfField::Bool("success", success),
            PerfField::Bool("read_ahead", read_ahead),
            PerfField::Bool("decode_ahead", decode_ahead),
            PerfField::Str("source_kind", hint.source_kind.as_str()),
            PerfField::Str("compression_method", hint.compression_method.as_str()),
            PerfField::Str("compression_state", hint.compression_state()),
            PerfField::Usize("compressed_size", size_hint_to_usize(hint.compressed_size)),
            PerfField::Usize(
                "uncompressed_size",
                size_hint_to_usize(hint.uncompressed_size),
            ),
            PerfField::Usize("compression_ratio_milli", hint.compression_ratio_milli()),
        ],
    );
}

#[cfg(not(any(feature = "perf-dev", feature = "perf-diagnostics")))]
pub(super) fn record_page_read(
    _index: usize,
    _book_epoch: usize,
    _read_ahead: bool,
    _decode_ahead: bool,
    _duration: Duration,
    _success: bool,
    _hint: Option<PageReadHint>,
) {
}

#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
fn size_hint_to_usize(size: Option<u64>) -> usize {
    size.and_then(|size| usize::try_from(size).ok())
        .unwrap_or_default()
}

#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
fn record_join_wait(
    index: usize,
    book_epoch: usize,
    reason: &'static str,
    duration: Duration,
    success: bool,
) {
    perf_trace::record_duration_if_at_least(
        "page_read_ahead_join_wait",
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
fn record_detach(index: usize, book_epoch: usize, reason: &'static str) {
    perf_trace::record_duration(
        "page_read_ahead_detach",
        Duration::ZERO,
        &[
            PerfField::Usize("page", index),
            PerfField::Usize("book_epoch", book_epoch),
            PerfField::Str("reason", reason),
        ],
    );
}

#[cfg(not(any(feature = "perf-dev", feature = "perf-diagnostics")))]
fn record_detach(_index: usize, _book_epoch: usize, _reason: &'static str) {}

#[cfg(not(any(feature = "perf-dev", feature = "perf-diagnostics")))]
fn record_join_wait(
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
        clear_pending, consume_matching, maybe_start, PageJob, PublishedAppCacheHints, ReadAhead,
        WorkerCommand, WorkerOptions,
    };
    use crate::core::source::{BookSource, SharedSource, SourceError};
    use crossbeam_channel::unbounded;
    use lru::LruCache;
    use std::num::NonZeroUsize;
    use std::path::{Path, PathBuf};
    use std::sync::{mpsc, Arc, Mutex};
    use std::thread;
    use std::time::{Duration, Instant};

    #[test]
    fn read_ahead_matches_book_epoch_and_page() {
        let source: SharedSource = Arc::new(StaticSource {
            path: PathBuf::from("static-source"),
            bytes: vec![1, 2, 3, 4],
        });
        let read = ReadAhead::start(source, "book".to_owned(), 7, 1);

        assert!(read.matches("book", 7, 1));
        assert!(!read.matches("book", 8, 1));
        assert!(!read.matches("other", 7, 1));
        assert!(!read.matches("book", 7, 2));
        assert_eq!(read.finish("test").unwrap(), vec![1, 2, 3, 4]);
    }

    #[test]
    fn maybe_start_reads_next_job_on_read_ahead_thread() {
        let (_command_tx, command_rx) = unbounded::<WorkerCommand>();
        let read_log = Arc::new(Mutex::new(Vec::new()));
        let source: SharedSource = Arc::new(ThreadRecordingSource {
            path: PathBuf::from("thread-recording-source"),
            bytes: vec![1, 2, 3, 4],
            read_log: read_log.clone(),
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

        maybe_start(
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
        );

        assert_eq!(
            consume_matching(&mut pending, "book", 7, 1)
                .unwrap()
                .unwrap(),
            vec![1, 2, 3, 4]
        );
        let read_log = read_log.lock().unwrap();
        assert!(read_log.iter().any(|(index, thread_name)| {
            *index == 1 && thread_name.as_deref() == Some("suisuiview-page-read-ahead")
        }));
    }

    #[test]
    fn clear_pending_detaches_without_waiting_for_slow_read() {
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
        let mut pending = Some(ReadAhead::start(source, "book".to_owned(), 7, 1));
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();

        let started = Instant::now();
        clear_pending(&mut pending, "test");
        assert!(started.elapsed() < Duration::from_millis(100));

        release_tx.send(()).unwrap();
        done_rx.recv_timeout(Duration::from_secs(2)).unwrap();
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

    struct ThreadRecordingSource {
        path: PathBuf,
        bytes: Vec<u8>,
        read_log: Arc<Mutex<Vec<(usize, Option<String>)>>>,
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
}
