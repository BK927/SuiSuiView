//! Actual render-chain comparison, sharing the viewer's page layout and input.
use super::{PageCacheKey, SuiSuiViewApp};
use crate::core::source::SharedSource;
use crate::core::worker::{prepare_image_with_options, DecodeOptions, PreparedPage};
use crossbeam_channel::{bounded, unbounded, Receiver, Sender};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread::{self, JoinHandle};
use std::time::Duration;

mod controls;
mod paint;
mod state;
pub(in crate::app) use state::{DebugCompareState, DebugCompareTarget};
pub(in crate::app) struct DebugCompareWorker {
    command_tx: Sender<DebugCompareCommand>,
    event_rx: Receiver<DebugCompareEvent>,
    shutdown_requested: Arc<AtomicBool>,
    stopped_rx: Receiver<()>,
    join: Option<JoinHandle<()>>,
}

impl DebugCompareWorker {
    pub(in crate::app) fn new(ctx: egui::Context) -> Self {
        let (command_tx, command_rx) = unbounded();
        let (event_tx, event_rx) = unbounded();
        let (stopped_tx, stopped_rx) = bounded(1);
        let shutdown_requested = Arc::new(AtomicBool::new(false));
        let worker_shutdown_requested = shutdown_requested.clone();
        let join = thread::Builder::new()
            .name("suisuiview-debug-compare-worker".to_owned())
            .spawn(move || {
                run_debug_compare_worker(command_rx, event_tx, ctx, worker_shutdown_requested);
                let _ = stopped_tx.send(());
            })
            .expect("debug compare worker thread should start");

        Self {
            command_tx,
            event_rx,
            shutdown_requested,
            stopped_rx,
            join: Some(join),
        }
    }

    fn prepare(&self, request: DebugCompareRequest) {
        let _ = self.command_tx.send(DebugCompareCommand::Prepare(request));
    }

    pub(in crate::app) fn try_recv(&self) -> Option<DebugCompareEvent> {
        self.event_rx.try_recv().ok()
    }

    pub(in crate::app) fn request_shutdown(&mut self) -> bool {
        if self.shutdown_requested.swap(true, Ordering::AcqRel) {
            return self.join.is_none();
        }
        let sent = self.command_tx.send(DebugCompareCommand::Shutdown).is_ok();
        let had_thread = self.join.take().is_some();
        let stopped = self
            .stopped_rx
            .recv_timeout(Duration::from_millis(30))
            .is_ok();
        sent && (!had_thread || stopped)
    }
}

impl Drop for DebugCompareWorker {
    fn drop(&mut self) {
        self.request_shutdown();
    }
}

struct DebugCompareRequest {
    book_id: String,
    source: SharedSource,
    page_index: usize,
    target_long_edge: u32,
    decode: DecodeOptions,
}

pub(in crate::app) struct DebugCompareEvent {
    book_id: String,
    page_index: usize,
    target_long_edge: u32,
    decode: DecodeOptions,
    result: Result<Arc<PreparedPage>, String>,
}

enum DebugCompareCommand {
    Prepare(DebugCompareRequest),
    Shutdown,
}

impl SuiSuiViewApp {
    pub(in crate::app) fn drain_debug_compare_events(&mut self) {
        let events = self
            .debug_compare_worker
            .as_ref()
            .map(|worker| {
                let mut events = Vec::new();
                while let Some(event) = worker.try_recv() {
                    events.push(event);
                }
                events
            })
            .unwrap_or_default();
        for event in events {
            let Some(page_id) = self
                .source
                .as_ref()
                .and_then(|source| source.page_id(event.page_index))
            else {
                continue;
            };
            let key = PageCacheKey {
                page_id,
                target_long_edge: event.target_long_edge,
                decode: event.decode,
            };
            self.debug_compare_inflight.remove(&key);

            if self.book_id.as_deref() != Some(event.book_id.as_str())
                || !self.is_relevant_debug_compare_key(key)
            {
                continue;
            }

            match event.result {
                Ok(page) => {
                    if let Some(notice) = page.notice.as_ref() {
                        self.set_status(notice.clone());
                    }
                    self.page_errors.remove(&key);
                    self.insert_prepared_page(key, page);
                    self.prune_decoded_cache();
                }
                Err(message) if self.debug_compare.enabled => {
                    self.notify(format!(
                        "비교 이미지 준비 실패 p.{}: {message}",
                        event.page_index + 1
                    ));
                }
                Err(_) => {}
            }
        }
    }

    pub(in crate::app) fn request_debug_compare_page(&mut self, key: PageCacheKey) {
        if self.debug_compare_inflight.contains(&key) || self.best_page_key(key).is_some() {
            return;
        }
        let Some(source) = self.source.as_ref().cloned() else {
            return;
        };
        let Some(page_index) = source.page_index_for_id(key.page_id) else {
            return;
        };
        let Some(book_id) = self.book_id.clone() else {
            return;
        };
        self.debug_compare_inflight.insert(key);
        let request = DebugCompareRequest {
            book_id,
            source,
            page_index,
            target_long_edge: key.target_long_edge,
            decode: key.decode,
        };
        self.ensure_debug_compare_worker().prepare(request);
    }

    pub(in crate::app) fn debug_compare_pin_keys(&self) -> Vec<PageCacheKey> {
        if !self.debug_compare.enabled {
            return Vec::new();
        }
        self.visible_page_indices()
            .into_iter()
            .filter_map(|index| self.page_key_at(index, self.target_long_edge))
            .collect()
    }

    fn is_relevant_debug_compare_key(&self, key: PageCacheKey) -> bool {
        self.debug_compare.enabled
            && self.target_is_relevant(key.target_long_edge)
            && self.debug_compare_pin_keys().contains(&key)
    }
}
fn run_debug_compare_worker(
    command_rx: Receiver<DebugCompareCommand>,
    event_tx: Sender<DebugCompareEvent>,
    ctx: egui::Context,
    shutdown_requested: Arc<AtomicBool>,
) {
    while !shutdown_requested.load(Ordering::Acquire) {
        let Ok(command) = command_rx.recv() else {
            break;
        };
        let request = match command {
            DebugCompareCommand::Prepare(request) => request,
            DebugCompareCommand::Shutdown => break,
        };
        let result = request
            .source
            .read_page(request.page_index)
            .map_err(|error| error.to_string())
            .and_then(|bytes| {
                prepare_image_with_options(&bytes, request.target_long_edge, request.decode)
            })
            .map(Arc::new);
        let _ = event_tx.send(DebugCompareEvent {
            book_id: request.book_id,
            page_index: request.page_index,
            target_long_edge: request.target_long_edge,
            decode: request.decode,
            result,
        });
        ctx.request_repaint();
    }
}
