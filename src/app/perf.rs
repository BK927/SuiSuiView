#![cfg_attr(
    not(any(feature = "perf-dev", feature = "perf-diagnostics")),
    allow(dead_code)
)]

use crate::core::perf_trace::{self, PerfField};
use std::env;
#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::{Duration, Instant};
#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
use sysinfo::{ProcessesToUpdate, System};

const SLOW_UI_UPDATE: Duration = Duration::from_millis(50);
const PERF_FLUSH_TIMEOUT: Duration = Duration::from_millis(200);
#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
const AUTO_PAGE_TURNS_ENV: &str = "SUISUIVIEW_PERF_AUTO_PAGE_TURNS";
#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
const AUTO_PAGE_TURN_INTERVAL_MS_ENV: &str = "SUISUIVIEW_PERF_AUTO_PAGE_TURN_INTERVAL_MS";
#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
const AUTO_PAGE_TURN_CLOSE_DELAY_MS_ENV: &str = "SUISUIVIEW_PERF_AUTO_PAGE_TURN_CLOSE_DELAY_MS";
#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
const START_PAGE_INDEX_ENV: &str = "SUISUIVIEW_PERF_START_PAGE_INDEX";
const ADJACENT_SEED_PREFETCH_ENV: &str = "SUISUIVIEW_EXPERIMENT_ADJACENT_SEED_PREFETCH";
const ADJACENT_SEED_MEMORY_GUARD_ENV: &str = "SUISUIVIEW_EXPERIMENT_ADJACENT_SEED_MEMORY_GUARD";
const TEXTURE_PREWARM_ENV: &str = "SUISUIVIEW_EXPERIMENT_TEXTURE_PREWARM";
const ORIGINAL_TEXTURE_ONLY_ENV: &str = "SUISUIVIEW_EXPERIMENT_ORIGINAL_TEXTURE_ONLY";
#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
const AUTO_PAGE_TURN_INITIAL_DELAY: Duration = Duration::from_millis(1500);
#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
const AUTO_PAGE_TURN_DEFAULT_INTERVAL: Duration = Duration::from_millis(220);
#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
const AUTO_PAGE_TURN_DEFAULT_CLOSE_DELAY: Duration = Duration::from_millis(2500);

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

#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
pub(super) struct AutoPageTurnDriver {
    remaining_turns: usize,
    interval: Duration,
    close_delay: Duration,
    next_turn_at: Option<Instant>,
    close_at: Option<Instant>,
}

#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
pub(super) enum AutoPageTurnAction {
    Wait(Duration),
    Turn,
    Close,
}

#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
impl AutoPageTurnDriver {
    pub(super) fn from_env() -> Option<Self> {
        let remaining_turns = parse_env_usize(AUTO_PAGE_TURNS_ENV)?;
        if remaining_turns == 0 {
            return None;
        }
        Some(Self {
            remaining_turns,
            interval: parse_env_millis(AUTO_PAGE_TURN_INTERVAL_MS_ENV)
                .unwrap_or(AUTO_PAGE_TURN_DEFAULT_INTERVAL),
            close_delay: parse_env_millis(AUTO_PAGE_TURN_CLOSE_DELAY_MS_ENV)
                .unwrap_or(AUTO_PAGE_TURN_DEFAULT_CLOSE_DELAY),
            next_turn_at: None,
            close_at: None,
        })
    }

    pub(super) fn update(&mut self, has_source: bool, now: Instant) -> AutoPageTurnAction {
        if !has_source {
            return AutoPageTurnAction::Wait(Duration::from_millis(50));
        }

        if self.remaining_turns > 0 {
            let next_turn_at = *self
                .next_turn_at
                .get_or_insert(now + AUTO_PAGE_TURN_INITIAL_DELAY);
            if now < next_turn_at {
                return AutoPageTurnAction::Wait(next_turn_at - now);
            }
            self.remaining_turns = self.remaining_turns.saturating_sub(1);
            self.next_turn_at = Some(now + self.interval);
            return AutoPageTurnAction::Turn;
        }

        let close_at = *self.close_at.get_or_insert(now + self.close_delay);
        if now < close_at {
            AutoPageTurnAction::Wait(close_at - now)
        } else {
            AutoPageTurnAction::Close
        }
    }
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

pub(super) fn adjacent_seed_prefetch_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        adjacent_seed_prefetch_value_enabled(env::var(ADJACENT_SEED_PREFETCH_ENV).ok().as_deref())
    })
}

