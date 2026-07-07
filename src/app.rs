use crate::core::effects::ViewEffects;
use crate::core::formats::OPENABLE_FILE_EXTENSIONS;
use crate::core::source::{BookSource, SharedSource};
use crate::core::state::{
    AppSettings, BookRecordInput, DecodeMode, DecoderPreferences, FastStartFailureNotice, FitMode,
    ReadingDirection, StateStore, WgpuUpscaleMethod,
};
use crate::core::worker::{
    DecodeOptions, DecodeStrategy, NavigationDirection, PageWorker, PreparedPage, WorkerEvent,
    WorkerOptions, DEFAULT_TARGET_LONG_EDGE, PREVIEW_TARGET_LONG_EDGE,
};
use commands::{collect_keyboard_actions, AppCommand, KeyboardAction, NavigationRelease};
use crossbeam_channel::{unbounded, Receiver, Sender};
use debug_compare::{DebugCompareState, DebugCompareWorker};
use egui::{self, Pos2, Rect, Vec2};
use image_info::ImageInfoState;
use lru::LruCache;
use rfd::FileDialog;
use std::collections::{HashMap, HashSet, VecDeque};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::time::{Duration, Instant};
use ui::{BookmarkFilter, BookmarkRowsCache, BookmarkThumbnails};

mod about;
mod adjacent_seed;
mod cache;
mod commands;
mod context_menu;
mod debug_compare;
mod delete_dialog;
mod deletion;
mod edge_prompt;
pub(crate) mod fast_start;
#[cfg(target_os = "windows")]
mod file_associations;
mod gpu_paint;
mod image_header;
mod image_info;
mod navigation;
mod opening;
mod perf;
mod platform;
mod realtime_sr;
mod refresh;
mod runtime;
mod settings;
mod settings_bookmarks;
mod settings_input;
mod settings_performance;
mod settings_rendering;
mod sibling_books;
mod texture_prewarm;
mod ui;
mod update_loop;
mod upscale_probe;
mod viewer;
mod window;
pub(crate) mod winit_host;

#[cfg(test)]
use crate::core::effects::{apply_effects_to_image, transformed_page_size};
#[cfg(test)]
use crate::core::worker::preview_prefetch_indices;
pub(in crate::app) use adjacent_seed::{AdjacentSeedCache, AdjacentSeedEvent, SeededPreparedPage};
#[cfg(test)]
use cache::{
    automatic_total_budget_bytes_for, best_page_key_excluding_preview_fallback_in_cache,
    best_page_key_in_cache, cache_budget_bytes, final_quality_page_key_in_cache,
    lower_resolution_page_keys, page_cache_state_from_hit, prepared_target_intent_for_view,
    texture_cache_budget_bytes_for, texture_cache_budget_cap_bytes,
    touch_normal_navigation_page_keys,
};
pub(in crate::app) use cache::{
    gpu_intermediate_texture_budget_bytes, gpu_source_texture_budget_bytes, gpu_visual_needs_wgsl,
    rect_target_size, should_allow_cpu_display_upscale, total_memory_budget_bytes, PageCacheKey,
    TextureCacheKey, TextureEntry, TextureSampling, BYTES_PER_RGBA_PIXEL,
};
pub(in crate::app) use delete_dialog::PendingDeleteDialog;
pub(in crate::app) use edge_prompt::EdgePrompt;
pub(crate) use opening::{start_startup_open_loader, StartupOpen};
pub(in crate::app) use opening::{LoaderEvent, OpenOrigin};

/// Spawn a fresh copy of this process (winit forbids creating a second event
/// loop in-process, so degrading renderers requires a relaunch).
pub(crate) fn restart_current_process() -> Result<(), String> {
    platform::restart_current_process()
}
pub(in crate::app) use navigation::SiblingOpenRetry;
pub(in crate::app) use refresh::{RefreshOutcome, RefreshTicket};
#[cfg(test)]
use sibling_books::adjacent_sibling_book_paths;
pub(in crate::app) use sibling_books::{adjacent_sibling_book_paths_ordered, sibling_book_path};
#[cfg(test)]
use viewer::{
    double_spread_indices, ordered_spread_indices, relative_difference,
    smart_spread_indices_for_metrics, transition_paint_params,
};
pub(in crate::app) use viewer::{
    page_visual_size, texture_options_for_sampling, transition_screen_sign,
    worker_center_page_for_mode, CurrentViewState, PageMetrics, PageRenderInfo, PageVisual,
    Transition, UpscaleDecisionOrigin, ViewMode, ViewTargetSettle,
};

#[cfg(test)]
use crate::core::effects::ImageFilter;
#[cfg(test)]
use crate::core::effects::ViewTransform;
#[cfg(test)]
use commands::command_for_shortcut;
#[cfg(test)]
use platform::{korean_font_candidates, load_first_existing_font, sanitize_font_name};
const STATE_SAVE_DEBOUNCE: Duration = Duration::from_millis(750);
pub(in crate::app) const TEXTURE_PRESENT_REPAINT_DELAY: Duration = Duration::from_millis(16);
pub(in crate::app) const SIBLING_BOOK_TURN_REPAINT_DELAY: Duration = Duration::from_millis(16);
const WORKER_EVENT_DRAIN_BUDGET: Duration = Duration::from_millis(4);

