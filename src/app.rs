use crate::core::auto_kind::AutoKindPrediction;
use crate::core::effects::ViewEffects;
use crate::core::formats::OPENABLE_FILE_EXTENSIONS;
use crate::core::source::{BookSource, SharedSource};
use crate::core::state::{
    AppSettings, BookmarkInput, DecodeMode, DecoderPreferences, FastStartFailureNotice, FitMode,
    ReadingDirection, StateStore, WgpuUpscaleMethod,
};
use crate::core::worker::{
    DecodeOptions, DecodeStrategy, NavigationDirection, PageWorker, PreparedPage, WorkerEvent,
    WorkerOptions, DEFAULT_TARGET_LONG_EDGE, PREVIEW_TARGET_LONG_EDGE,
};
use commands::{collect_keyboard_commands, AppCommand, DeleteMode};
use crossbeam_channel::{unbounded, Receiver, Sender};
use debug_compare::{DebugCompareState, DebugCompareWorker};
use eframe::egui::{self, Color32, Pos2, Rect, RichText, Stroke, Vec2};
use image_info::ImageInfoState;
use lru::LruCache;
use rfd::{FileDialog, MessageButtons, MessageDialog, MessageDialogResult, MessageLevel};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::time::{Duration, Instant};
use ui::{BookmarkFilter, BookmarkRowsCache, BookmarkThumbnails};

mod about;
mod adjacent_seed;
mod auto_kind;
mod cache;
mod commands;
mod context_menu;
mod debug_compare;
mod eframe_host;
pub(crate) mod fast_start;
#[cfg(target_os = "windows")]
mod file_associations;
mod gpu_paint;
#[cfg(feature = "wgpu-fast-start")]
pub(crate) mod handoff_preview;
mod image_header;
mod image_info;
mod navigation;
mod opening;
mod perf;
mod platform;
mod realtime_sr;
mod runtime;
mod settings;
mod settings_bookmarks;
mod settings_input;
mod settings_performance;
mod sibling_books;
mod texture_prewarm;
mod ui;
mod update_loop;
mod viewer;
mod window;

