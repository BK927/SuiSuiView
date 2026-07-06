use crate::core::perf_trace::{self, PerfField};
use crate::core::source::SharedSource;
use crossbeam_channel::{bounded, unbounded, Receiver, Sender};
use egui::Context;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

mod api;
mod bmp;
mod cache;
mod decode_ahead;
mod decode_policy;
#[cfg(test)]
mod decoder_tests;
mod gif;
mod image_crate;
mod jpeg;
mod metadata;
mod png;
mod prepare;
mod read_ahead;
mod region;
mod resize;
mod run_loop;
mod scheduler;
mod selection;
mod source_bytes;
#[cfg(feature = "native-webp")]
mod webp;

pub use api::*;
#[cfg(test)]
pub use prepare::prepare_image;
pub use prepare::{
    clamp_navigation_target_long_edge, clamp_target_long_edge, display_dimensions,
    display_dimensions_with_upscale, prepare_image_with_options, prepare_image_with_strategy,
};
// Part of the worker's public API surface (unused in this binary's own `mod core`
// tree, which is compiled with `#[allow(dead_code)]`; the library re-export is
// reachable). Kept as a re-export so `crate::core::worker::is_original_inspection_target`
// stays valid.
#[allow(unused_imports)]
pub use prepare::is_original_inspection_target;
pub use region::{prepare_original_region_with_options, OriginalRegion, PreparedRegion};

#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
use prepare::record_prepare_stage;
use prepare::{
    decoded_byte_size, image_reader, prepared_page_from_luma, prepared_page_from_rgba,
    reject_oversized_dimensions, reject_oversized_original, retained_page_byte_size,
    sampled_index_map,
};

use image_crate::prepare_image_with_image_crate;
use resize::{image_filter_type, resize_luma, resize_rgba};

#[cfg(test)]
use selection::prepare_unavailable_or_image_fallback;

use run_loop::run_worker;

const MAX_IMAGE_DIMENSION: u32 = 20_000;
const MAX_DECODED_PAGE_BYTES: usize = 256 * 1024 * 1024;
const JPEG_SCALED_MIN_RATIO: u32 = 2;
const BMP_SAMPLED_MIN_RATIO: u32 = 2;
const GIF_SAMPLED_MIN_RATIO: u32 = 2;
const PNG_SAMPLED_MIN_RATIO: u32 = 2;

pub struct PageWorker {
    command_tx: Sender<WorkerCommand>,
    event_rx: Receiver<WorkerEvent>,
    shutdown_requested: Arc<AtomicBool>,
    stopped_rx: Receiver<()>,
    join: Option<JoinHandle<()>>,
}

impl PageWorker {
    pub fn new(ctx: Context) -> Self {
        let (command_tx, command_rx) = unbounded();
        let (event_tx, event_rx) = unbounded();
        let (stopped_tx, stopped_rx) = bounded(1);
        let shutdown_requested = Arc::new(AtomicBool::new(false));
        let worker_shutdown_requested = shutdown_requested.clone();
        let join = thread::Builder::new()
            .name("suisuiview-page-worker".to_owned())
            .spawn(move || {
                run_worker(command_rx, event_tx, ctx, worker_shutdown_requested);
                let _ = stopped_tx.send(());
            })
            .expect("page worker thread should start");

        Self {
            command_tx,
            event_rx,
            shutdown_requested,
            stopped_rx,
            join: Some(join),
        }
    }

    pub fn load_book(
        &self,
        source: SharedSource,
        center: usize,
        direction: NavigationDirection,
        target_long_edge: u32,
        visible_pages: usize,
        options: WorkerOptions,
    ) {
        let _ = self.command_tx.send(WorkerCommand::LoadBook {
            source,
            center,
            direction,
            target_long_edge: clamp_target_long_edge(target_long_edge),
            visible_pages: visible_pages.max(1),
            options: options.normalized(),
        });
    }

    pub fn set_page(
        &self,
        center: usize,
        direction: NavigationDirection,
        target_long_edge: u32,
        visible_pages: usize,
        options: WorkerOptions,
    ) {
        let _ = self.command_tx.send(WorkerCommand::SetPage {
            center,
            direction,
            target_long_edge: clamp_target_long_edge(target_long_edge),
            visible_pages: visible_pages.max(1),
            options: options.normalized(),
        });
    }

    pub fn try_recv(&self) -> Option<WorkerEvent> {
        self.event_rx.try_recv().ok()
    }

    pub fn clear_book_blocking(&self) -> bool {
        let (ack, done) = bounded(1);
        if self
            .command_tx
            .send(WorkerCommand::ClearBook { ack })
            .is_err()
        {
            return false;
        }
        done.recv_timeout(Duration::from_millis(1500)).is_ok()
    }

    pub fn request_shutdown(&mut self) -> bool {
        if self.shutdown_requested.swap(true, Ordering::AcqRel) {
            return self.join.is_none();
        }
        let started = Instant::now();
        let sent = self.command_tx.send(WorkerCommand::Shutdown).is_ok();
        let had_thread = self.join.take().is_some();
        let stopped = self
            .stopped_rx
            .recv_timeout(Duration::from_millis(30))
            .is_ok();
        perf_trace::record_duration(
            "shutdown_request",
            started.elapsed(),
            &[
                PerfField::Str("component", "page_worker"),
                PerfField::Bool("command_sent", sent),
                PerfField::Bool("thread_detached", had_thread && !stopped),
                PerfField::Bool("thread_stopped", stopped),
            ],
        );
        stopped
    }
}

impl Drop for PageWorker {
    fn drop(&mut self) {
        self.request_shutdown();
    }
}

#[cfg(test)]
mod core_tests;

#[cfg(test)]
mod luma_tests;