pub(super) fn adjacent_seed_memory_guard_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        adjacent_seed_memory_guard_value_enabled(
            env::var(ADJACENT_SEED_MEMORY_GUARD_ENV).ok().as_deref(),
        )
    })
}

pub(super) fn texture_prewarm_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        default_enabled_experiment_value(env::var(TEXTURE_PREWARM_ENV).ok().as_deref())
    })
}

pub(super) fn original_texture_only_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        opt_in_experiment_value_enabled(env::var(ORIGINAL_TEXTURE_ONLY_ENV).ok().as_deref())
    })
}

fn adjacent_seed_prefetch_value_enabled(value: Option<&str>) -> bool {
    default_enabled_experiment_value(value)
}

fn adjacent_seed_memory_guard_value_enabled(value: Option<&str>) -> bool {
    opt_in_experiment_value_enabled(value)
}

fn default_enabled_experiment_value(value: Option<&str>) -> bool {
    !matches!(
        value.map(str::trim),
        Some(value)
            if value.eq_ignore_ascii_case("0")
                || value.eq_ignore_ascii_case("false")
                || value.eq_ignore_ascii_case("off")
                || value.eq_ignore_ascii_case("no")
    )
}

fn opt_in_experiment_value_enabled(value: Option<&str>) -> bool {
    matches!(
        value.map(str::trim),
        Some(value)
            if value.eq_ignore_ascii_case("1")
                || value.eq_ignore_ascii_case("true")
                || value.eq_ignore_ascii_case("on")
                || value.eq_ignore_ascii_case("yes")
    )
}

pub(super) fn record_adjacent_seed_prefetch_prepare(
    started: Instant,
    origin: &'static str,
    direction: &'static str,
    page: usize,
    target_long_edge: u32,
    success: bool,
) {
    perf_trace::record_duration(
        "adjacent_seed_prefetch_prepare",
        started.elapsed(),
        &[
            PerfField::Str("origin", origin),
            PerfField::Str("direction", direction),
            PerfField::Usize("page", page),
            PerfField::U32("target_long_edge", target_long_edge),
            PerfField::Bool("success", success),
        ],
    );
}

pub(super) fn record_adjacent_seed_prefetch_hit(hit: bool, target_long_edge: u32) {
    perf_trace::record_duration(
        "adjacent_seed_prefetch_hit",
        Duration::ZERO,
        &[
            PerfField::Bool("hit", hit),
            PerfField::U32("target_long_edge", target_long_edge),
        ],
    );
}

