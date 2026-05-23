#![cfg_attr(
    not(any(feature = "perf-dev", feature = "perf-diagnostics")),
    allow(dead_code)
)]

use crate::core::perf_trace::{self, PerfField};
use std::time::{Duration, Instant};

const SLOW_UI_UPDATE: Duration = Duration::from_millis(50);
const PERF_FLUSH_TIMEOUT: Duration = Duration::from_millis(200);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PageCacheState {
    DecodedExact,
    DecodedPreview,
    DecodedFallback,
    UpscaledExact,
    UpscaledPreview,
    UpscaledFallback,
    Miss,
}

impl PageCacheState {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::DecodedExact => "decoded_exact",
            Self::DecodedPreview => "decoded_preview",
            Self::DecodedFallback => "decoded_fallback",
            Self::UpscaledExact => "upscaled_exact",
            Self::UpscaledPreview => "upscaled_preview",
            Self::UpscaledFallback => "upscaled_fallback",
            Self::Miss => "miss",
        }
    }

    pub(super) fn cached(self) -> bool {
        !matches!(self, Self::Miss)
    }
}

#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
pub(super) struct OpenToFirstVisibleTrace {
    origin: &'static str,
    started_at: Instant,
    book_id: Option<String>,
}

#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
impl OpenToFirstVisibleTrace {
    pub(super) fn new(origin: &'static str) -> Self {
        Self {
            origin,
            started_at: Instant::now(),
            book_id: None,
        }
    }
}

pub(super) fn record_app_new(started: Instant) {
    perf_trace::record_duration_if_at_least(
        "app_new",
        started.elapsed(),
        Duration::from_millis(50),
        &[],
    );
}

pub(super) fn record_open_source(started: Instant, origin: &'static str, success: bool) {
    perf_trace::record_duration_if_at_least(
        "open_source",
        started.elapsed(),
        Duration::from_millis(50),
        &[
            PerfField::Str("origin", origin),
            PerfField::Bool("success", success),
        ],
    );
}

#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
pub(super) fn arm_open_to_first_visible(
    trace: &mut Option<OpenToFirstVisibleTrace>,
    book_id: &str,
) {
    if let Some(trace) = trace.as_mut() {
        trace.book_id = Some(book_id.to_owned());
    }
}

#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
pub(super) fn record_open_to_first_visible_if_pending(
    trace: &mut Option<OpenToFirstVisibleTrace>,
    active_book_id: Option<&str>,
    page: usize,
    target_long_edge: u32,
    gpu_effects: bool,
) {
    let Some(pending) = trace.as_ref() else {
        return;
    };
    if pending.book_id.as_deref() != active_book_id {
        return;
    }
    let pending = trace.take().expect("checked pending first-visible trace");
    perf_trace::record_duration(
        "open_to_first_visible",
        pending.started_at.elapsed(),
        &[
            PerfField::Str("origin", pending.origin),
            PerfField::Usize("page", page),
            PerfField::U32("target_long_edge", target_long_edge),
            PerfField::Bool("gpu_effects", gpu_effects),
        ],
    );
}

pub(super) fn record_page_turn_ready(
    started: Instant,
    cache_state: PageCacheState,
    page: usize,
    target_long_edge: u32,
) {
    perf_trace::record_duration(
        "page_turn_ready",
        started.elapsed(),
        &[
            PerfField::Bool("cached", cache_state.cached()),
            PerfField::Str("cache_state", cache_state.as_str()),
            PerfField::Usize("page", page),
            PerfField::U32("target_long_edge", target_long_edge),
        ],
    );
}

pub(super) fn record_close_book_worker_clear(started: Instant, completed: bool) {
    perf_trace::record_duration_if_at_least(
        "close_book_worker_clear",
        started.elapsed(),
        Duration::from_millis(50),
        &[PerfField::Bool("completed", completed)],
    );
}

pub(super) fn record_page_effects_cpu(started: Instant, page: usize, target_long_edge: u32) {
    perf_trace::record_duration_if_at_least(
        "page_effects_cpu",
        started.elapsed(),
        Duration::from_millis(25),
        &[
            PerfField::Usize("page", page),
            PerfField::U32("target_long_edge", target_long_edge),
        ],
    );
}

pub(super) fn record_texture_load(
    started: Instant,
    page: usize,
    target_long_edge: u32,
    upscaled: bool,
) {
    perf_trace::record_duration_if_at_least(
        "texture_load",
        started.elapsed(),
        Duration::from_millis(16),
        &[
            PerfField::Usize("page", page),
            PerfField::U32("target_long_edge", target_long_edge),
            PerfField::Bool("upscaled", upscaled),
        ],
    );
}

pub(super) fn record_ui_update(started: Instant, has_book: bool, transition: bool) {
    perf_trace::record_duration_if_at_least(
        "ui_update",
        started.elapsed(),
        SLOW_UI_UPDATE,
        &[
            PerfField::Bool("has_book", has_book),
            PerfField::Bool("transition", transition),
        ],
    );
}

pub(super) fn record_app_shutdown(
    started: Instant,
    had_book: bool,
    page_worker_stopped: bool,
    debug_compare_stopped: bool,
    thumbnails_stopped: bool,
    upscale_stopped: bool,
) {
    perf_trace::record_duration(
        "app_shutdown",
        started.elapsed(),
        &[
            PerfField::Bool("had_book", had_book),
            PerfField::Bool("page_worker_stopped", page_worker_stopped),
            PerfField::Bool("debug_compare_stopped", debug_compare_stopped),
            PerfField::Bool("thumbnails_stopped", thumbnails_stopped),
            PerfField::Bool("upscale_stopped", upscale_stopped),
        ],
    );
}

pub(super) fn flush() {
    let _ = perf_trace::flush_timeout(PERF_FLUSH_TIMEOUT);
}