#[derive(Debug, Clone)]
struct PendingBookmarkJump {
    book_id: String,
    path: PathBuf,
    page: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WindowTitleSnapshot {
    title: String,
    page: usize,
    total_pages: usize,
}

impl WindowTitleSnapshot {
    fn matches(&self, title: &str, page: usize, total_pages: usize) -> bool {
        self.title == title && self.page == page && self.total_pages == total_pages
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BookmarkDeleteDialog {
    scope: BookmarkFilter,
    count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PendingPageTurn {
    target: usize,
    direction: NavigationDirection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct QueuedPageTurns {
    direction: NavigationDirection,
    remaining: usize,
}

pub struct SuiSuiViewApp {
    egui_ctx: egui::Context,
    store: StateStore,
    settings: AppSettings,
    settings_open: bool,
    settings_section: settings::SettingsSection,
    #[cfg(target_os = "windows")]
    file_association_selection: file_associations::FileAssociationSelection,
    pending_gpu_acceleration: Option<bool>,
    startup_reveal_pending: bool,
    // Set when the app asks to close (esc-to-quit, restart, etc). The winit
    // host reads this to exit; a ViewportCommand::Close is also sent.
    close_requested: bool,
    fast_start_failure_notice: Option<FastStartFailureNotice>,
    shortcut_capture: Option<settings_input::ShortcutCapture>,
    shortcut_conflict: Option<settings_input::ShortcutConflict>,
    shortcut_expanded_groups: HashSet<&'static str>,
    about_open: bool,
    about_section: about::AboutSection,
    image_info: ImageInfoState,
    worker: PageWorker,
    deferred_worker_events: VecDeque<WorkerEvent>,
    loader_tx: Sender<LoaderEvent>,
    loader_rx: Receiver<LoaderEvent>,
    loader_pending: bool,
    refresh_tx: Sender<RefreshOutcome>,
    refresh_rx: Receiver<RefreshOutcome>,
    refresh_inflight: Option<RefreshTicket>,
    adjacent_seed_tx: Sender<AdjacentSeedEvent>,
    adjacent_seed_rx: Receiver<AdjacentSeedEvent>,
    adjacent_seed_generation: u64,
    adjacent_seed_generation_token: Arc<AtomicU64>,
    adjacent_seed_cache: Vec<AdjacentSeedCache>,
    pending_adjacent_seed_prefetch_at: Option<Instant>,
    ipc_rx: Option<Receiver<Option<PathBuf>>>,
    loader_generation: u64,
    source: Option<SharedSource>,
    book_id: Option<String>,
    open_origin: Option<OpenOrigin>,
    opened_path: Option<PathBuf>,
    current_page: usize,
    view_mode: ViewMode,
    reading_direction: ReadingDirection,
    fit_mode: FitMode,
    manual_zoom: f32,
    effects: ViewEffects,
    target_long_edge: u32,
    current_view_state: Option<CurrentViewState>,
    last_viewer_size_points: Option<Vec2>,
    pan: Vec2,
    decoded_pages: LruCache<PageCacheKey, Arc<PreparedPage>>,
    decoded_bytes: usize,
    page_metrics: HashMap<crate::core::source::PageId, PageMetrics>,
    page_errors: HashMap<PageCacheKey, String>,
    textures: LruCache<TextureCacheKey, TextureEntry>,
    debug_compare: DebugCompareState,
    debug_compare_worker: Option<DebugCompareWorker>,
    debug_compare_inflight: HashSet<PageCacheKey>,
    upscale_probe_worker: Option<upscale_probe::UpscaleProbeWorker>,
    upscale_probe_generation: u64,
    probe_page_results: Vec<upscale_probe::PageProbeResult>,
    probed_page_ids: HashSet<crate::core::source::PageId>,
    upscale_probe_failures: usize,
    book_upscale_decision: Option<WgpuUpscaleMethod>,
    bookmark_thumbnails: Option<BookmarkThumbnails>,
    gpu_effects_available: bool,
    gpu_target_format: Option<wgpu::TextureFormat>,
    last_nav_direction: NavigationDirection,
    transition: Option<Transition>,
    pending_page_turn: Option<PendingPageTurn>,
    queued_page_turns: Option<QueuedPageTurns>,
    page_turn_paint_hold: bool,
    queued_sibling_book_turns: VecDeque<isize>,
    sibling_book_visual_pending: bool,
    sibling_book_wgpu_present_wait: Option<(u64, usize)>,
    sibling_book_visual_hold_until: Option<Instant>,
    sibling_open_retry: Option<SiblingOpenRetry>,
    view_target_settle: ViewTargetSettle,
    pending_original_inspection_cache_cleanup_at: Option<Instant>,
    pending_gpu_original_inspection_cleanup: bool,
    fullscreen: bool,
    maximized: bool,
    window_position_checked: bool,
    window_last_native_pixels_per_point: Option<f32>,
    window_size_save_block_until: Option<Instant>,
    view_target_update_block_until: Option<Instant>,
    window_dpi_size_guard: Option<window::WindowDpiSizeGuard>,
    window_stable_inner_size: Option<[f32; 2]>,
    window_title: Option<WindowTitleSnapshot>,
    top_bar_auto_hide_until: Option<Instant>,
    status: String,
    status_updated_at: Instant,
    toast: String,
    toast_updated_at: Instant,
    pending_state_save_at: Option<Instant>,
    #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
    page_turn_started_at: Option<(usize, Instant)>,
    #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
    open_to_first_visible_trace: Option<perf::OpenToFirstVisibleTrace>,
    #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
    auto_page_turn_driver: Option<perf::AutoPageTurnDriver>,
    bookmark_popover_open: bool,
    bookmark_popover_pos: Pos2,
    bookmark_popover_anchor: Option<Rect>,
    bookmark_filter: BookmarkFilter,
    bookmark_search: String,
    bookmark_delete_dialog: Option<BookmarkDeleteDialog>,
    bookmark_rows: BookmarkRowsCache,
    pending_bookmark_jump: Option<PendingBookmarkJump>,
    pending_delete_dialog: Option<PendingDeleteDialog>,
    edge_prompt: Option<EdgePrompt>,
}

impl SuiSuiViewApp {
    pub(crate) fn new(
        runtime: runtime::AppRuntime,
        store: StateStore,
        ipc_rx: Option<Receiver<Option<PathBuf>>>,
        startup_open_path: Option<PathBuf>,
        startup_open: Option<StartupOpen>,
    ) -> Self {
        let app_started = Instant::now();
        let egui_ctx = runtime.egui_ctx().clone();
        let startup_reveal_pending =
            runtime.startup_reveal() == runtime::StartupReveal::AfterFirstFrame;
        platform::install_app_fonts(&egui_ctx);
        ui::apply_app_theme(&egui_ctx);
        let startup_open_parts = startup_open.map(StartupOpen::into_parts);
        let (loader_tx, loader_rx, loader_generation, loader_pending, startup_open_trace) =
            match startup_open_parts {
                Some(parts) => (
                    parts.loader_tx,
                    parts.loader_rx,
                    parts.generation,
                    true,
                    Some((parts.origin, parts.started_at)),
                ),
                None => {
                    let (loader_tx, loader_rx) = unbounded();
                    (loader_tx, loader_rx, 0, false, None)
                }
            };
        let (adjacent_seed_tx, adjacent_seed_rx) = unbounded();
        let (refresh_tx, refresh_rx) = unbounded();
        let settings = store.settings().clone();
        let fast_start_failure_notice = store.fast_start_failure_notice().cloned();
        let initial_window_size = store.window_placement().inner_size;
        platform::apply_window_level(&egui_ctx, settings.always_on_top);
        let mut app = Self {
            egui_ctx: egui_ctx.clone(),
            store,
            settings: settings.clone(),
            settings_open: false,
            settings_section: settings::SettingsSection::default(),
            #[cfg(target_os = "windows")]
            file_association_selection: file_associations::FileAssociationSelection::default(),
            pending_gpu_acceleration: None,
            startup_reveal_pending,
            close_requested: false,
            fast_start_failure_notice,
            shortcut_capture: None,
            shortcut_conflict: None,
            shortcut_expanded_groups: HashSet::new(),
            about_open: false,
            about_section: about::AboutSection::default(),
            image_info: ImageInfoState::new(),
            worker: PageWorker::new(egui_ctx.clone()),
            deferred_worker_events: VecDeque::new(),
            loader_tx,
            loader_rx,
            loader_pending,
            refresh_tx,
            refresh_rx,
            refresh_inflight: None,
            adjacent_seed_tx,
            adjacent_seed_rx,
            adjacent_seed_generation: 0,
            adjacent_seed_generation_token: Arc::new(AtomicU64::new(0)),
            adjacent_seed_cache: Vec::new(),
            pending_adjacent_seed_prefetch_at: None,
            ipc_rx,
            loader_generation,
            source: None,
            book_id: None,
            open_origin: None,
            opened_path: None,
            current_page: 0,
            view_mode: ViewMode::Single,
            reading_direction: ReadingDirection::default(),
            fit_mode: FitMode::default(),
            manual_zoom: 1.0,
            effects: ViewEffects::default(),
            target_long_edge: DEFAULT_TARGET_LONG_EDGE,
            current_view_state: None,
            last_viewer_size_points: None,
            pan: Vec2::ZERO,
            decoded_pages: LruCache::new(NonZeroUsize::new(64).unwrap()),
            decoded_bytes: 0,
            page_metrics: HashMap::new(),
            page_errors: HashMap::new(),
            textures: LruCache::new(NonZeroUsize::new(12).unwrap()),
            debug_compare: DebugCompareState::default(),
            debug_compare_worker: None,
            debug_compare_inflight: HashSet::new(),
            upscale_probe_worker: None,
            upscale_probe_generation: 0,
            probe_page_results: Vec::new(),
            probed_page_ids: HashSet::new(),
            upscale_probe_failures: 0,
            book_upscale_decision: None,
            bookmark_thumbnails: None,
            // Both stages start on Glow; the WGPU stage patches these in
            // `begin_handoff` once its render state is available.
            gpu_effects_available: false,
            gpu_target_format: None,
            last_nav_direction: NavigationDirection::Forward,
            transition: None,
            pending_page_turn: None,
            queued_page_turns: None,
            page_turn_paint_hold: false,
            queued_sibling_book_turns: VecDeque::new(),
            sibling_book_visual_pending: false,
            sibling_book_wgpu_present_wait: None,
            sibling_book_visual_hold_until: None,
            sibling_open_retry: None,
            view_target_settle: ViewTargetSettle::default(),
            pending_original_inspection_cache_cleanup_at: None,
            pending_gpu_original_inspection_cleanup: false,
            fullscreen: false,
            maximized: false,
            window_position_checked: false,
            window_last_native_pixels_per_point: None,
            window_size_save_block_until: None,
            view_target_update_block_until: None,
            window_dpi_size_guard: None,
            window_stable_inner_size: initial_window_size,
            window_title: None,
            top_bar_auto_hide_until: None,
            status: String::new(),
            status_updated_at: Instant::now(),
            toast: String::new(),
            toast_updated_at: Instant::now(),
            pending_state_save_at: None,
            #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
            page_turn_started_at: None,
            #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
            open_to_first_visible_trace: None,
            #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
            auto_page_turn_driver: perf::AutoPageTurnDriver::from_env(),
            bookmark_popover_open: false,
            bookmark_popover_pos: Pos2::new(900.0, 72.0),
            bookmark_popover_anchor: None,
            bookmark_filter: BookmarkFilter::default(),
            bookmark_search: String::new(),
            bookmark_delete_dialog: None,
            bookmark_rows: BookmarkRowsCache::default(),
            pending_bookmark_jump: None,
            pending_delete_dialog: None,
            edge_prompt: None,
        };
        if let Some((origin, started_at)) = startup_open_trace {
            #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
            {
                app.open_to_first_visible_trace = Some(perf::OpenToFirstVisibleTrace::new_at(
                    origin.perf_label(),
                    started_at,
                ));
            }
            #[cfg(not(any(feature = "perf-dev", feature = "perf-diagnostics")))]
            let _ = (origin, started_at);
            app.set_status(app.i18n().text("status.opening"));
            egui_ctx.request_repaint();
        } else if let Some(path) = startup_open_path {
            app.open_path(path);
        }
        perf::record_app_new(app_started);
        app
    }

    fn ensure_debug_compare_worker(&mut self) -> &DebugCompareWorker {
        self.debug_compare_worker
            .get_or_insert_with(|| DebugCompareWorker::new(self.egui_ctx.clone()))
    }

    fn ensure_upscale_probe_worker(&mut self) -> &upscale_probe::UpscaleProbeWorker {
        let generation = self.upscale_probe_generation;
        let worker = self
            .upscale_probe_worker
            .get_or_insert_with(|| upscale_probe::UpscaleProbeWorker::new(self.egui_ctx.clone()));
        worker.set_generation(generation);
        worker
    }

    fn ensure_bookmark_thumbnails(&mut self) -> &mut BookmarkThumbnails {
        self.bookmark_thumbnails
            .get_or_insert_with(|| BookmarkThumbnails::new(self.egui_ctx.clone()))
    }

    fn set_status(&mut self, status: impl Into<String>) {
        self.status = status.into();
        self.status_updated_at = Instant::now();
    }

    fn notify(&mut self, message: impl Into<String>) {
        let message = message.into();
        self.status = message.clone();
        self.status_updated_at = Instant::now();
        if self.settings.show_toasts {
            self.toast = message;
            self.toast_updated_at = Instant::now();
        }
    }

    fn persist_reading_position(&mut self) {
        if !self.settings.auto_save_reading_position {
            return;
        }
        self.write_current_book_record();
    }

    /// Writes the current book's record unconditionally. Automatic reading-position
    /// saves go through `persist_reading_position`, which honors the
    /// auto-save-reading-position setting; explicit actions (adding a bookmark) call
    /// this directly so the book is always persisted regardless of that setting.
    fn write_current_book_record(&mut self) {
        let Some(source) = self.source.as_ref() else {
            return;
        };
        let path = self.current_bookmark_path(source.as_ref()).to_path_buf();
        self.store.upsert_book_record(BookRecordInput {
            book_id: source.book_id(),
            title: source.title(),
            last_page: self.current_page,
            last_page_name: self.current_bookmark_page_name(source.as_ref()),
            total_pages: source.page_count(),
            path: &path,
            reading_direction: self.reading_direction,
            fit_mode: self.fit_mode,
            manual_zoom: self.current_bookmark_manual_zoom(),
        });
        self.bookmark_rows.clear();
        self.pending_state_save_at = None;
    }

    fn persist_reading_position_deferred(&mut self) {
        let Some(source) = self.source.as_ref() else {
            return;
        };
        if !self.settings.auto_save_reading_position {
            return;
        }
        let path = self.current_bookmark_path(source.as_ref()).to_path_buf();
        let changed = self.store.upsert_book_record_deferred(BookRecordInput {
            book_id: source.book_id(),
            title: source.title(),
            last_page: self.current_page,
            last_page_name: self.current_bookmark_page_name(source.as_ref()),
            total_pages: source.page_count(),
            path: &path,
            reading_direction: self.reading_direction,
            fit_mode: self.fit_mode,
            manual_zoom: self.current_bookmark_manual_zoom(),
        });
        if changed {
            self.bookmark_rows.clear();
            self.pending_state_save_at = Some(Instant::now() + STATE_SAVE_DEBOUNCE);
            self.egui_ctx.request_repaint_after(STATE_SAVE_DEBOUNCE);
        }
    }

    fn current_bookmark_path<'a>(&'a self, source: &'a dyn BookSource) -> &'a Path {
        if self.open_origin == Some(OpenOrigin::SingleImage) {
            return self
                .opened_path
                .as_deref()
                .unwrap_or_else(|| source.source_path());
        }
        source.source_path()
    }

    fn current_bookmark_page_name<'a>(&self, source: &'a dyn BookSource) -> Option<&'a str> {
        if self.open_origin == Some(OpenOrigin::ZipCbz) && self.settings.remember_archive_page_name
        {
            source.page_name(self.current_page)
        } else {
            None
        }
    }

    fn current_bookmark_manual_zoom(&self) -> Option<f32> {
        (self.settings.remember_zoom_per_book && self.fit_mode == FitMode::Manual)
            .then_some(self.manual_zoom)
    }

    fn flush_deferred_state_save_if_due(&mut self) {
        let Some(save_at) = self.pending_state_save_at else {
            return;
        };
        if Instant::now() >= save_at {
            self.flush_deferred_state_save();
        }
    }

    fn flush_deferred_state_save(&mut self) {
        if self.pending_state_save_at.take().is_some() {
            let _ = self.store.flush();
        }
    }

    fn drain_worker_events(&mut self) {
        let started = Instant::now();
        let mut pending = std::mem::take(&mut self.deferred_worker_events);
        while let Some(event) = self.worker.try_recv() {
            pending.push_back(event);
        }
        if pending.is_empty() {
            return;
        }

        let mut decoded_cache_changed = false;
        let mut remaining = VecDeque::with_capacity(pending.len());
        while let Some(event) = pending.pop_front() {
            if self.worker_event_targets_current_spread(&event) {
                decoded_cache_changed |= self.handle_worker_event(event);
            } else {
                remaining.push_back(event);
            }
        }

        while let Some(event) = remaining.pop_front() {
            if started.elapsed() >= WORKER_EVENT_DRAIN_BUDGET {
                self.deferred_worker_events.push_back(event);
                self.deferred_worker_events.extend(remaining);
                self.egui_ctx
                    .request_repaint_after(TEXTURE_PRESENT_REPAINT_DELAY);
                break;
            }
            decoded_cache_changed |= self.handle_worker_event(event);
        }

        if decoded_cache_changed && !self.original_inspection_cache_cleanup_pending() {
            self.prune_decoded_cache();
        }
    }

    fn worker_event_targets_current_spread(&self, event: &WorkerEvent) -> bool {
        match event {
            WorkerEvent::PageReady {
                book_id,
                page_id,
                decode,
                page,
            } => {
                self.book_id.as_deref() == Some(book_id.as_str())
                    && *decode == self.decode_options()
                    && self.target_is_relevant(page.target_long_edge)
                    && self.event_page_id_in_current_spread(*page_id)
            }
            WorkerEvent::PageFailed {
                book_id,
                page_id,
                target_long_edge,
                decode,
                ..
            } => {
                self.book_id.as_deref() == Some(book_id.as_str())
                    && *decode == self.decode_options()
                    && self.target_is_relevant(*target_long_edge)
                    && self.event_page_id_in_current_spread(*page_id)
            }
        }
    }

    /// True when the page identified by `page_id` still maps to an index in the
    /// current snapshot and that index is part of the visible spread.
    fn event_page_id_in_current_spread(&self, page_id: crate::core::source::PageId) -> bool {
        let Some(source) = self.source.as_ref() else {
            return false;
        };
        let Some(index) = source.page_index_for_id(page_id) else {
            return false;
        };
        self.spread_indices().contains(&index)
    }

    fn handle_worker_event(&mut self, event: WorkerEvent) -> bool {
        let mut decoded_cache_changed = false;
        match event {
            WorkerEvent::PageReady {
                book_id,
                page_id,
                decode,
                page,
            } if self.book_id.as_deref() == Some(book_id.as_str())
                && decode == self.decode_options()
                && self.target_is_relevant(page.target_long_edge) =>
            {
                // Drop events for pages that vanished from the current snapshot
                // mid-flight so orphaned ids never enter the cache.
                let Some(index) = resolve_worker_event_index(self.source.as_deref(), page_id)
                else {
                    return false;
                };
                let key = PageCacheKey {
                    page_id,
                    target_long_edge: page.target_long_edge,
                    decode,
                };
                self.page_errors.remove(&key);
                if let Some(notice) = page.notice.as_ref() {
                    self.set_status(notice.clone());
                }
                self.page_metrics
                    .insert(page_id, PageMetrics::from_page(&page));
                self.insert_prepared_page(key, page.clone());
                decoded_cache_changed = true;
                self.maybe_enqueue_upscale_probe(key, page);
                self.commit_pending_page_turn_if_ready();
                if self.spread_indices().contains(&index) {
                    self.egui_ctx
                        .request_repaint_after(TEXTURE_PRESENT_REPAINT_DELAY);
                }
                #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
                self.record_cache_snapshot("page_ready");
            }
            WorkerEvent::PageFailed {
                book_id,
                page_id,
                target_long_edge,
                decode,
                message,
            } if self.book_id.as_deref() == Some(book_id.as_str())
                && decode == self.decode_options()
                && self.target_is_relevant(target_long_edge) =>
            {
                let Some(index) = resolve_worker_event_index(self.source.as_deref(), page_id)
                else {
                    return false;
                };
                self.page_errors.insert(
                    PageCacheKey {
                        page_id,
                        target_long_edge,
                        decode,
                    },
                    message,
                );
                // A folder page that failed because its file vanished (not a
                // corrupt decode) means the snapshot is stale; rebuild it.
                if self.open_origin == Some(OpenOrigin::Folder)
                    && self
                        .source
                        .as_deref()
                        .is_some_and(|source| refresh::folder_page_file_vanished(source, index))
                {
                    self.request_folder_refresh();
                }
                self.commit_pending_page_turn_if_ready();
            }
            _ => {}
        }
        decoded_cache_changed
    }

    fn clear_debug_compare_requests(&mut self) {
        self.debug_compare_inflight.clear();
    }

    fn target_is_relevant(&self, target_long_edge: u32) -> bool {
        (self.settings.progressive_preview_enabled && target_long_edge == PREVIEW_TARGET_LONG_EDGE)
            || target_long_edge == self.target_long_edge
            || self
                .transition
                .as_ref()
                .is_some_and(|transition| target_long_edge == transition.target_long_edge)
    }

    fn decode_options(&self) -> DecodeOptions {
        let strategy = match self.settings.decode_mode {
            DecodeMode::AutoFast => DecodeStrategy::Auto,
            DecodeMode::Compatibility => DecodeStrategy::ImageCrate,
            DecodeMode::Custom => DecodeStrategy::Auto,
        };
        let decoder_preferences = if matches!(self.settings.decode_mode, DecodeMode::Custom) {
            self.settings.decoder_preferences
        } else {
            DecoderPreferences::default()
        };
        DecodeOptions {
            strategy,
            decoder_preferences,
            fast_sampled_scaled_decode: self.settings.fast_sampled_scaled_decode,
            cpu_upscale_filter: self.settings.cpu_upscale_filter,
            cpu_downscale_filter: self.settings.cpu_downscale_filter,
            allow_display_upscale: self.should_allow_display_upscale(),
            apply_exif_orientation: self.settings.apply_exif_orientation,
            apply_embedded_icc: self.settings.apply_embedded_icc,
        }
    }

    fn should_allow_display_upscale(&self) -> bool {
        should_allow_cpu_display_upscale(
            self.fit_mode,
            self.manual_zoom,
            self.gpu_display_upscale_can_own_upscale(),
            self.settings.cpu_upscale_filter,
        )
    }

    fn gpu_display_upscale_can_own_upscale(&self) -> bool {
        self.active_wgpu_upscale_method() != WgpuUpscaleMethod::None
    }

    fn worker_options(&self) -> WorkerOptions {
        WorkerOptions {
            decode: self.decode_options(),
            target_intent: self.current_prepared_target_intent(),
            prefetch_enabled: self.settings.prefetch_enabled,
            progressive_preview_enabled: self.settings.progressive_preview_enabled,
            cache_bytes: self.worker_cache_budget_bytes(),
            app_cached_pages: self.app_cached_page_keys(),
        }
    }

    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped_files = ctx.input(|input| input.raw.dropped_files.clone());
        for dropped in dropped_files {
            if let Some(path) = dropped.path {
                self.open_path(path);
                break;
            }
        }
    }

    fn handle_keyboard(&mut self, ctx: &egui::Context) {
        if self.pending_delete_dialog.is_some() {
            if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
                self.cancel_delete_confirmation();
            }
            return;
        }
        if self.edge_prompt.is_some() && ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
            self.edge_prompt = None;
            return;
        }
        let actions = ctx.input(|input| collect_keyboard_actions(input, &self.settings));
        for action in actions {
            match action {
                KeyboardAction::Command(command) => self.apply_command(ctx, command),
                KeyboardAction::Release(release) => self.apply_navigation_key_release(release),
            }
        }
    }

