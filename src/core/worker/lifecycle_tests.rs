use super::{
    CachedPageKey, CachedPageRef, NavigationDirection, PageWorker, WorkerEvent, WorkerOptions,
};
use crate::core::source::{BookSource, SharedSource, SourceError};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Default)]
struct Gate {
    released: Mutex<bool>,
    wake: Condvar,
}

impl Gate {
    fn release(&self) {
        *self.released.lock().unwrap() = true;
        self.wake.notify_all();
    }
    fn wait(&self) {
        let (released, _) = self
            .wake
            .wait_timeout_while(self.released.lock().unwrap(), TIMEOUT, |value| !*value)
            .unwrap();
        assert!(*released, "synthetic read was not released");
    }
}

struct FixtureSource {
    book: &'static str,
    instance: u64,
    encoded: Vec<u8>,
    reads: Vec<AtomicUsize>,
    gate: Option<(mpsc::Sender<()>, Arc<Gate>)>,
    concurrent_reads: bool,
}

impl FixtureSource {
    fn new(book: &'static str, count: usize, edge: usize) -> Self {
        let stride = (edge * 3 + 3) & !3;
        let mut encoded = vec![0; 54 + stride * edge];
        let length = encoded.len() as u32;
        for (offset, value) in [
            (2, length),
            (10, 54),
            (14, 40),
            (18, edge as u32),
            (22, edge as u32),
        ] {
            encoded[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        encoded[..2].copy_from_slice(b"BM");
        encoded[26..28].copy_from_slice(&1u16.to_le_bytes());
        encoded[28..30].copy_from_slice(&24u16.to_le_bytes());
        for row in encoded[54..].chunks_mut(stride) {
            for pixel in row[..edge * 3].chunks_mut(3) {
                pixel.copy_from_slice(&[17, 80, 160]);
            }
        }
        Self {
            book,
            instance: 1,
            encoded,
            reads: (0..count).map(|_| AtomicUsize::new(0)).collect(),
            gate: None,
            concurrent_reads: false,
        }
    }
    fn count(&self, page: usize) -> usize {
        self.reads[page].load(Ordering::Acquire)
    }
}

impl BookSource for FixtureSource {
    fn title(&self) -> &str {
        self.book
    }
    fn book_id(&self) -> &str {
        self.book
    }
    fn source_path(&self) -> &Path {
        Path::new("synthetic-memory-source")
    }
    fn source_instance_id(&self) -> u64 {
        self.instance
    }
    fn source_cache_id(&self) -> u64 {
        7
    }
    fn page_count(&self) -> usize {
        self.reads.len()
    }
    fn page_name(&self, index: usize) -> Option<&str> {
        (index < self.page_count()).then_some("page.bmp")
    }
    fn supports_concurrent_page_reads(&self) -> bool {
        self.concurrent_reads
    }
    fn read_page(&self, index: usize) -> Result<Vec<u8>, SourceError> {
        let previous = self.reads[index].fetch_add(1, Ordering::AcqRel);
        if index == 1 && previous == 0 {
            if let Some((started, gate)) = &self.gate {
                started.send(()).unwrap();
                gate.wait();
            }
        }
        Ok(self.encoded.clone())
    }
}

fn options(prefetch: bool) -> WorkerOptions {
    WorkerOptions {
        prefetch_enabled: prefetch,
        progressive_preview_enabled: false,
        cache_bytes: 8 * 1024 * 1024,
        ..WorkerOptions::default()
    }
}

fn receive_through(worker: &PageWorker, last: u32) -> Vec<WorkerEvent> {
    let deadline = Instant::now() + TIMEOUT;
    let mut events = Vec::new();
    loop {
        if let Some(event) = worker.try_recv() {
            let done = match &event {
                WorkerEvent::PageReady { page_id, .. } => page_id.0 == last,
                WorkerEvent::PageFailed { message, .. } => panic!("synthetic image: {message}"),
            };
            events.push(event);
            if done {
                return events;
            }
        } else {
            assert!(Instant::now() < deadline, "missing page {last}");
            thread::sleep(Duration::from_millis(1));
        }
    }
}

fn waiting_prefetch(concurrent_reads: bool) -> (PageWorker, Arc<FixtureSource>, Arc<Gate>) {
    let (started, ready) = mpsc::channel();
    let gate = Arc::new(Gate::default());
    let mut source = FixtureSource::new("book-a", 2, 8);
    source.gate = Some((started, gate.clone()));
    source.concurrent_reads = concurrent_reads;
    let source = Arc::new(source);
    let worker = PageWorker::new(Default::default());
    worker.load_book(
        source.clone(),
        0,
        NavigationDirection::Forward,
        1024,
        1,
        options(true),
    );
    ready.recv_timeout(TIMEOUT).unwrap();
    (worker, source, gate)
}

#[test]
fn promoting_prefetched_bytes_reads_once_with_either_prefetch_policy() {
    for prefetch in [true, false] {
        let (mut worker, source, gate) = waiting_prefetch(false);
        worker.set_page(1, NavigationDirection::Forward, 1024, 1, options(prefetch));
        gate.release();
        receive_through(&worker, 1);
        assert_eq!(source.count(1), 1);
        assert!(worker.request_shutdown());
    }
}

#[test]
fn completed_encoded_read_survives_decode_and_target_changes() {
    let (mut worker, source, gate) = waiting_prefetch(false);
    let mut changed = options(false);
    changed.decode.apply_exif_orientation = true;
    worker.set_page(1, NavigationDirection::Forward, 2048, 1, changed);
    gate.release();
    let events = receive_through(&worker, 1);
    assert!(
        matches!(events.last(), Some(WorkerEvent::PageReady { decode, page, .. }) if decode.apply_exif_orientation && page.target_long_edge == 2048)
    );
    assert_eq!(source.count(1), 1);
    assert!(worker.request_shutdown());
}

#[test]
fn source_switch_and_snapshot_refresh_reject_old_completed_bytes() {
    for book in ["book-a", "book-b"] {
        let (mut worker, old_source, gate) = waiting_prefetch(false);
        let mut next = FixtureSource::new(book, 2, 8);
        next.instance = 2;
        let next = Arc::new(next);
        worker.load_book(
            next.clone(),
            1,
            NavigationDirection::Forward,
            1024,
            1,
            options(false),
        );
        gate.release();
        let events = receive_through(&worker, 1);
        assert!(
            matches!(events.last(), Some(WorkerEvent::PageReady { source_instance_id: 2, book_id, .. }) if book_id == book)
        );
        assert_eq!(old_source.count(1), 1);
        assert_eq!(next.count(1), 1);
        assert!(worker.request_shutdown());
    }
}

#[test]
fn target_change_reuses_offscreen_bytes_without_duplicate_overlap() {
    let (mut worker, source, gate) = waiting_prefetch(true);
    worker.set_page(0, NavigationDirection::Forward, 2048, 1, options(true));
    gate.release();
    let events = receive_through(&worker, 1);
    assert!(
        matches!(events.last(), Some(WorkerEvent::PageReady { page, .. }) if page.target_long_edge == 2048)
    );
    assert_eq!(source.count(1), 1);
    assert!(worker.request_shutdown());
}

#[test]
fn dropped_publications_are_prefetched_again_after_worker_lru_eviction() {
    let source = Arc::new(FixtureSource::new("book", 13, 8));
    let mut worker = PageWorker::new(Default::default());
    worker.load_book(
        source.clone(),
        0,
        NavigationDirection::Forward,
        1024,
        1,
        options(true),
    );
    drop(receive_through(&worker, 12));
    worker.set_page(0, NavigationDirection::Forward, 1024, 1, options(true));
    let events = receive_through(&worker, 12);
    assert!(events
        .iter()
        .any(|event| matches!(event, WorkerEvent::PageReady { page_id, .. } if page_id.0 == 1)));
    assert_eq!(source.count(1), 2);
    assert!(worker.request_shutdown());
}

#[test]
fn expired_app_cache_observation_cannot_hide_a_visible_request() {
    for clear_worker_cache in [false, true] {
        let source = Arc::new(FixtureSource::new("book", 2, 8));
        let mut worker = PageWorker::new(Default::default());
        worker.load_book(
            source.clone(),
            0,
            NavigationDirection::Forward,
            1024,
            1,
            options(false),
        );
        let events = receive_through(&worker, 0);
        let WorkerEvent::PageReady {
            page_id,
            page,
            decode,
            ..
        } = &events[0]
        else {
            unreachable!()
        };
        let observed = CachedPageRef::new(CachedPageKey::new(*page_id, 1024, *decode), page);
        if clear_worker_cache {
            assert!(worker.clear_book_blocking());
        }
        drop(events);
        assert_eq!(observed.is_alive(), !clear_worker_cache);
        let mut request = options(false);
        request.app_cached_pages.push(observed);
        worker.load_book(
            source.clone(),
            0,
            NavigationDirection::Forward,
            1024,
            1,
            request,
        );
        receive_through(&worker, 0);
        assert_eq!(source.count(0), if clear_worker_cache { 2 } else { 1 });
        assert!(worker.request_shutdown());
    }
}

#[test]
fn deferred_results_keep_their_budget_and_full_queue_accepts_clear() {
    let source: SharedSource = Arc::new(FixtureSource::new("queue", 14, 1024));
    let mut worker = PageWorker::new(Default::default());
    worker.load_book(
        source,
        0,
        NavigationDirection::Forward,
        1024,
        1,
        options(true),
    );
    let budget = options(true).cache_bytes;
    let deadline = Instant::now() + TIMEOUT;
    while worker.pending_result_bytes() < budget {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(worker.pending_result_bytes(), budget);
    let deferred = worker.try_recv_pending().unwrap();
    // Removing from the transport does not release admission while UI defers it.
    assert_eq!(worker.pending_result_bytes(), budget);
    assert!(
        worker.clear_book_blocking(),
        "full result queue must not hide commands"
    );
    drop(deferred);
    while worker.try_recv().is_some() {}
    assert_eq!(worker.pending_result_bytes(), 0);
    assert!(worker.request_shutdown());
}
