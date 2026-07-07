//! Background page-dimension prescan for the virtualized strip.
//!
//! A dedicated worker reads each page's header prefix and extracts its pixel
//! dimensions into an app-side hint store (`strip_dim_hints`). The store is
//! ISOLATED: header dimensions predate EXIF orientation, so only the V2 strip
//! layout is allowed to read it (and only as a fallback behind `page_metrics`).
//! Smart-spread and every other `page_metrics` consumer must never touch it.

use crate::app::image_header::dimensions_from_header;
use crate::app::SuiSuiViewApp;
use crate::core::source::{BookSource, PageId, SharedSource};
use crossbeam_channel::{bounded, Receiver, Sender};
use image::ImageReader;
use std::io::Cursor;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;

/// Header prefix read per page; enough for every supported format's dimension
/// fields without pulling the whole image.
const STRIP_DIM_PREFIX_BYTES: usize = 64 * 1024;
/// Pages coalesced into one delivered batch.
const STRIP_DIM_BATCH: usize = 16;
/// Pending scan jobs; a fresh book kicks one, older jobs are skipped by generation.
const STRIP_DIM_REQUEST_QUEUE: usize = 4;
/// Delivered batches buffered before the worker blocks on the drain.
const STRIP_DIM_EVENT_QUEUE: usize = 64;

struct StripDimScanJob {
    generation: u64,
    source: SharedSource,
    page_ids: Vec<PageId>,
}

pub(in crate::app) struct StripDimBatch {
    generation: u64,
    dims: Vec<(PageId, [u32; 2])>,
}

pub(in crate::app) struct StripDimScanWorker {
    tx: Sender<StripDimScanJob>,
    rx: Receiver<StripDimBatch>,
    generation: Arc<AtomicU64>,
}

impl StripDimScanWorker {
    pub(in crate::app) fn new(ctx: egui::Context) -> Self {
        let (request_tx, request_rx) = bounded::<StripDimScanJob>(STRIP_DIM_REQUEST_QUEUE);
        let (event_tx, event_rx) = bounded::<StripDimBatch>(STRIP_DIM_EVENT_QUEUE);
        let generation = Arc::new(AtomicU64::new(0));
        let worker_generation = generation.clone();

        let _ = thread::Builder::new()
            .name("suisuiview-strip-dims".to_owned())
            .spawn(move || {
                while let Ok(job) = request_rx.recv() {
                    if job.generation != worker_generation.load(Ordering::Acquire) {
                        continue;
                    }
                    let mut batch: Vec<(PageId, [u32; 2])> = Vec::with_capacity(STRIP_DIM_BATCH);
                    for (index, &page_id) in job.page_ids.iter().enumerate() {
                        if job.generation != worker_generation.load(Ordering::Acquire) {
                            break;
                        }
                        if let Some(dims) = scan_page_dimensions(job.source.as_ref(), index) {
                            batch.push((page_id, dims));
                        }
                        if batch.len() >= STRIP_DIM_BATCH
                            && !deliver_batch(
                                &event_tx,
                                &ctx,
                                job.generation,
                                std::mem::take(&mut batch),
                            )
                        {
                            // Receiver dropped: abandon this book's scan.
                            break;
                        }
                    }
                    if !batch.is_empty() {
                        deliver_batch(&event_tx, &ctx, job.generation, batch);
                    }
                }
            });

        Self {
            tx: request_tx,
            rx: event_rx,
            generation,
        }
    }

    pub(in crate::app) fn set_generation(&self, generation: u64) {
        self.generation.store(generation, Ordering::Release);
    }

    pub(in crate::app) fn enqueue(
        &self,
        generation: u64,
        source: SharedSource,
        page_ids: Vec<PageId>,
    ) {
        // Best-effort: a full queue only holds stale jobs the worker will skip.
        let _ = self.tx.try_send(StripDimScanJob {
            generation,
            source,
            page_ids,
        });
    }

    pub(in crate::app) fn try_recv(&self) -> Option<StripDimBatch> {
        self.rx.try_recv().ok()
    }
}

/// Deliver one batch and wake the UI to drain it. `false` means the receiver was
/// dropped (book closed / app exiting), signalling the worker to stop.
fn deliver_batch(
    tx: &Sender<StripDimBatch>,
    ctx: &egui::Context,
    generation: u64,
    dims: Vec<(PageId, [u32; 2])>,
) -> bool {
    if tx.send(StripDimBatch { generation, dims }).is_ok() {
        ctx.request_repaint();
        true
    } else {
        false
    }
}

/// Header-first, decoder-fallback pixel dimensions for one page. `None` when
/// neither path can determine a size (nothing is emitted for that page).
fn scan_page_dimensions(source: &dyn BookSource, index: usize) -> Option<[u32; 2]> {
    let bytes = source
        .read_page_prefix(index, STRIP_DIM_PREFIX_BYTES)
        .ok()?;
    let (width, height) = dimensions_from_header(&bytes).or_else(|| {
        ImageReader::new(Cursor::new(&bytes))
            .with_guessed_format()
            .ok()?
            .into_dimensions()
            .ok()
    })?;
    Some([width, height])
}

impl SuiSuiViewApp {
    fn ensure_strip_dim_scan_worker(&mut self) -> &StripDimScanWorker {
        let generation = self.strip_dim_scan_generation;
        let worker = self
            .strip_dim_scan
            .get_or_insert_with(|| StripDimScanWorker::new(self.egui_ctx.clone()));
        worker.set_generation(generation);
        worker
    }

    /// Bump the generation (invalidating any in-flight scan) and drop the hints
    /// for the previous book.
    pub(in crate::app) fn clear_strip_dim_scan_state(&mut self) {
        self.strip_dim_scan_generation = self.strip_dim_scan_generation.wrapping_add(1);
        if let Some(worker) = self.strip_dim_scan.as_ref() {
            worker.set_generation(self.strip_dim_scan_generation);
        }
        self.strip_dim_hints.clear();
    }

    /// Start prescanning the current book's page dimensions. No-op for a single
    /// page (nothing to virtualize) or if page identity cannot be resolved.
    pub(in crate::app) fn kick_strip_dim_scan(&mut self) {
        let Some(source) = self.source.as_ref() else {
            return;
        };
        let page_count = source.page_count();
        if page_count <= 1 {
            return;
        }
        let page_ids: Vec<PageId> = (0..page_count)
            .map_while(|index| source.page_id(index))
            .collect();
        if page_ids.len() != page_count {
            // A page-id gap would misalign index/id; skip rather than mispair.
            return;
        }
        let source = source.clone();
        let generation = self.strip_dim_scan_generation;
        self.ensure_strip_dim_scan_worker()
            .enqueue(generation, source, page_ids);
    }

    pub(in crate::app) fn drain_strip_dim_scan_events(&mut self) {
        let batches: Vec<StripDimBatch> = match self.strip_dim_scan.as_ref() {
            Some(worker) => {
                let mut batches = Vec::new();
                while let Some(batch) = worker.try_recv() {
                    batches.push(batch);
                }
                batches
            }
            None => return,
        };
        for batch in batches {
            if batch.generation != self.strip_dim_scan_generation {
                continue;
            }
            for (page_id, dims) in batch.dims {
                // `page_metrics` is the authoritative, EXIF-oriented size; never
                // let a pre-orientation header hint shadow it. (V2 reads
                // page_metrics first, hints only as a fallback.)
                if self.page_metrics.contains_key(&page_id) {
                    continue;
                }
                self.strip_dim_hints.insert(page_id, dims);
            }
        }
    }
}