    fn apply_navigation_key_release(&mut self, release: NavigationRelease) {
        match release {
            NavigationRelease::PageTurn => self.clear_queued_page_turns(),
            NavigationRelease::SiblingBook => self.clear_queued_sibling_book_turns(),
        }
    }

    fn apply_command(&mut self, ctx: &egui::Context, command: AppCommand) {
        match command {
            AppCommand::OpenFile => self.open_file_dialog(),
            AppCommand::OpenFolder => self.open_folder_dialog(),
            AppCommand::CloseBook => {
                self.close_book("Closed current book.");
            }
            AppCommand::Quit => {
                self.close_requested = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            AppCommand::QuitFromEsc => {
                if self.settings.esc_to_quit {
                    self.close_requested = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                } else {
                    self.notify("ESC exit is disabled in settings.");
                }
            }
            AppCommand::ToggleFullscreen => self.toggle_fullscreen(ctx),
            AppCommand::ToggleMaximized => self.toggle_maximized(ctx),
            AppCommand::Minimize => ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true)),
            AppCommand::OpenSettings => {
                self.settings_open = true;
            }
            AppCommand::OpenAbout => self.open_about_window(),
            AppCommand::ToggleAlwaysOnTop => {
                let mut settings = self.settings.clone();
                settings.always_on_top = !settings.always_on_top;
                self.apply_settings(ctx, settings);
            }
            AppCommand::NextPage => self.next_page(),
            AppCommand::PreviousPage => self.previous_page(),
            AppCommand::MovePages(delta) => self.move_pages(delta),
            AppCommand::ForceMovePages(delta) => self.move_pages(delta),
            AppCommand::Home => self.set_page(0, NavigationDirection::Backward),
            AppCommand::End => {
                if let Some(source) = self.source.as_ref() {
                    self.set_page(
                        source.page_count().saturating_sub(1),
                        NavigationDirection::Forward,
                    );
                }
            }
            AppCommand::RandomForward => self.random_page(NavigationDirection::Forward),
            AppCommand::RandomBackward => self.random_page(NavigationDirection::Backward),
            AppCommand::NextBook => self.open_sibling_book(1),
            AppCommand::PreviousBook => self.open_sibling_book(-1),
            AppCommand::SetFitMode(mode) => self.set_fit_mode(mode),
            AppCommand::SetDouble(direction) => self.set_double_mode(direction),
            AppCommand::ToggleDouble => self.toggle_double_mode(),
            AppCommand::Zoom(factor) => self.adjust_zoom(factor),
            AppCommand::ZoomFine(delta) => self.adjust_zoom_by_delta(delta),
            AppCommand::RotateClockwise => self.update_effects(|effects| {
                effects.transform = effects.transform.rotated_cw();
            }),
            AppCommand::RotateCounterClockwise => self.update_effects(|effects| {
                effects.transform = effects.transform.rotated_ccw();
            }),
            AppCommand::SetRotation(rotation) => self.update_effects(|effects| {
                effects.transform = effects.transform.with_rotation(rotation);
            }),
            AppCommand::ToggleFlipHorizontal => self.update_effects(|effects| {
                effects.transform.flip_horizontal = !effects.transform.flip_horizontal;
            }),
            AppCommand::ToggleFlipVertical => self.update_effects(|effects| {
                effects.transform.flip_vertical = !effects.transform.flip_vertical;
            }),
            AppCommand::ToggleInvert => self.update_effects(|effects| {
                effects.invert_colors = !effects.invert_colors;
            }),
            AppCommand::SetFilter(filter) => self.update_effects(|effects| {
                effects.filter = filter;
            }),
            AppCommand::ToggleGamma => self.update_effects(|effects| {
                effects.gamma = !effects.gamma;
            }),
            AppCommand::Delete(mode) => self.delete_current_file(mode),
            AppCommand::OpenExplorer => self.open_current_in_file_manager(),
            AppCommand::CopyPageImage => self.copy_current_page_image(),
            AppCommand::CopyDisplayImage => self.copy_current_spread_image(),
            AppCommand::CopyPath => self.copy_current_path(),
            AppCommand::ToggleCurrentPageBookmark => self.toggle_current_page_bookmark(),
            AppCommand::ToggleBookmarkPopover => self.toggle_bookmark_popover(ctx),
        }
    }

    fn open_file_dialog(&mut self) {
        if let Some(path) = FileDialog::new()
            .add_filter("Images and comics", OPENABLE_FILE_EXTENSIONS)
            .pick_file()
        {
            self.open_path(path);
        }
    }

    fn open_folder_dialog(&mut self) {
        if let Some(path) = FileDialog::new().pick_folder() {
            self.open_path(path);
        }
    }

    fn close_book(&mut self, status: impl Into<String>) {
        self.flush_deferred_state_save();
        let clear_started = Instant::now();
        let worker_cleared = self.worker.clear_book_blocking();
        perf::record_close_book_worker_clear(clear_started, worker_cleared);
        self.clear_local_book_state(status);
        if !worker_cleared {
            self.set_status("Closed view state; background decode is still finishing briefly.");
        }
    }

    fn clear_local_book_state(&mut self, status: impl Into<String>) {
        self.source = None;
        self.book_id = None;
        self.open_origin = None;
        self.opened_path = None;
        self.current_page = 0;
        self.pan = Vec2::ZERO;
        self.manual_zoom = 1.0;
        self.effects = ViewEffects::default();
        self.current_view_state = None;
        self.edge_prompt = None;
        self.pending_delete_dialog = None;
        self.decoded_pages.clear();
        self.decoded_bytes = 0;
        self.page_metrics.clear();
        self.textures.clear();
        self.clear_debug_compare_requests();
        self.clear_upscale_probe_state();
        if let Some(thumbnails) = self.bookmark_thumbnails.as_mut() {
            thumbnails.clear();
        }
        self.page_errors.clear();
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        {
            self.page_turn_started_at = None;
            self.open_to_first_visible_trace = None;
        }
        self.transition = None;
        self.clear_pending_sibling_book_turns();
        self.sibling_book_visual_pending = false;
        self.sibling_book_wgpu_present_wait = None;
        self.sibling_book_visual_hold_until = None;
        self.clear_adjacent_seed_cache();
        self.set_status(status);
    }

    fn toggle_fullscreen(&mut self, ctx: &egui::Context) {
        self.fullscreen = !self.fullscreen;
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.fullscreen));
    }

    fn toggle_maximized(&mut self, ctx: &egui::Context) {
        self.maximized = !self.maximized;
        ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(self.maximized));
    }

    fn open_current_in_file_manager(&mut self) {
        let Some(target) = self.current_file_manager_target() else {
            self.notify("No current file to reveal.");
            return;
        };
        match platform::reveal_in_file_manager(&target) {
            Ok(()) => {
                self.notify(format!("Opened file location: {}", target.display()));
            }
            Err(error) => {
                self.notify(format!("Could not open file location: {error}"));
            }
        }
    }

    fn current_file_manager_target(&self) -> Option<PathBuf> {
        let source = self.source.as_ref()?;
        match self.open_origin? {
            OpenOrigin::ZipCbz => Some(source.source_path().to_path_buf()),
            OpenOrigin::Folder | OpenOrigin::SingleImage => {
                source.page_file_path(self.current_page)
            }
        }
    }

    fn copy_current_path(&mut self) {
        let Some(source) = self.source.as_ref() else {
            self.notify("No current page path to copy.");
            return;
        };
        let Some(text) = source.page_display_path(self.current_page) else {
            self.notify("No current page path to copy.");
            return;
        };
        match platform::copy_text_to_clipboard(&text) {
            Ok(()) => self.notify("Copied current path."),
            Err(error) => self.notify(format!("Could not copy path: {error}")),
        }
    }

    fn copy_current_page_image(&mut self) {
        let Some(image) = self.effected_page_image(self.current_page, self.target_long_edge) else {
            self.notify("Current page is not ready to copy.");
            return;
        };
        match platform::copy_color_image_to_clipboard(&image) {
            Ok(()) => self.notify("Copied current page image."),
            Err(error) => self.notify(format!("Could not copy image: {error}")),
        }
    }

    fn copy_current_spread_image(&mut self) {
        let indices = self.spread_indices();
        let Some(image) = self.compose_spread_image(&indices, self.target_long_edge) else {
            self.notify("Current spread is not ready to copy.");
            return;
        };
        match platform::copy_color_image_to_clipboard(&image) {
            Ok(()) => self.notify("Copied visible spread image."),
            Err(error) => self.notify(format!("Could not copy spread: {error}")),
        }
    }

    #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
    fn drive_auto_page_turn_diagnostics(&mut self, ctx: &egui::Context) {
        let Some(driver) = self.auto_page_turn_driver.as_mut() else {
            return;
        };
        match driver.update(self.source.is_some(), Instant::now()) {
            perf::AutoPageTurnAction::Wait(delay) => ctx.request_repaint_after(delay),
            perf::AutoPageTurnAction::Turn => {
                self.next_page();
                ctx.request_repaint();
            }
            perf::AutoPageTurnAction::Close => {
                self.auto_page_turn_driver = None;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }
}

impl Drop for SuiSuiViewApp {
    fn drop(&mut self) {
        let shutdown_started = Instant::now();
        let page_worker_stopped = self.worker.request_shutdown();
        let debug_compare_stopped = self
            .debug_compare_worker
            .as_mut()
            .is_none_or(DebugCompareWorker::request_shutdown);
        let thumbnails_stopped = self
            .bookmark_thumbnails
            .as_mut()
            .is_none_or(BookmarkThumbnails::request_shutdown);
        self.flush_deferred_state_save();
        perf::record_app_shutdown(
            shutdown_started,
            self.source.is_some(),
            page_worker_stopped,
            debug_compare_stopped,
            thumbnails_stopped,
        );
        perf::flush();
    }
}

impl SuiSuiViewApp {
    fn drain_ipc_open_requests(&mut self, ctx: &egui::Context) {
        let Some(request) = self.ipc_rx.as_ref().and_then(|rx| rx.try_iter().last()) else {
            return;
        };
        if let Some(path) = request {
            self.open_path(path);
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        ctx.request_repaint();
    }

    fn refresh_single_instance_listener(&mut self) {
        if !self.settings.single_instance {
            self.ipc_rx = None;
            return;
        }
        if self.ipc_rx.is_none() {
            let pipe_name =
                crate::single_instance::pipe_name_for_key(&self.store.path().display().to_string());
            self.ipc_rx = Some(crate::single_instance::start_listener(pipe_name));
        }
    }

    fn settings_is_capturing_keyboard(&self) -> bool {
        self.settings_open && self.shortcut_capture.is_some()
    }

    fn update_window_title(&mut self, ctx: &egui::Context) {
        let Some(source) = self.source.as_ref() else {
            if self
                .window_title
                .as_ref()
                .is_some_and(|title| title.matches("", 0, 0))
            {
                return;
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Title("SuiSuiView".to_owned()));
            self.window_title = Some(WindowTitleSnapshot {
                title: String::new(),
                page: 0,
                total_pages: 0,
            });
            return;
        };

        let title = source.title();
        let page = self.current_page + 1;
        let total_pages = source.page_count();
        if self
            .window_title
            .as_ref()
            .is_some_and(|snapshot| snapshot.matches(title, page, total_pages))
        {
            return;
        };
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!(
            "SuiSuiView - {} [{} / {}]",
            title, page, total_pages
        )));
        self.window_title = Some(WindowTitleSnapshot {
            title: title.to_owned(),
            page,
            total_pages,
        });
    }
}