#[cfg(test)]
use crate::core::effects::{apply_effects_to_image, transformed_page_size};
#[cfg(test)]
use crate::core::worker::preview_prefetch_indices;
pub(in crate::app) use adjacent_seed::{AdjacentSeedCache, AdjacentSeedEvent, SeededPreparedPage};
#[cfg(test)]
use cache::{
    automatic_cache_budget_bytes_for_total, best_page_key_excluding_preview_fallback_in_cache,
    best_page_key_in_cache, final_quality_page_key_in_cache, lower_resolution_page_keys,
    page_cache_state_from_hit, prepared_target_intent_for_view, texture_cache_budget_bytes_for,
    touch_normal_navigation_page_keys,
};
pub(in crate::app) use cache::{
    cache_budget_bytes, gpu_visual_needs_wgsl, rect_target_size, should_allow_cpu_display_upscale,
    PageCacheKey, TextureCacheKey, TextureEntry, TextureSampling, BYTES_PER_RGBA_PIXEL,
};
pub(in crate::app) use navigation::EdgePrompt;
pub(crate) use opening::{start_startup_open_loader, StartupOpen};
pub(in crate::app) use opening::{LoaderEvent, OpenOrigin};
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
    Transition, ViewMode,
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
    fast_start_failure_notice: Option<FastStartFailureNotice>,
    shortcut_capture: Option<settings_input::ShortcutCapture>,
    shortcut_conflict: Option<settings_input::ShortcutConflict>,
    shortcut_expanded_groups: HashSet<&'static str>,
    about_open: bool,
    about_section: about::AboutSection,
    image_info: ImageInfoState,
    worker: PageWorker,
    loader_tx: Sender<LoaderEvent>,
    loader_rx: Receiver<LoaderEvent>,
    loader_pending: bool,
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
    pan: Vec2,
    decoded_pages: LruCache<PageCacheKey, Arc<PreparedPage>>,
    decoded_bytes: usize,
    page_metrics: HashMap<usize, PageMetrics>,
    page_errors: HashMap<PageCacheKey, String>,
    textures: LruCache<TextureCacheKey, TextureEntry>,
    debug_compare: DebugCompareState,
    debug_compare_worker: Option<DebugCompareWorker>,
    debug_compare_inflight: HashSet<PageCacheKey>,
    auto_kind_worker: Option<auto_kind::AutoKindWorker>,
    auto_kind_generation: u64,
    auto_kind_hints: HashMap<PageCacheKey, AutoKindPrediction>,
    auto_kind_inflight: HashSet<PageCacheKey>,
    bookmark_thumbnails: Option<BookmarkThumbnails>,
    gpu_effects_available: bool,
    gpu_target_format: Option<wgpu::TextureFormat>,
    last_nav_direction: NavigationDirection,
    transition: Option<Transition>,
    pending_page_turn: Option<PendingPageTurn>,
    queued_page_turns: Option<QueuedPageTurns>,
    page_turn_paint_hold: bool,
    pending_target_long_edge_increase: Option<(u32, Instant)>,
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
        let screen_renderer = runtime.screen_renderer();
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
            fast_start_failure_notice,
            shortcut_capture: None,
            shortcut_conflict: None,
            shortcut_expanded_groups: HashSet::new(),
            about_open: false,
            about_section: about::AboutSection::default(),
            image_info: ImageInfoState::new(),
            worker: PageWorker::new(egui_ctx.clone()),
            loader_tx,
            loader_rx,
            loader_pending,
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
            pan: Vec2::ZERO,
            decoded_pages: LruCache::new(NonZeroUsize::new(64).unwrap()),
            decoded_bytes: 0,
            page_metrics: HashMap::new(),
            page_errors: HashMap::new(),
            textures: LruCache::new(NonZeroUsize::new(12).unwrap()),
            debug_compare: DebugCompareState::default(),
            debug_compare_worker: None,
            debug_compare_inflight: HashSet::new(),
            auto_kind_worker: None,
            auto_kind_generation: 0,
            auto_kind_hints: HashMap::new(),
            auto_kind_inflight: HashSet::new(),
            bookmark_thumbnails: None,
            gpu_effects_available: screen_renderer.supports_wgsl_paint(),
            gpu_target_format: screen_renderer.wgpu_target_format(),
            last_nav_direction: NavigationDirection::Forward,
            transition: None,
            pending_page_turn: None,
            queued_page_turns: None,
            page_turn_paint_hold: false,
            pending_target_long_edge_increase: None,
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

    fn ensure_auto_kind_worker(&mut self) -> &auto_kind::AutoKindWorker {
        let generation = self.auto_kind_generation;
        let worker = self
            .auto_kind_worker
            .get_or_insert_with(|| auto_kind::AutoKindWorker::new(self.egui_ctx.clone()));
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

    fn persist_current_bookmark(&mut self) {
        let Some(source) = self.source.as_ref() else {
            return;
        };
        let path = self.current_bookmark_path(source.as_ref()).to_path_buf();
        if self.settings.share_state_between_instances {
            self.store.reload_books_from_disk();
        }
        self.store.upsert_bookmark(BookmarkInput {
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
        self.store
            .prune_auto_bookmarks(self.settings.max_remembered_books);
        self.bookmark_rows.clear();
        self.pending_state_save_at = None;
    }

    fn persist_current_bookmark_deferred(&mut self) {
        let Some(source) = self.source.as_ref() else {
            return;
        };
        if !self.settings.auto_save_reading_position {
            return;
        }
        let path = self.current_bookmark_path(source.as_ref()).to_path_buf();
        if self.settings.share_state_between_instances {
            self.store.reload_books_from_disk();
        }
        let changed = self.store.upsert_bookmark_deferred(BookmarkInput {
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
            self.store
                .prune_auto_bookmarks(self.settings.max_remembered_books);
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
            let _ = self.store.save();
        }
    }

    fn drain_worker_events(&mut self) {
        while let Some(event) = self.worker.try_recv() {
            match event {
                WorkerEvent::PageReady {
                    book_id,
                    index,
                    decode,
                    page,
                } if self.book_id.as_deref() == Some(book_id.as_str())
                    && decode == self.decode_options()
                    && self.target_is_relevant(page.target_long_edge) =>
                {
                    let key = PageCacheKey {
                        index,
                        target_long_edge: page.target_long_edge,
                        decode,
                    };
                    self.page_errors.remove(&key);
                    if let Some(notice) = page.notice.as_ref() {
                        self.set_status(notice.clone());
                    }
                    self.page_metrics
                        .insert(index, PageMetrics::from_page(&page));
                    self.insert_prepared_page(key, page.clone());
                    self.maybe_enqueue_auto_kind(key, page);
                    if !self.original_inspection_cache_cleanup_pending() {
                        self.prune_decoded_cache();
                    }
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
                    index,
                    target_long_edge,
                    decode,
                    message,
                } if self.book_id.as_deref() == Some(book_id.as_str())
                    && decode == self.decode_options()
                    && self.target_is_relevant(target_long_edge) =>
                {
                    self.page_errors.insert(
                        PageCacheKey {
                            index,
                            target_long_edge,
                            decode,
                        },
                        message,
                    );
                    self.commit_pending_page_turn_if_ready();
                }
                _ => {}
            }
        }
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
        if self.edge_prompt.is_some() && ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
            self.edge_prompt = None;
            return;
        }
        let commands = ctx.input(|input| collect_keyboard_commands(input, &self.settings));
        for command in commands {
            self.apply_command(ctx, command);
        }
    }

    fn apply_command(&mut self, ctx: &egui::Context, command: AppCommand) {
        match command {
            AppCommand::OpenFile => self.open_file_dialog(),
            AppCommand::OpenFolder => self.open_folder_dialog(),
            AppCommand::CloseBook => {
                self.close_book("Closed current book.");
            }
            AppCommand::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            AppCommand::QuitFromEsc => {
                if self.settings.esc_to_quit {
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
        self.decoded_pages.clear();
        self.decoded_bytes = 0;
        self.page_metrics.clear();
        self.textures.clear();
        self.clear_debug_compare_requests();
        self.clear_auto_kind_state();
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

    fn delete_current_file(&mut self, mode: DeleteMode) {
        let Some(target) = self.current_delete_target() else {
            self.notify("No current file to delete.");
            return;
        };

        let title = match mode {
            DeleteMode::Recycle => "Move file to Recycle Bin?",
            DeleteMode::Permanent => "Permanently delete file?",
        };
        let description = match mode {
            DeleteMode::Recycle => format!("{}", target.display()),
            DeleteMode::Permanent => format!("This cannot be undone.\n\n{}", target.display()),
        };
        let level = match mode {
            DeleteMode::Recycle => MessageLevel::Warning,
            DeleteMode::Permanent => MessageLevel::Error,
        };
        let confirmed = mode == DeleteMode::Recycle && !self.settings.confirm_delete
            || MessageDialog::new()
                .set_level(level)
                .set_title(title)
                .set_description(description)
                .set_buttons(MessageButtons::YesNo)
                .show()
                == MessageDialogResult::Yes;

        if !confirmed {
            self.set_status("Delete cancelled.");
            return;
        }

        if !self.worker.clear_book_blocking() {
            self.notify(
                "Background decode is still finishing; deletion was not attempted. Try again soon.",
            );
            return;
        }
        self.clear_local_book_state("Closing current book before deleting file...");
        let result = match mode {
            DeleteMode::Recycle => trash::delete(&target).map_err(|error| error.to_string()),
            DeleteMode::Permanent => fs::remove_file(&target).map_err(|error| error.to_string()),
        };

        self.notify(match result {
            Ok(()) => match mode {
                DeleteMode::Recycle => format!("Moved to Recycle Bin: {}", target.display()),
                DeleteMode::Permanent => format!("Permanently deleted: {}", target.display()),
            },
            Err(error) => format!("Could not delete {}: {error}", target.display()),
        });
    }

    fn current_delete_target(&self) -> Option<PathBuf> {
        let source = self.source.as_ref()?;
        delete_target_for(self.open_origin?, source.as_ref(), self.current_page)
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
            .map_or(true, DebugCompareWorker::request_shutdown);
        let thumbnails_stopped = self
            .bookmark_thumbnails
            .as_mut()
            .map_or(true, BookmarkThumbnails::request_shutdown);
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

fn edge_prompt_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new(
            RichText::new(label)
                .size(16.0)
                .color(ui::theme::TEXT_PRIMARY)
                .strong(),
        )
        .min_size(Vec2::new(220.0, 34.0))
        .fill(Color32::from_rgb(5, 6, 8))
        .stroke(Stroke::new(1.0, Color32::from_rgb(52, 55, 60))),
    )
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

fn delete_target_for(
    origin: OpenOrigin,
    source: &dyn BookSource,
    current_page: usize,
) -> Option<PathBuf> {
    match origin {
        OpenOrigin::ZipCbz => Some(source.source_path().to_path_buf()),
        OpenOrigin::Folder | OpenOrigin::SingleImage => source.page_file_path(current_page),
    }
}

#[cfg(test)]
mod tests {
    use super::perf::PageCacheState;
    use super::{
        adjacent_sibling_book_paths, adjacent_sibling_book_paths_ordered, apply_effects_to_image,
        best_page_key_excluding_preview_fallback_in_cache, best_page_key_in_cache,
        command_for_shortcut, delete_target_for, double_spread_indices,
        final_quality_page_key_in_cache, gpu_visual_needs_wgsl, korean_font_candidates,
        load_first_existing_font, lower_resolution_page_keys, ordered_spread_indices,
        page_cache_state_from_hit, platform, prepared_target_intent_for_view,
        preview_prefetch_indices, relative_difference, sanitize_font_name,
        should_allow_cpu_display_upscale, sibling_book_path, smart_spread_indices_for_metrics,
        texture_cache_budget_bytes_for, touch_normal_navigation_page_keys, transformed_page_size,
        transition_paint_params, transition_screen_sign, worker_center_page_for_mode, AppCommand,
        DeleteMode, ImageFilter, OpenOrigin, PageCacheKey, PageMetrics, TextureCacheKey,
        TextureSampling, ViewEffects, ViewMode, ViewTransform,
    };
    use crate::core::source::{BookSource, SourceError};
    use crate::core::state::{
        AppSettings, CacheMemoryMode, FitMode, KeyCode, KeyShortcut, PageTransitionStyle,
        ReadingDirection, WgpuDownscaleMethod, WgpuUpscaleMethod, MANUAL_CACHE_MB_MAX,
        MANUAL_CACHE_MB_MIN,
    };
    use crate::core::worker::{
        DecodeBackend, DecodeOptions, DecodeStrategy, NavigationDirection, PreparedPage,
        MAX_TARGET_LONG_EDGE, PREVIEW_TARGET_LONG_EDGE,
    };
    use eframe::egui::{Color32, ColorImage, Pos2, Rect, Vec2};
    use lru::LruCache;
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
        let metrics = [
            (0, page_metrics(900.0, 1400.0)),
            (1, page_metrics(910.0, 1410.0)),
            (2, page_metrics(900.0, 1400.0)),
            (3, page_metrics(890.0, 1390.0)),
        ]
        .into_iter()
        .collect();

        assert_eq!(smart_spread_indices_for_metrics(0, 4, &metrics), vec![0, 1]);
        assert_eq!(smart_spread_indices_for_metrics(1, 4, &metrics), vec![0, 1]);
        assert_eq!(smart_spread_indices_for_metrics(2, 4, &metrics), vec![2, 3]);
    }

    #[test]
    fn smart_spread_solos_wide_tall_and_mismatched_pages() {
        let metrics = [
            (0, page_metrics(1600.0, 1000.0)),
            (1, page_metrics(900.0, 1400.0)),
            (2, page_metrics(500.0, 1300.0)),
            (3, page_metrics(900.0, 1400.0)),
            (4, page_metrics(900.0, 1400.0)),
            (5, page_metrics(900.0, 1900.0)),
        ]
        .into_iter()
        .collect();

        assert_eq!(smart_spread_indices_for_metrics(0, 6, &metrics), vec![0]);
        assert_eq!(smart_spread_indices_for_metrics(1, 6, &metrics), vec![1]);
        assert_eq!(smart_spread_indices_for_metrics(2, 6, &metrics), vec![2]);
        assert_eq!(smart_spread_indices_for_metrics(3, 6, &metrics), vec![3]);
        assert_eq!(smart_spread_indices_for_metrics(4, 6, &metrics), vec![4]);
        assert_eq!(smart_spread_indices_for_metrics(5, 6, &metrics), vec![5]);
    }

    #[test]
    fn smart_spread_falls_back_to_current_page_until_metrics_arrive() {
        let metrics = [(0, page_metrics(900.0, 1400.0))].into_iter().collect();

        assert_eq!(smart_spread_indices_for_metrics(0, 2, &metrics), vec![0]);
        assert_eq!(smart_spread_indices_for_metrics(1, 2, &metrics), vec![1]);
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
            index: 7,
            target_long_edge: PREVIEW_TARGET_LONG_EDGE,
            decode: DecodeOptions::default(),
        };
        let exact_key = PageCacheKey {
            index: 7,
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
            index: 7,
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
            index: 7,
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
            index: 7,
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
            index: 7,
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
            index: 7,
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
            index: 7,
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
            index: 7,
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
            index: 7,
            target_long_edge: 4096,
            decode,
        };
        let preview = PageCacheKey {
            target_long_edge: PREVIEW_TARGET_LONG_EDGE,
            ..inserted
        };
        let other_page = PageCacheKey {
            index: 8,
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
            index: 7,
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
            index: 7,
            target_long_edge: MAX_TARGET_LONG_EDGE,
            decode,
        };
        let filler_a = PageCacheKey {
            index: 8,
            ..navigation
        };
        let filler_b = PageCacheKey {
            index: 9,
            ..navigation
        };
        let filler_c = PageCacheKey {
            index: 10,
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

        touch_normal_navigation_page_keys(&mut cache, &[navigation.index], decode);
        let evicted = cache.push(original, dummy_page(MAX_TARGET_LONG_EDGE + 1));

        assert_eq!(evicted.map(|(key, _page)| key), Some(filler_a));
        assert!(cache.peek(&navigation).is_some());
        assert!(cache.peek(&original).is_some());
    }

    #[test]
    fn page_cache_state_tracks_exact_preview_and_fallback() {
        let requested = PageCacheKey {
            index: 3,
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
            index: 1,
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
            index: 1,
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
            eframe::egui::Vec2::new(100.0, 200.0)
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
            eframe::egui::Vec2::new(200.0, 100.0)
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
            index: 0,
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
        assert_eq!(
            texture_cache_budget_bytes_for(1024, 1, false),
            64 * 1024 * 1024
        );
        assert_eq!(
            texture_cache_budget_bytes_for(4096, 1, false),
            128 * 1024 * 1024
        );
        assert_eq!(
            texture_cache_budget_bytes_for(4096, 1, true),
            128 * 1024 * 1024
        );
        assert_eq!(
            texture_cache_budget_bytes_for(8192, 2, true),
            128 * 1024 * 1024
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
            index: 0,
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
    fn manual_cache_budget_is_clamped() {
        let mut settings = AppSettings {
            cache_memory_mode: CacheMemoryMode::Manual,
            manual_cache_mb: MANUAL_CACHE_MB_MIN - 1,
            ..AppSettings::default()
        };

        assert_eq!(
            super::cache_budget_bytes(&settings),
            MANUAL_CACHE_MB_MIN as usize * 1024 * 1024
        );
        settings.manual_cache_mb = MANUAL_CACHE_MB_MAX + 1;
        assert_eq!(
            super::cache_budget_bytes(&settings),
            MANUAL_CACHE_MB_MAX as usize * 1024 * 1024
        );
    }

    #[test]
    fn automatic_cache_budget_is_bounded_for_viewer_responsiveness() {
        assert_eq!(
            super::automatic_cache_budget_bytes_for_total(8 * 1024 * 1024 * 1024),
            (8 * 1024 * 1024 * 1024) / 100
        );
        assert_eq!(
            super::automatic_cache_budget_bytes_for_total(64 * 1024 * 1024 * 1024),
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
        assert!(!should_allow_cpu_display_upscale(
            FitMode::FitPage,
            1.0,
            true
        ));
        assert!(!should_allow_cpu_display_upscale(
            FitMode::Manual,
            2.0,
            true
        ));
    }

    #[test]
    fn cpu_prepare_upscale_stays_disabled_for_display_fit_modes() {
        assert!(!should_allow_cpu_display_upscale(
            FitMode::FitPage,
            1.0,
            false
        ));
        assert!(!should_allow_cpu_display_upscale(
            FitMode::FitWidth,
            1.0,
            false
        ));
        assert!(!should_allow_cpu_display_upscale(
            FitMode::FitHeight,
            1.0,
            false
        ));
        assert!(!should_allow_cpu_display_upscale(
            FitMode::Manual,
            1.25,
            false
        ));
        assert!(!should_allow_cpu_display_upscale(
            FitMode::Original,
            4.0,
            false
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
        ));
        assert!(gpu_visual_needs_wgsl(
            [2000, 3000],
            [1000, 1500],
            ViewEffects::default(),
            WgpuUpscaleMethod::Auto,
            WgpuDownscaleMethod::Hamming,
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
        ));
        assert!(gpu_visual_needs_wgsl(
            [800, 1200],
            [1600, 2400],
            ViewEffects::default(),
            WgpuUpscaleMethod::WgslNisStyle,
            WgpuDownscaleMethod::Bilinear,
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
        ));
    }

    #[test]
    fn delete_target_uses_archive_for_zip_and_page_file_for_folders() {
        let archive = PathBuf::from("book.cbz");
        let page = PathBuf::from("page-001.jpg");
        let source = FakeSource {
            source_path: archive.clone(),
            page_file: Some(page.clone()),
        };

        assert_eq!(
            delete_target_for(OpenOrigin::ZipCbz, &source, 0),
            Some(archive)
        );
        assert_eq!(
            delete_target_for(OpenOrigin::Folder, &source, 0),
            Some(page.clone())
        );
        assert_eq!(
            delete_target_for(OpenOrigin::SingleImage, &source, 0),
            Some(page)
        );
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

    fn dummy_page(target_long_edge: u32) -> Arc<PreparedPage> {
        Arc::new(PreparedPage {
            rgba: Arc::<[u8]>::from([255, 255, 255, 255]),
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

    struct FakeSource {
        source_path: PathBuf,
        page_file: Option<PathBuf>,
    }

    impl BookSource for FakeSource {
        fn title(&self) -> &str {
            "fake"
        }

        fn source_path(&self) -> &Path {
            &self.source_path
        }

        fn book_id(&self) -> &str {
            "fake"
        }

        fn page_count(&self) -> usize {
            1
        }

        fn page_name(&self, _index: usize) -> Option<&str> {
            Some("page-001.jpg")
        }

        fn page_file_path(&self, _index: usize) -> Option<PathBuf> {
            self.page_file.clone()
        }

        fn read_page(&self, _index: usize) -> Result<Vec<u8>, SourceError> {
            Ok(Vec::new())
        }
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