#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
pub(super) fn forced_start_page_index() -> Option<usize> {
    parse_env_usize(START_PAGE_INDEX_ENV)
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

pub(super) fn record_page_turn_request(
    cache_state: PageCacheState,
    page: usize,
    target_long_edge: u32,
) {
    perf_trace::record_duration(
        "page_turn_request",
        Duration::ZERO,
        &[
            PerfField::Bool("cached", cache_state.cached()),
            PerfField::Str("cache_state", cache_state.as_str()),
            PerfField::Usize("page", page),
            PerfField::U32("target_long_edge", target_long_edge),
        ],
    );
}

#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
#[derive(Debug, Clone, Copy)]
pub(super) struct AppCacheSnapshot {
    pub(super) reason: &'static str,
    pub(super) current_page: usize,
    pub(super) target_long_edge: u32,
    pub(super) decoded_pages: usize,
    pub(super) decoded_bytes: usize,
    pub(super) decoded_budget_bytes: usize,
    pub(super) upscaled_pages: usize,
    pub(super) upscaled_bytes: usize,
    pub(super) upscaled_budget_bytes: usize,
    pub(super) textures: usize,
    pub(super) texture_bytes: usize,
}

#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
pub(super) fn record_app_cache_snapshot(snapshot: AppCacheSnapshot) {
    let process_memory_bytes = process_memory_bytes().unwrap_or(0);
    perf_trace::record_duration(
        "app_cache_snapshot",
        Duration::ZERO,
        &[
            PerfField::Str("reason", snapshot.reason),
            PerfField::Usize("current_page", snapshot.current_page),
            PerfField::U32("target_long_edge", snapshot.target_long_edge),
            PerfField::Usize("decoded_pages", snapshot.decoded_pages),
            PerfField::Usize("decoded_bytes", snapshot.decoded_bytes),
            PerfField::Usize("decoded_budget_bytes", snapshot.decoded_budget_bytes),
            PerfField::Usize("upscaled_pages", snapshot.upscaled_pages),
            PerfField::Usize("upscaled_bytes", snapshot.upscaled_bytes),
            PerfField::Usize("upscaled_budget_bytes", snapshot.upscaled_budget_bytes),
            PerfField::Usize("textures", snapshot.textures),
            PerfField::Usize("texture_bytes", snapshot.texture_bytes),
            PerfField::Usize("process_memory_bytes", process_memory_bytes),
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

pub(super) fn record_texture_prewarm(
    started: Instant,
    page: usize,
    target_long_edge: u32,
    upscaled: bool,
) {
    perf_trace::record_duration_if_at_least(
        "texture_prewarm",
        started.elapsed(),
        Duration::from_millis(16),
        &[
            PerfField::Usize("page", page),
            PerfField::U32("target_long_edge", target_long_edge),
            PerfField::Bool("upscaled", upscaled),
        ],
    );
}

#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
fn process_memory_bytes() -> Option<usize> {
    static SYSTEM: OnceLock<Mutex<System>> = OnceLock::new();

    let pid = sysinfo::get_current_pid().ok()?;
    let mut system = SYSTEM
        .get_or_init(|| Mutex::new(System::new()))
        .lock()
        .ok()?;
    system.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
    let memory = system.process(pid)?.memory();
    usize::try_from(memory).ok()
}

#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
fn parse_env_usize(name: &str) -> Option<usize> {
    env::var(name).ok()?.parse().ok()
}

#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
fn parse_env_millis(name: &str) -> Option<Duration> {
    parse_env_usize(name).map(|millis| Duration::from_millis(millis as u64))
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

#[cfg(test)]
mod tests {
    use super::{
        adjacent_seed_memory_guard_value_enabled, adjacent_seed_prefetch_value_enabled,
        opt_in_experiment_value_enabled,
    };

    #[test]
    fn adjacent_seed_prefetch_is_default_on_with_explicit_opt_out() {
        assert!(adjacent_seed_prefetch_value_enabled(None));
        assert!(adjacent_seed_prefetch_value_enabled(Some("")));
        assert!(adjacent_seed_prefetch_value_enabled(Some("1")));
        assert!(adjacent_seed_prefetch_value_enabled(Some("true")));
        assert!(!adjacent_seed_prefetch_value_enabled(Some("0")));
        assert!(!adjacent_seed_prefetch_value_enabled(Some("false")));
        assert!(!adjacent_seed_prefetch_value_enabled(Some("OFF")));
        assert!(!adjacent_seed_prefetch_value_enabled(Some(" no ")));
    }

    #[test]
    fn adjacent_seed_memory_guard_is_opt_in() {
        assert!(!adjacent_seed_memory_guard_value_enabled(None));
        assert!(!adjacent_seed_memory_guard_value_enabled(Some("")));
        assert!(!adjacent_seed_memory_guard_value_enabled(Some("0")));
        assert!(adjacent_seed_memory_guard_value_enabled(Some("1")));
        assert!(adjacent_seed_memory_guard_value_enabled(Some("true")));
        assert!(adjacent_seed_memory_guard_value_enabled(Some(" ON ")));
    }

    #[test]
    fn opt_in_experiment_requires_affirmative_value() {
        assert!(!opt_in_experiment_value_enabled(None));
        assert!(!opt_in_experiment_value_enabled(Some("")));
        assert!(opt_in_experiment_value_enabled(Some("1")));
        assert!(opt_in_experiment_value_enabled(Some("true")));
        assert!(opt_in_experiment_value_enabled(Some("ON")));
        assert!(opt_in_experiment_value_enabled(Some(" yes ")));
        assert!(!opt_in_experiment_value_enabled(Some("0")));
        assert!(!opt_in_experiment_value_enabled(Some("false")));
        assert!(!opt_in_experiment_value_enabled(Some("off")));
    }
}