/// Current index of a worker event's page in `source`, or None when the page
/// vanished from the snapshot mid-flight (the event must then be dropped so an
/// orphaned id never enters the cache).
fn resolve_worker_event_index(
    source: Option<&dyn BookSource>,
    page_id: crate::core::source::PageId,
) -> Option<usize> {
    source?.page_index_for_id(page_id)
}

#[cfg(test)]
mod tests {
    use super::commands::DeleteMode;
    use super::perf::PageCacheState;
    use super::{
        adjacent_sibling_book_paths, adjacent_sibling_book_paths_ordered, apply_effects_to_image,
        best_page_key_excluding_preview_fallback_in_cache, best_page_key_in_cache,
        command_for_shortcut, double_spread_indices, final_quality_page_key_in_cache,
        gpu_visual_needs_wgsl, korean_font_candidates, load_first_existing_font,
        lower_resolution_page_keys, ordered_spread_indices, page_cache_state_from_hit, platform,
        prepared_target_intent_for_view, preview_prefetch_indices, relative_difference,
        sanitize_font_name, should_allow_cpu_display_upscale, sibling_book_path,
        smart_spread_indices_for_metrics, texture_cache_budget_bytes_for,
        touch_normal_navigation_page_keys, transformed_page_size, transition_paint_params,
        transition_screen_sign, worker_center_page_for_mode, AppCommand, ImageFilter, PageCacheKey,
        PageMetrics, TextureCacheKey, TextureSampling, ViewEffects, ViewMode, ViewTransform,
    };
    use crate::core::source::PageId;
    use crate::core::state::{
        AppSettings, CacheMemoryMode, CpuScaleFilter, FitMode, KeyCode, KeyShortcut,
        PageTransitionStyle, ReadingDirection, RendererMode, WgpuDownscaleMethod,
        WgpuUpscaleMethod, AMPLE_TOTAL_BUDGET_BYTES, MANUAL_CACHE_MB_MAX, MANUAL_CACHE_MB_MIN,
        SAVER_TOTAL_BUDGET_BYTES, STANDARD_TOTAL_BUDGET_BYTES,
    };
    use crate::core::worker::{
        DecodeBackend, DecodeOptions, DecodeStrategy, NavigationDirection, PagePixels,
        PreparedPage, MAX_TARGET_LONG_EDGE, PREVIEW_TARGET_LONG_EDGE,
    };
    use egui::{Color32, ColorImage, Pos2, Rect, Vec2};
    use lru::LruCache;
    use std::collections::HashMap;
    use std::fs;
    use std::num::NonZeroUsize;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn transition_screen_sign_follows_reading_direction() {
        assert_eq!(
            transition_screen_sign(ReadingDirection::LeftToRight, NavigationDirection::Forward),
            1.0
        );
        assert_eq!(
            transition_screen_sign(ReadingDirection::LeftToRight, NavigationDirection::Backward),
            -1.0
        );
        assert_eq!(
            transition_screen_sign(ReadingDirection::RightToLeft, NavigationDirection::Forward),
            -1.0
        );
        assert_eq!(
            transition_screen_sign(ReadingDirection::RightToLeft, NavigationDirection::Backward),
            1.0
        );
    }

    #[test]
    fn book_flip_transition_keeps_target_page_moving_in_flow_direction() {
        let viewport = Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 800.0));
        let paint = transition_paint_params(PageTransitionStyle::BookFlip2d, 0.5, 1.0, viewport);

        assert!(paint.from_offset.x > 0.0);
        assert!(paint.to_offset.x < 0.0);
        assert!(paint.from_scale.x < 1.0);
        assert!(paint.to_scale.x < 1.0);
    }

    #[test]
    fn smart_spread_pairs_even_pages_without_cover_assumption() {
        let metrics: HashMap<usize, PageMetrics> = [
            (0, page_metrics(900.0, 1400.0)),
            (1, page_metrics(910.0, 1410.0)),
            (2, page_metrics(900.0, 1400.0)),
            (3, page_metrics(890.0, 1390.0)),
        ]
        .into_iter()
        .collect();
        let at = |index: usize| metrics.get(&index).copied();

        assert_eq!(smart_spread_indices_for_metrics(0, 4, at), vec![0, 1]);
        assert_eq!(smart_spread_indices_for_metrics(1, 4, at), vec![0, 1]);
        assert_eq!(smart_spread_indices_for_metrics(2, 4, at), vec![2, 3]);
    }

    #[test]
    fn smart_spread_solos_wide_tall_and_mismatched_pages() {
        let metrics: HashMap<usize, PageMetrics> = [
            (0, page_metrics(1600.0, 1000.0)),
            (1, page_metrics(900.0, 1400.0)),
            (2, page_metrics(500.0, 1300.0)),
            (3, page_metrics(900.0, 1400.0)),
            (4, page_metrics(900.0, 1400.0)),
            (5, page_metrics(900.0, 1900.0)),
        ]
        .into_iter()
        .collect();
        let at = |index: usize| metrics.get(&index).copied();

        assert_eq!(smart_spread_indices_for_metrics(0, 6, at), vec![0]);
        assert_eq!(smart_spread_indices_for_metrics(1, 6, at), vec![1]);
        assert_eq!(smart_spread_indices_for_metrics(2, 6, at), vec![2]);
        assert_eq!(smart_spread_indices_for_metrics(3, 6, at), vec![3]);
        assert_eq!(smart_spread_indices_for_metrics(4, 6, at), vec![4]);
        assert_eq!(smart_spread_indices_for_metrics(5, 6, at), vec![5]);
    }

    #[test]
    fn smart_spread_falls_back_to_current_page_until_metrics_arrive() {
        let metrics: HashMap<usize, PageMetrics> =
            [(0, page_metrics(900.0, 1400.0))].into_iter().collect();
        let at = |index: usize| metrics.get(&index).copied();

        assert_eq!(smart_spread_indices_for_metrics(0, 2, at), vec![0]);
        assert_eq!(smart_spread_indices_for_metrics(1, 2, at), vec![1]);
    }

    #[test]
    fn smart_spread_direction_changes_display_order_only() {
        let indices = vec![0, 1];

        assert_eq!(
            ordered_spread_indices(
                indices.clone(),
                ViewMode::SmartDoubleLeftToRight,
                ReadingDirection::RightToLeft,
            ),
            vec![0, 1]
        );
        assert_eq!(
            ordered_spread_indices(
                indices,
                ViewMode::SmartDoubleRightToLeft,
                ReadingDirection::LeftToRight,
            ),
            vec![1, 0]
        );
    }

    #[test]
    fn smart_spread_worker_request_starts_from_pair_anchor() {
        assert_eq!(
            worker_center_page_for_mode(3, ViewMode::SmartDoubleLeftToRight),
            2
        );
        assert_eq!(
            worker_center_page_for_mode(3, ViewMode::DoubleLeftToRight),
            3
        );
        assert_eq!(
            worker_center_page_for_mode(0, ViewMode::SmartDoubleRightToLeft),
            0
        );
    }

    fn page_metrics(width: f32, height: f32) -> PageMetrics {
        PageMetrics { width, height }
    }

    #[test]
    fn best_page_key_uses_preview_until_exact_target_arrives() {
        let mut cache = LruCache::new(NonZeroUsize::new(4).unwrap());
        let preview_key = PageCacheKey {
            page_id: PageId(7),
            target_long_edge: PREVIEW_TARGET_LONG_EDGE,
            decode: DecodeOptions::default(),
        };
        let exact_key = PageCacheKey {
            page_id: PageId(7),
            target_long_edge: 4096,
            decode: DecodeOptions::default(),
        };

        cache.put(preview_key, dummy_page(PREVIEW_TARGET_LONG_EDGE));
        assert_eq!(best_page_key_in_cache(&cache, exact_key), Some(preview_key));

        cache.put(exact_key, dummy_page(4096));
        assert_eq!(best_page_key_in_cache(&cache, exact_key), Some(exact_key));
    }

    #[test]
    fn best_page_key_keeps_original_targets_out_of_navigation_fallback() {
        let mut cache = LruCache::new(NonZeroUsize::new(4).unwrap());
        let requested = PageCacheKey {
            page_id: PageId(7),
            target_long_edge: MAX_TARGET_LONG_EDGE,
            decode: DecodeOptions::default(),
        };
        let original = PageCacheKey {
            target_long_edge: MAX_TARGET_LONG_EDGE + 1,
            ..requested
        };

        cache.put(original, dummy_page(MAX_TARGET_LONG_EDGE + 1));

        assert_eq!(best_page_key_in_cache(&cache, requested), None);
        assert_eq!(best_page_key_in_cache(&cache, original), Some(original));
    }

    #[test]
    fn best_page_key_without_preview_uses_smaller_navigation_target_for_resize_recovery() {
        let mut cache = LruCache::new(NonZeroUsize::new(4).unwrap());
        let cached = PageCacheKey {
            page_id: PageId(7),
            target_long_edge: 1536,
            decode: DecodeOptions::default(),
        };
        let requested = PageCacheKey {
            target_long_edge: 2304,
            ..cached
        };

        cache.put(cached, dummy_page(cached.target_long_edge));

        assert_eq!(
            best_page_key_excluding_preview_fallback_in_cache(&cache, requested),
            Some(cached)
        );
    }

    #[test]
    fn best_page_key_without_preview_uses_larger_navigation_target_for_dpi_downshift() {
        let mut cache = LruCache::new(NonZeroUsize::new(4).unwrap());
        let requested = PageCacheKey {
            page_id: PageId(7),
            target_long_edge: 1536,
            decode: DecodeOptions::default(),
        };
        let cached = PageCacheKey {
            target_long_edge: 2304,
            ..requested
        };

        cache.put(cached, dummy_page(cached.target_long_edge));

        assert_eq!(
            best_page_key_excluding_preview_fallback_in_cache(&cache, requested),
            Some(cached)
        );
    }

    #[test]
    fn best_page_key_without_preview_does_not_promote_preview_target() {
        let mut cache = LruCache::new(NonZeroUsize::new(4).unwrap());
        let preview = PageCacheKey {
            page_id: PageId(7),
            target_long_edge: PREVIEW_TARGET_LONG_EDGE,
            decode: DecodeOptions::default(),
        };
        let requested = PageCacheKey {
            target_long_edge: 2048,
            ..preview
        };

        cache.put(preview, dummy_page(preview.target_long_edge));

        assert_eq!(
            best_page_key_excluding_preview_fallback_in_cache(&cache, requested),
            None
        );
    }

    #[test]
    fn best_page_key_without_preview_keeps_original_targets_out_of_navigation_fallback() {
        let mut cache = LruCache::new(NonZeroUsize::new(4).unwrap());
        let requested = PageCacheKey {
            page_id: PageId(7),
            target_long_edge: MAX_TARGET_LONG_EDGE,
            decode: DecodeOptions::default(),
        };
        let original = PageCacheKey {
            target_long_edge: MAX_TARGET_LONG_EDGE + 1,
            ..requested
        };

        cache.put(original, dummy_page(original.target_long_edge));

        assert_eq!(
            best_page_key_excluding_preview_fallback_in_cache(&cache, requested),
            None
        );
    }

    #[test]
    fn final_quality_page_key_rejects_previews_for_navigation_commit() {
        let mut cache = LruCache::new(NonZeroUsize::new(4).unwrap());
        let requested = PageCacheKey {
            page_id: PageId(7),
            target_long_edge: 2048,
            decode: DecodeOptions::default(),
        };
        let preview = PageCacheKey {
            target_long_edge: PREVIEW_TARGET_LONG_EDGE,
            ..requested
        };
        let exact_or_better = PageCacheKey {
            target_long_edge: 4096,
            ..requested
        };

        cache.put(preview, dummy_page(PREVIEW_TARGET_LONG_EDGE));
        assert_eq!(final_quality_page_key_in_cache(&cache, requested), None);

        cache.put(exact_or_better, dummy_page(4096));
        assert_eq!(
            final_quality_page_key_in_cache(&cache, requested),
            Some(exact_or_better)
        );
        assert_eq!(
            best_page_key_in_cache(&cache, requested),
            Some(exact_or_better)
        );
    }

    #[test]
    fn final_quality_page_key_rejects_original_for_navigation_commit() {
        let mut cache = LruCache::new(NonZeroUsize::new(4).unwrap());
        let requested = PageCacheKey {
            page_id: PageId(7),
            target_long_edge: MAX_TARGET_LONG_EDGE,
            decode: DecodeOptions::default(),
        };
        let original = PageCacheKey {
            target_long_edge: MAX_TARGET_LONG_EDGE + 1,
            ..requested
        };

        cache.put(original, dummy_page(MAX_TARGET_LONG_EDGE + 1));

        assert_eq!(final_quality_page_key_in_cache(&cache, requested), None);
        assert_eq!(
            final_quality_page_key_in_cache(&cache, original),
            Some(original)
        );
    }

    #[test]
    fn lower_resolution_page_keys_only_matches_same_page_and_decode() {
        let mut cache = LruCache::new(NonZeroUsize::new(4).unwrap());
        let decode = DecodeOptions::default();
        let inserted = PageCacheKey {
            page_id: PageId(7),
            target_long_edge: 4096,
            decode,
        };
        let preview = PageCacheKey {
            target_long_edge: PREVIEW_TARGET_LONG_EDGE,
            ..inserted
        };
        let other_page = PageCacheKey {
            page_id: PageId(8),
            ..preview
        };
        let other_decode = PageCacheKey {
            decode: DecodeOptions {
                apply_embedded_icc: true,
                ..decode
            },
            ..preview
        };
        let larger = PageCacheKey {
            target_long_edge: 8192,
            ..inserted
        };

        cache.put(preview, dummy_page(PREVIEW_TARGET_LONG_EDGE));
        cache.put(other_page, dummy_page(PREVIEW_TARGET_LONG_EDGE));
        cache.put(other_decode, dummy_page(PREVIEW_TARGET_LONG_EDGE));
        cache.put(larger, dummy_page(8192));

        assert_eq!(lower_resolution_page_keys(&cache, inserted), vec![preview]);
    }

    #[test]
    fn lower_resolution_page_keys_keeps_navigation_keys_for_original_insert() {
        let mut cache = LruCache::new(NonZeroUsize::new(4).unwrap());
        let inserted = PageCacheKey {
            page_id: PageId(7),
            target_long_edge: MAX_TARGET_LONG_EDGE + 2,
            decode: DecodeOptions::default(),
        };
        let preview = PageCacheKey {
            target_long_edge: PREVIEW_TARGET_LONG_EDGE,
            ..inserted
        };
        let navigation = PageCacheKey {
            target_long_edge: MAX_TARGET_LONG_EDGE,
            ..inserted
        };
        let smaller_original = PageCacheKey {
            target_long_edge: MAX_TARGET_LONG_EDGE + 1,
            ..inserted
        };

        cache.put(preview, dummy_page(PREVIEW_TARGET_LONG_EDGE));
        cache.put(navigation, dummy_page(MAX_TARGET_LONG_EDGE));
        cache.put(smaller_original, dummy_page(MAX_TARGET_LONG_EDGE + 1));

        assert_eq!(
            lower_resolution_page_keys(&cache, inserted),
            vec![smaller_original]
        );
    }

    #[test]
    fn original_insert_touches_visible_navigation_key_before_lru_push() {
        let mut cache = LruCache::new(NonZeroUsize::new(4).unwrap());
        let decode = DecodeOptions::default();
        let navigation = PageCacheKey {
            page_id: PageId(7),
            target_long_edge: MAX_TARGET_LONG_EDGE,
            decode,
        };
        let filler_a = PageCacheKey {
            page_id: PageId(8),
            ..navigation
        };
        let filler_b = PageCacheKey {
            page_id: PageId(9),
            ..navigation
        };
        let filler_c = PageCacheKey {
            page_id: PageId(10),
            ..navigation
        };
        let original = PageCacheKey {
            target_long_edge: MAX_TARGET_LONG_EDGE + 1,
            ..navigation
        };

        cache.put(navigation, dummy_page(MAX_TARGET_LONG_EDGE));
        cache.put(filler_a, dummy_page(MAX_TARGET_LONG_EDGE));
        cache.put(filler_b, dummy_page(MAX_TARGET_LONG_EDGE));
        cache.put(filler_c, dummy_page(MAX_TARGET_LONG_EDGE));

        touch_normal_navigation_page_keys(&mut cache, &[navigation.page_id], decode);
        let evicted = cache.push(original, dummy_page(MAX_TARGET_LONG_EDGE + 1));

        assert_eq!(evicted.map(|(key, _page)| key), Some(filler_a));
        assert!(cache.peek(&navigation).is_some());
        assert!(cache.peek(&original).is_some());
    }

    #[test]
    fn page_cache_state_tracks_exact_preview_and_fallback() {
        let requested = PageCacheKey {
            page_id: PageId(3),
            target_long_edge: 2048,
            decode: DecodeOptions::default(),
        };
        let preview = PageCacheKey {
            target_long_edge: PREVIEW_TARGET_LONG_EDGE,
            ..requested
        };
        let fallback = PageCacheKey {
            target_long_edge: 4096,
            ..requested
        };

        assert_eq!(
            page_cache_state_from_hit(Some(requested), requested),
            PageCacheState::DecodedExact
        );
        assert_eq!(
            page_cache_state_from_hit(Some(preview), requested),
            PageCacheState::DecodedPreview
        );
        assert_eq!(
            page_cache_state_from_hit(Some(fallback), requested),
            PageCacheState::DecodedFallback
        );
        assert_eq!(
            page_cache_state_from_hit(None, requested),
            PageCacheState::Miss
        );
    }

    #[test]
    fn texture_cache_key_tracks_effects_without_changing_page_key() {
        let page = PageCacheKey {
            page_id: PageId(1),
            target_long_edge: 2048,
            decode: DecodeOptions::default(),
        };
        let normal = TextureCacheKey {
            page,
            effects: ViewEffects::default(),
            sampling: TextureSampling::Linear,
        };
        let inverted = TextureCacheKey {
            page,
            effects: ViewEffects {
                invert_colors: true,
                ..ViewEffects::default()
            },
            sampling: TextureSampling::Linear,
        };

        assert_ne!(normal, inverted);
        assert_eq!(normal.page, inverted.page);
    }

    #[test]
    fn texture_cache_key_tracks_sampling_without_changing_page_key() {
        let page = PageCacheKey {
            page_id: PageId(1),
            target_long_edge: 4096,
            decode: DecodeOptions::default(),
        };
        let linear = TextureCacheKey {
            page,
            effects: ViewEffects::default(),
            sampling: TextureSampling::Linear,
        };
        let nearest = TextureCacheKey {
            page,
            effects: ViewEffects::default(),
            sampling: TextureSampling::Nearest,
        };

        assert_ne!(linear, nearest);
        assert_eq!(linear.page, nearest.page);
    }

    #[test]
    fn prepared_target_intent_splits_large_fit_from_original_inspection() {
        use crate::core::worker::PreparedTargetIntent;

        assert_eq!(
            prepared_target_intent_for_view(FitMode::FitPage, 1.0, 3840),
            PreparedTargetIntent::NormalNavigation
        );
        assert_eq!(
            prepared_target_intent_for_view(FitMode::FitWidth, 1.0, 5120),
            PreparedTargetIntent::LargeFitDisplay
        );
        assert_eq!(
            prepared_target_intent_for_view(FitMode::Manual, 1.0, 3840),
            PreparedTargetIntent::OriginalInspection
        );
        assert_eq!(
            prepared_target_intent_for_view(FitMode::Original, 1.0, 3840),
            PreparedTargetIntent::OriginalInspection
        );
    }

    #[test]
    fn rotation_swaps_layout_size_for_quarter_turns() {
        assert_eq!(
            transformed_page_size(100.0, 200.0, ViewTransform::default()),
            egui::Vec2::new(100.0, 200.0)
        );
        assert_eq!(
            transformed_page_size(
                100.0,
                200.0,
                ViewTransform {
                    rotation_quadrants: 1,
                    ..ViewTransform::default()
                },
            ),
            egui::Vec2::new(200.0, 100.0)
        );
    }

    #[test]
    fn transforms_apply_rotation_flip_and_invert_in_order() {
        let image = ColorImage::new(
            [2, 1],
            vec![Color32::from_rgb(10, 20, 30), Color32::from_rgb(200, 0, 10)],
        );
        let output = apply_effects_to_image(
            &image,
            ViewEffects {
                transform: ViewTransform {
                    rotation_quadrants: 1,
                    flip_vertical: true,
                    ..ViewTransform::default()
                },
                invert_colors: true,
                ..ViewEffects::default()
            },
        );

        assert_eq!(output.size, [1, 2]);
        assert_eq!(output.pixels[0], Color32::from_rgb(55, 255, 245));
        assert_eq!(output.pixels[1], Color32::from_rgb(245, 235, 225));
    }

    #[test]
    fn shortcut_conflicts_follow_honeyview_defaults() {
        let settings = AppSettings::default();
        assert_eq!(
            command_for_shortcut(KeyShortcut::new(KeyCode::F), &settings),
            Some(AppCommand::OpenFolder)
        );
        assert_eq!(
            command_for_shortcut(KeyShortcut::new(KeyCode::F11), &settings),
            Some(AppCommand::ToggleFullscreen)
        );
        assert_eq!(
            command_for_shortcut(KeyShortcut::alt(KeyCode::Enter), &settings),
            Some(AppCommand::ToggleFullscreen)
        );
        assert_eq!(
            command_for_shortcut(KeyShortcut::new(KeyCode::N), &settings),
            Some(AppCommand::ToggleFullscreen)
        );
        assert_eq!(
            command_for_shortcut(KeyShortcut::ctrl(KeyCode::F), &settings),
            Some(AppCommand::ToggleFlipVertical)
        );
        assert_eq!(
            command_for_shortcut(KeyShortcut::new(KeyCode::F5), &settings),
            Some(AppCommand::OpenSettings)
        );
        assert_eq!(
            command_for_shortcut(KeyShortcut::new(KeyCode::F1), &settings),
            Some(AppCommand::OpenAbout)
        );
        assert_eq!(
            command_for_shortcut(KeyShortcut::ctrl(KeyCode::A), &settings),
            Some(AppCommand::ToggleAlwaysOnTop)
        );
        assert_eq!(
            command_for_shortcut(KeyShortcut::new(KeyCode::B), &settings),
            Some(AppCommand::ToggleCurrentPageBookmark)
        );
        assert_eq!(
            command_for_shortcut(KeyShortcut::ctrl(KeyCode::B), &settings),
            Some(AppCommand::ToggleBookmarkPopover)
        );
    }

    #[test]
    fn shortcut_maps_view_modes_and_delete_modes() {
        let settings = AppSettings::default();
        assert_eq!(
            command_for_shortcut(KeyShortcut::new(KeyCode::Z), &settings),
            Some(AppCommand::SetFitMode(FitMode::FitPage))
        );
        assert_eq!(
            command_for_shortcut(KeyShortcut::new(KeyCode::Num7), &settings),
            Some(AppCommand::SetDouble(ReadingDirection::LeftToRight))
        );
        assert_eq!(
            command_for_shortcut(KeyShortcut::new(KeyCode::Num6), &settings),
            Some(AppCommand::SetDouble(ReadingDirection::RightToLeft))
        );
        assert_eq!(
            command_for_shortcut(KeyShortcut::new(KeyCode::Delete), &settings),
            Some(AppCommand::Delete(DeleteMode::Recycle))
        );
        assert_eq!(
            command_for_shortcut(KeyShortcut::shift(KeyCode::Delete), &settings),
            Some(AppCommand::Delete(DeleteMode::Permanent))
        );
    }

    #[test]
    fn effects_filter_is_part_of_texture_key() {
        let page = PageCacheKey {
            page_id: PageId(0),
            target_long_edge: 1024,
            decode: DecodeOptions::default(),
        };
        let none = TextureCacheKey {
            page,
            effects: ViewEffects::default(),
            sampling: TextureSampling::Linear,
        };
        let filtered = TextureCacheKey {
            page,
            effects: ViewEffects {
                filter: ImageFilter::SmoothSharpen,
                ..ViewEffects::default()
            },
            sampling: TextureSampling::Linear,
        };

        assert_ne!(none, filtered);
    }

    #[test]
    fn texture_cache_budget_keeps_visible_and_transition_pages_bounded() {
        // A generous cap lets the content-derived goal (and its MIN floor) decide.
        let cap = 512 * 1024 * 1024;
        assert_eq!(
            texture_cache_budget_bytes_for(1024, 1, false, cap),
            64 * 1024 * 1024
        );
        assert_eq!(
            texture_cache_budget_bytes_for(4096, 1, false, cap),
            128 * 1024 * 1024
        );
        // Transition doubles the visible-page goal (1 + 1 transition + 1 = 3 pages at 64 MB each).
        assert_eq!(
            texture_cache_budget_bytes_for(4096, 1, true, cap),
            192 * 1024 * 1024
        );
        // A tight total-budget cap dominates the larger content goal.
        assert_eq!(
            texture_cache_budget_bytes_for(8192, 2, true, 80 * 1024 * 1024),
            80 * 1024 * 1024
        );
        // The MIN texture floor always wins, even below a sub-minimum cap.
        assert_eq!(
            texture_cache_budget_bytes_for(1024, 1, false, 16 * 1024 * 1024),
            64 * 1024 * 1024
        );
    }

    #[test]
    fn preview_prefetch_indices_cover_forward_window() {
        let pages = preview_prefetch_indices(0, 20, NavigationDirection::Forward, 1);

        assert_eq!(pages.first(), Some(&0));
        assert!(pages.contains(&19));
    }

    #[test]
    fn page_cache_key_tracks_decode_options() {
        let normal = PageCacheKey {
            page_id: PageId(0),
            target_long_edge: 2048,
            decode: DecodeOptions::default(),
        };
        let icc = PageCacheKey {
            decode: DecodeOptions {
                strategy: DecodeStrategy::Auto,
                apply_exif_orientation: true,
                apply_embedded_icc: true,
                ..DecodeOptions::default()
            },
            ..normal
        };

        assert_ne!(normal, icc);
    }

    #[test]
    fn manual_total_budget_is_clamped() {
        let mut settings = AppSettings {
            cache_memory_mode: CacheMemoryMode::Manual,
            manual_cache_mb: MANUAL_CACHE_MB_MIN - 1,
            ..AppSettings::default()
        };

        assert_eq!(
            super::total_memory_budget_bytes(&settings),
            MANUAL_CACHE_MB_MIN as usize * 1024 * 1024
        );
        settings.manual_cache_mb = MANUAL_CACHE_MB_MAX + 1;
        assert_eq!(
            super::total_memory_budget_bytes(&settings),
            MANUAL_CACHE_MB_MAX as usize * 1024 * 1024
        );
    }

    #[test]
    fn preset_total_budgets_are_fixed_regardless_of_renderer() {
        for renderer_mode in RendererMode::ALL {
            let settings = |mode| AppSettings {
                cache_memory_mode: mode,
                renderer_mode,
                ..AppSettings::default()
            };
            assert_eq!(
                super::total_memory_budget_bytes(&settings(CacheMemoryMode::Saver)),
                SAVER_TOTAL_BUDGET_BYTES
            );
            assert_eq!(
                super::total_memory_budget_bytes(&settings(CacheMemoryMode::Standard)),
                STANDARD_TOTAL_BUDGET_BYTES
            );
            assert_eq!(
                super::total_memory_budget_bytes(&settings(CacheMemoryMode::Ample)),
                AMPLE_TOTAL_BUDGET_BYTES
            );
        }
    }

    #[test]
    fn automatic_total_budget_is_mode_aware_and_bounded() {
        // Glow: 2% of RAM, clamped to [128, 256] MB.
        assert_eq!(
            super::automatic_total_budget_bytes_for(
                RendererMode::LowMemoryGlow,
                8 * 1024 * 1024 * 1024
            ),
            (8 * 1024 * 1024 * 1024) / 50
        );
        assert_eq!(
            super::automatic_total_budget_bytes_for(
                RendererMode::LowMemoryGlow,
                2 * 1024 * 1024 * 1024
            ),
            128 * 1024 * 1024
        );
        assert_eq!(
            super::automatic_total_budget_bytes_for(
                RendererMode::LowMemoryGlow,
                64 * 1024 * 1024 * 1024
            ),
            256 * 1024 * 1024
        );
        // Wgpu: 4% of RAM, clamped to [256, 768] MB.
        assert_eq!(
            super::automatic_total_budget_bytes_for(RendererMode::Wgpu, 8 * 1024 * 1024 * 1024),
            (8 * 1024 * 1024 * 1024) / 25
        );
        assert_eq!(
            super::automatic_total_budget_bytes_for(RendererMode::Wgpu, 2 * 1024 * 1024 * 1024),
            256 * 1024 * 1024
        );
        assert_eq!(
            super::automatic_total_budget_bytes_for(RendererMode::Wgpu, 64 * 1024 * 1024 * 1024),
            768 * 1024 * 1024
        );
    }

    #[test]
    fn pool_derivations_split_total_by_renderer_mode() {
        let base = |renderer_mode| AppSettings {
            cache_memory_mode: CacheMemoryMode::Ample,
            renderer_mode,
            ..AppSettings::default()
        };
        // Ample = 768 MB total, divides by 100 cleanly at the MB scale.
        let total = AMPLE_TOTAL_BUDGET_BYTES;
        let share = |numer: usize| total / 100 * numer;

        let wgpu = base(RendererMode::Wgpu);
        assert_eq!(super::cache_budget_bytes(&wgpu), share(35));
        assert_eq!(super::texture_cache_budget_cap_bytes(&wgpu), share(25));
        assert_eq!(super::gpu_source_texture_budget_bytes(&wgpu), share(25));
        assert_eq!(
            super::gpu_intermediate_texture_budget_bytes(&wgpu),
            share(15)
        );

        // Glow redistributes the unused 40% GPU share into decode + texture.
        let glow = base(RendererMode::LowMemoryGlow);
        assert_eq!(super::cache_budget_bytes(&glow), share(55));
        assert_eq!(super::texture_cache_budget_cap_bytes(&glow), share(45));
    }

    #[test]
    fn pool_derivations_respect_minimum_floors_at_smallest_budget() {
        // 64 MB total (Manual floor) is below every pool's MIN, so the floors dominate.
        let settings = AppSettings {
            cache_memory_mode: CacheMemoryMode::Manual,
            manual_cache_mb: MANUAL_CACHE_MB_MIN,
            renderer_mode: RendererMode::Wgpu,
            ..AppSettings::default()
        };
        assert_eq!(
            super::total_memory_budget_bytes(&settings),
            64 * 1024 * 1024
        );
        // Decode + texture floors are both 64 MB.
        assert_eq!(super::cache_budget_bytes(&settings), 64 * 1024 * 1024);
        assert_eq!(
            super::texture_cache_budget_cap_bytes(&settings),
            64 * 1024 * 1024
        );
        // GPU floors keep the current page + SR round-trip viable.
        assert_eq!(
            super::gpu_source_texture_budget_bytes(&settings),
            64 * 1024 * 1024
        );
        assert_eq!(
            super::gpu_intermediate_texture_budget_bytes(&settings),
            96 * 1024 * 1024
        );
    }

    #[test]
    fn relative_difference_handles_zero_and_ratio_cases() {
        assert_eq!(relative_difference(10.0, 10.0), 0.0);
        assert_eq!(relative_difference(10.0, 5.0), 0.5);
        assert_eq!(relative_difference(0.0, 0.0), 0.0);
    }

    #[test]
    fn double_spread_indices_uses_current_page_then_next() {
        assert_eq!(double_spread_indices(0, 4), vec![0, 1]);
        assert_eq!(double_spread_indices(2, 4), vec![2, 3]);
        assert_eq!(double_spread_indices(3, 4), vec![3]);
        assert!(double_spread_indices(0, 0).is_empty());
    }

    #[test]
    fn gpu_display_upscale_disables_cpu_prepare_upscale() {
        // WGPU mode with a GPU display upscaler owns the enlargement, so the CPU
        // prepare step must not also upscale, regardless of the CPU filter.
        assert!(!should_allow_cpu_display_upscale(
            FitMode::FitPage,
            1.0,
            true,
            CpuScaleFilter::Lanczos3,
        ));
        assert!(!should_allow_cpu_display_upscale(
            FitMode::Manual,
            2.0,
            true,
            CpuScaleFilter::Lanczos3,
        ));
    }

    #[test]
    fn cpu_prepare_upscale_enabled_for_fit_modes_without_gpu_upscaler() {
        // Glow mode (no GPU display upscaler): the user's CPU upscale filter enlarges
        // fit-mode pages during preparation.
        for fit_mode in [FitMode::FitPage, FitMode::FitWidth, FitMode::FitHeight] {
            assert!(should_allow_cpu_display_upscale(
                fit_mode,
                1.0,
                false,
                CpuScaleFilter::Lanczos3,
            ));
        }
        // Bilinear equals the free hardware sampler, so stay native and let the
        // sampler enlarge instead of caching a large upscaled page.
        assert!(!should_allow_cpu_display_upscale(
            FitMode::FitPage,
            1.0,
            false,
            CpuScaleFilter::Bilinear,
        ));
        // Manual zoom and original size never CPU-upscale.
        assert!(!should_allow_cpu_display_upscale(
            FitMode::Manual,
            1.25,
            false,
            CpuScaleFilter::Lanczos3,
        ));
        assert!(!should_allow_cpu_display_upscale(
            FitMode::Original,
            4.0,
            false,
            CpuScaleFilter::Lanczos3,
        ));
    }

    #[test]
    fn auto_gpu_visual_uses_cpu_texture_path_for_downscale_without_effects() {
        assert!(!gpu_visual_needs_wgsl(
            [2000, 3000],
            [1000, 1500],
            ViewEffects::default(),
            WgpuUpscaleMethod::Auto,
            WgpuDownscaleMethod::Bilinear,
            1.10,
        ));
        assert!(gpu_visual_needs_wgsl(
            [2000, 3000],
            [1000, 1500],
            ViewEffects::default(),
            WgpuUpscaleMethod::Auto,
            WgpuDownscaleMethod::Hamming,
            1.10,
        ));
    }

    #[test]
    fn gpu_visual_uses_wgsl_for_auto_upscale_explicit_methods_and_effects() {
        assert!(gpu_visual_needs_wgsl(
            [800, 1200],
            [1600, 2400],
            ViewEffects::default(),
            WgpuUpscaleMethod::Auto,
            WgpuDownscaleMethod::Bilinear,
            1.10,
        ));
        assert!(gpu_visual_needs_wgsl(
            [800, 1200],
            [1600, 2400],
            ViewEffects::default(),
            WgpuUpscaleMethod::WgslNisStyle,
            WgpuDownscaleMethod::Bilinear,
            1.10,
        ));
        assert!(gpu_visual_needs_wgsl(
            [2000, 3000],
            [1000, 1500],
            ViewEffects {
                invert_colors: true,
                ..ViewEffects::default()
            },
            WgpuUpscaleMethod::Auto,
            WgpuDownscaleMethod::Bilinear,
            1.10,
        ));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn explorer_select_args_keep_switch_and_path_separate() {
        let path = Path::new(r"C:\Users\dead4\Pictures\folder with space\image.png");
        let args = platform::windows_explorer_select_arguments(path);

        assert_eq!(args[0], std::ffi::OsString::from("/select,"));
        assert_eq!(args[1], path.as_os_str().to_os_string());
        assert!(!args[0].to_string_lossy().contains("image.png"));
    }

    #[test]
    fn sibling_books_sort_naturally_and_wrap() {
        let dir = temp_test_dir("siblings");
        fs::create_dir_all(dir.join("book-2")).unwrap();
        fs::create_dir_all(dir.join("book-10")).unwrap();
        fs::write(dir.join("book-1.cbz"), b"placeholder").unwrap();

        assert_eq!(
            sibling_book_path(&dir.join("book-1.cbz"), 1),
            Some(dir.join("book-2"))
        );
        assert_eq!(
            sibling_book_path(&dir.join("book-1.cbz"), -1),
            Some(dir.join("book-10"))
        );
    }

    #[test]
    fn sibling_book_path_falls_back_to_sort_position_when_current_is_missing() {
        let dir = temp_test_dir("siblings-missing-current");
        fs::create_dir_all(dir.join("book-2")).unwrap();
        fs::write(dir.join("book-1.cbz"), b"placeholder").unwrap();

        // "missing.cbz" sorts after both siblings, so it anchors past the end:
        // next wraps to the first entry, previous lands on the last.
        assert_eq!(
            sibling_book_path(&dir.join("missing.cbz"), 1),
            Some(dir.join("book-1.cbz"))
        );
        assert_eq!(
            sibling_book_path(&dir.join("missing.cbz"), -1),
            Some(dir.join("book-2"))
        );
    }

    #[test]
    fn adjacent_sibling_books_report_next_then_previous() {
        let dir = temp_test_dir("adjacent-siblings");
        fs::create_dir_all(dir.join("book-2")).unwrap();
        fs::create_dir_all(dir.join("book-10")).unwrap();
        fs::write(dir.join("book-1.cbz"), b"placeholder").unwrap();

        let adjacent = adjacent_sibling_book_paths(&dir.join("book-1.cbz"));

        assert_eq!(adjacent.len(), 2);
        assert_eq!(adjacent[0], (dir.join("book-2"), 1, "next"));
        assert_eq!(adjacent[1], (dir.join("book-10"), -1, "previous"));
    }

    #[test]
    fn adjacent_sibling_books_can_prefer_previous_direction() {
        let dir = temp_test_dir("adjacent-siblings-previous");
        fs::create_dir_all(dir.join("book-2")).unwrap();
        fs::create_dir_all(dir.join("book-10")).unwrap();
        fs::write(dir.join("book-1.cbz"), b"placeholder").unwrap();

        let adjacent = adjacent_sibling_book_paths_ordered(
            &dir.join("book-1.cbz"),
            NavigationDirection::Backward,
        );

        assert_eq!(adjacent.len(), 2);
        assert_eq!(adjacent[0], (dir.join("book-10"), -1, "previous"));
        assert_eq!(adjacent[1], (dir.join("book-2"), 1, "next"));
    }

    #[test]
    fn korean_font_candidates_include_windows_default() {
        assert!(korean_font_candidates()
            .iter()
            .any(|path| path.ends_with("malgun.ttf")));
        assert_eq!(
            sanitize_font_name("C:\\Windows\\Fonts\\malgun.ttf"),
            "C--Windows-Fonts-malgun-ttf"
        );
    }

    #[test]
    fn loads_a_system_korean_font_on_windows() {
        if cfg!(target_os = "windows") {
            let Some((_name, bytes)) = load_first_existing_font(korean_font_candidates()) else {
                panic!("Windows should provide a Korean UI fallback font");
            };
            assert!(!bytes.is_empty());
        }
    }

    #[test]
    fn unmappable_worker_event_index_is_dropped() {
        use super::resolve_worker_event_index;
        use crate::core::source::{BookSource, SourceError};

        // A source whose only page is id 0: an event for a vanished id (5)
        // resolves to None so `handle_worker_event` drops it before caching.
        struct OnePageSource;
        impl BookSource for OnePageSource {
            fn title(&self) -> &str {
                "one"
            }
            fn source_path(&self) -> &Path {
                Path::new("one")
            }
            fn book_id(&self) -> &str {
                "one"
            }
            fn page_count(&self) -> usize {
                1
            }
            fn page_name(&self, _index: usize) -> Option<&str> {
                Some("page.png")
            }
            fn read_page(&self, _index: usize) -> Result<Vec<u8>, SourceError> {
                Ok(Vec::new())
            }
        }

        let source = OnePageSource;
        assert_eq!(
            resolve_worker_event_index(Some(&source), PageId(0)),
            Some(0)
        );
        assert_eq!(resolve_worker_event_index(Some(&source), PageId(5)), None);
        assert_eq!(resolve_worker_event_index(None, PageId(0)), None);
    }

    fn dummy_page(target_long_edge: u32) -> Arc<PreparedPage> {
        Arc::new(PreparedPage {
            pixels: PagePixels::Rgba(Arc::<[u8]>::from([255, 255, 255, 255])),
            original_width: 1,
            original_height: 1,
            display_width: 1,
            display_height: 1,
            byte_size: 4,
            target_long_edge,
            decode_backend: DecodeBackend::ImageCrate,
            notice: None,
        })
    }

    fn temp_test_dir(name: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("suisuiview-app-{name}-{stamp}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }
}
