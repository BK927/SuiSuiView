use crate::core::effects::{
    apply_effects_to_image, compose_images_horizontally, transform_status_suffix,
    transformed_page_size, ImageFilter, ViewEffects,
};
use crate::core::formats::{unsupported_message_for_extension, OPENABLE_FILE_EXTENSIONS};
use crate::core::natural::cmp_natural;
use crate::core::source::{
    classify_path, open_source_from_path, BookSource, SharedSource, SourceKind,
};
use crate::core::state::{
    AiUpscaleBackend, AiUpscalePrefetchMode, AppSettings, BookmarkInput, CacheMemoryMode,
    DecodeMode, DisplayUpscaler, EdgePageAction, FitMode, LargeImageAnchor, ReadingDirection,
    StateStore, WheelMode,
};
use crate::core::upscale::{AiUpscaleWorker, UpscaleEvent, UpscaleRequest};
use crate::core::worker::{
    clamp_target_long_edge, DecodeOptions, DecodeStrategy, NavigationDirection, PageWorker,
    PreparedPage, WorkerEvent, WorkerOptions, DEFAULT_TARGET_LONG_EDGE, MAX_TARGET_LONG_EDGE,
    MIN_TARGET_LONG_EDGE, PREVIEW_TARGET_LONG_EDGE,
};
use arboard::{Clipboard, ImageData as ClipboardImageData};
use commands::{collect_keyboard_commands, AppCommand, DeleteMode};
use crossbeam_channel::{unbounded, Receiver, Sender};
use debug_compare::{DebugCompareState, DebugCompareWorker};
use eframe::egui::{
    self, Align2, Color32, ColorImage, FontId, ImageData, Pos2, Rect, Sense, Stroke, StrokeKind,
    TextureHandle, TextureOptions, Vec2,
};
use gpu_paint::{GpuPaintRequest, GpuPaintSourceKey};
use image_info::ImageInfoState;
use lru::LruCache;
use rfd::{FileDialog, MessageButtons, MessageDialog, MessageDialogResult, MessageLevel};
use std::borrow::Cow;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use ui::{BookmarkFilter, BookmarkRowsCache, BookmarkThumbnails};

mod about;
mod commands;
mod debug_compare;
mod gpu_paint;
mod image_info;
mod perf;
mod platform;
mod settings;
mod ui;
mod window;

#[cfg(test)]
use crate::core::effects::ViewTransform;
#[cfg(test)]
use commands::{command_for_shortcut, Shortcut, ShortcutKey};
#[cfg(test)]
use platform::{korean_font_candidates, load_first_existing_font, sanitize_font_name};
const TRANSITION_MS: f32 = 120.0;
const SPREAD_GAP_POINTS: f32 = 14.0;
const TARGET_EDGE_HYSTERESIS: u32 = 512;
const STATE_SAVE_DEBOUNCE: Duration = Duration::from_millis(750);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ViewMode {
    Single,
    Double,
}

impl ViewMode {
    fn step(self) -> usize {
        match self {
            Self::Single => 1,
            Self::Double => 2,
        }
    }
}

struct TextureEntry {
    texture: TextureHandle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct PageCacheKey {
    index: usize,
    target_long_edge: u32,
    decode: DecodeOptions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct TextureCacheKey {
    page: PageCacheKey,
    effects: ViewEffects,
    upscaled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpenOrigin {
    Folder,
    ZipCbz,
    SingleImage,
}

impl OpenOrigin {
    fn perf_label(self) -> &'static str {
        match self {
            Self::Folder => "folder",
            Self::ZipCbz => "zip_cbz",
            Self::SingleImage => "single_image",
        }
    }
}

struct Transition {
    from_indices: Vec<usize>,
    target_long_edge: u32,
    started_at: Instant,
    direction: NavigationDirection,
}

struct SpreadPaint<'a> {
    viewport: Rect,
    indices: &'a [usize],
    target_long_edge: u32,
    offset: Vec2,
    alpha: f32,
}

enum PageVisual {
    Ready {
        texture: TextureHandle,
        size: Vec2,
    },
    ReadyGpu {
        source_key: GpuPaintSourceKey,
        image: Arc<ColorImage>,
        size: Vec2,
        effects: ViewEffects,
        display_upscaler: DisplayUpscaler,
    },
    Loading {
        index: usize,
    },
    Failed {
        index: usize,
        message: String,
    },
}

struct LoaderEvent {
    generation: u64,
    path: PathBuf,
    origin: OpenOrigin,
    result: Result<(SharedSource, Option<usize>), String>,
}

#[derive(Debug, Clone)]
struct PendingBookmarkJump {
    book_id: String,
    path: PathBuf,
    page: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BookmarkMarqueeKey {
    book_id: String,
    page: usize,
}

#[derive(Debug, Default)]
struct BookmarkMarqueeState {
    key: Option<BookmarkMarqueeKey>,
    started_at: f64,
}

pub struct SuiSuiViewApp {
    egui_ctx: egui::Context,
    store: StateStore,
    settings: AppSettings,
    settings_open: bool,
    settings_section: settings::SettingsSection,
    about_open: bool,
    about_section: about::AboutSection,
    image_info: ImageInfoState,
    worker: PageWorker,
    loader_tx: Sender<LoaderEvent>,
    loader_rx: Receiver<LoaderEvent>,
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
    pan: Vec2,
    decoded_pages: LruCache<PageCacheKey, Arc<PreparedPage>>,
    decoded_bytes: usize,
    upscaled_pages: LruCache<PageCacheKey, Arc<PreparedPage>>,
    upscaled_bytes: usize,
    use_ai_upscaled_pages: bool,
    page_errors: HashMap<usize, String>,
    textures: LruCache<TextureCacheKey, TextureEntry>,
    debug_compare: DebugCompareState,
    debug_compare_worker: DebugCompareWorker,
    debug_compare_inflight: HashSet<PageCacheKey>,
    bookmark_thumbnails: BookmarkThumbnails,
    gpu_effects_available: bool,
    gpu_target_format: Option<wgpu::TextureFormat>,
    upscale_worker: AiUpscaleWorker,
    upscale_generation: u64,
    upscale_inflight: Option<(u64, usize)>,
    ai_upscale_queue: VecDeque<usize>,
    ai_upscale_failures: HashSet<PageCacheKey>,
    last_nav_direction: NavigationDirection,
    transition_effect: bool,
    transition: Option<Transition>,
    fullscreen: bool,
    maximized: bool,
    window_position_checked: bool,
    status: String,
    status_updated_at: Instant,
    pending_state_save_at: Option<Instant>,
    #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
    page_turn_started_at: Option<(usize, Instant)>,
    #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
    open_to_first_visible_trace: Option<perf::OpenToFirstVisibleTrace>,
    bookmark_popover_open: bool,
    bookmark_popover_pos: Pos2,
    bookmark_popover_anchor: Option<Rect>,
    bookmark_filter: BookmarkFilter,
    bookmark_search: String,
    bookmark_clear_confirming: bool,
    bookmark_rows: BookmarkRowsCache,
    bookmark_marquee: BookmarkMarqueeState,
    pending_bookmark_jump: Option<PendingBookmarkJump>,
}

impl SuiSuiViewApp {
    pub fn new(cc: &eframe::CreationContext<'_>, store: StateStore) -> Self {
        let app_started = Instant::now();
        platform::install_app_fonts(&cc.egui_ctx);
        ui::apply_app_theme(&cc.egui_ctx);
        let (loader_tx, loader_rx) = unbounded();
        let settings = store.settings().clone();
        apply_window_level(&cc.egui_ctx, settings.always_on_top);
        let app = Self {
            egui_ctx: cc.egui_ctx.clone(),
            store,
            settings: settings.clone(),
            settings_open: false,
            settings_section: settings::SettingsSection::default(),
            about_open: false,
            about_section: about::AboutSection::default(),
            image_info: ImageInfoState::new(),
            worker: PageWorker::new(cc.egui_ctx.clone()),
            upscale_worker: AiUpscaleWorker::new(cc.egui_ctx.clone()),
            loader_tx,
            loader_rx,
            loader_generation: 0,
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
            pan: Vec2::ZERO,
            decoded_pages: LruCache::new(NonZeroUsize::new(64).unwrap()),
            decoded_bytes: 0,
            upscaled_pages: LruCache::new(NonZeroUsize::new(8).unwrap()),
            upscaled_bytes: 0,
            use_ai_upscaled_pages: true,
            page_errors: HashMap::new(),
            textures: LruCache::new(NonZeroUsize::new(12).unwrap()),
            debug_compare: DebugCompareState::default(),
            debug_compare_worker: DebugCompareWorker::new(cc.egui_ctx.clone()),
            debug_compare_inflight: HashSet::new(),
            bookmark_thumbnails: BookmarkThumbnails::new(cc.egui_ctx.clone()),
            gpu_effects_available: cc.wgpu_render_state.is_some(),
            gpu_target_format: cc
                .wgpu_render_state
                .as_ref()
                .map(|render_state| render_state.target_format),
            upscale_generation: 0,
            upscale_inflight: None,
            ai_upscale_queue: VecDeque::new(),
            ai_upscale_failures: HashSet::new(),
            last_nav_direction: NavigationDirection::Forward,
            transition_effect: settings.transition_effect,
            transition: None,
            fullscreen: false,
            maximized: false,
            window_position_checked: false,
            status: "Open a folder, ZIP, or CBZ file. Drag-and-drop works too.".to_owned(),
            status_updated_at: Instant::now(),
            pending_state_save_at: None,
            #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
            page_turn_started_at: None,
            #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
            open_to_first_visible_trace: None,
            bookmark_popover_open: false,
            bookmark_popover_pos: Pos2::new(900.0, 72.0),
            bookmark_popover_anchor: None,
            bookmark_filter: BookmarkFilter::default(),
            bookmark_search: String::new(),
            bookmark_clear_confirming: false,
            bookmark_rows: BookmarkRowsCache::default(),
            bookmark_marquee: BookmarkMarqueeState::default(),
            pending_bookmark_jump: None,
        };
        perf::record_app_new(app_started);
        app
    }

    fn set_status(&mut self, status: impl Into<String>) {
        self.status = status.into();
        self.status_updated_at = Instant::now();
    }

    fn open_path(&mut self, path: PathBuf) {
        self.pending_bookmark_jump = None;
        self.open_path_inner(path);
    }

    fn open_path_for_bookmark(&mut self, path: PathBuf, book_id: String, page: usize) {
        self.pending_bookmark_jump = Some(PendingBookmarkJump {
            book_id,
            path: path.clone(),
            page,
        });
        self.open_path_inner(path);
    }

    fn open_path_inner(&mut self, path: PathBuf) {
        let source_kind = classify_path(&path);
        match source_kind {
            SourceKind::Folder | SourceKind::ZipCbz | SourceKind::SingleImage => {
                let origin = match source_kind {
                    SourceKind::Folder => OpenOrigin::Folder,
                    SourceKind::ZipCbz => OpenOrigin::ZipCbz,
                    SourceKind::SingleImage => OpenOrigin::SingleImage,
                    _ => unreachable!("openable source kinds are handled above"),
                };
                self.loader_generation = self.loader_generation.wrapping_add(1);
                let generation = self.loader_generation;
                #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
                {
                    self.open_to_first_visible_trace =
                        Some(perf::OpenToFirstVisibleTrace::new(origin.perf_label()));
                }
                let tx = self.loader_tx.clone();
                let ctx = self.egui_ctx.clone();
                let load_path = path.clone();
                self.set_status(format!("Indexing {} ...", path.display()));

                let _ = thread::Builder::new()
                    .name("suisuiview-source-loader".to_owned())
                    .spawn(move || {
                        let started = Instant::now();
                        let result =
                            open_source_from_path(&load_path).map_err(|error| error.to_string());
                        perf::record_open_source(started, origin.perf_label(), result.is_ok());
                        let _ = tx.send(LoaderEvent {
                            generation,
                            path: load_path,
                            origin,
                            result,
                        });
                        ctx.request_repaint();
                    });
            }
            SourceKind::UnsupportedRar => {
                self.set_status(
                    "CBR/RAR requires the restricted read-only archive backend before it can be opened.",
                );
            }
            SourceKind::Unsupported => {
                let extension = path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .unwrap_or_default();
                self.set_status(
                    unsupported_message_for_extension(extension)
                        .unwrap_or_else(|| format!("Unsupported file type: {}", path.display())),
                );
            }
        }
    }

    fn drain_loader_events(&mut self) {
        while let Ok(event) = self.loader_rx.try_recv() {
            if event.generation != self.loader_generation {
                continue;
            }

            match event.result {
                Ok((source, forced_page)) => {
                    self.install_source(source, forced_page, event.origin, event.path)
                }
                Err(message) => {
                    if self
                        .pending_bookmark_jump
                        .as_ref()
                        .is_some_and(|pending| pending.path == event.path)
                    {
                        self.pending_bookmark_jump = None;
                    }
                    #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
                    {
                        self.open_to_first_visible_trace = None;
                    }
                    self.set_status(format!(
                        "Could not open {}: {message}",
                        event.path.display()
                    ));
                }
            }
        }
    }

    fn install_source(
        &mut self,
        source: SharedSource,
        forced_page: Option<usize>,
        origin: OpenOrigin,
        opened_path: PathBuf,
    ) {
        let book_id = source.book_id().to_owned();
        let page_count = source.page_count();
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        perf::arm_open_to_first_visible(&mut self.open_to_first_visible_trace, &book_id);
        let bookmark = self.store.bookmark(&book_id).cloned();
        self.reading_direction = bookmark
            .as_ref()
            .map(|bookmark| bookmark.reading_direction)
            .unwrap_or_default();
        self.fit_mode = bookmark
            .as_ref()
            .map(|bookmark| bookmark.fit_mode)
            .unwrap_or_default();

        let pending_page = self
            .pending_bookmark_jump
            .as_ref()
            .filter(|pending| pending.book_id == book_id)
            .map(|pending| pending.page);
        let bookmarked_page = bookmark.map(|bookmark| bookmark.last_page);
        self.current_page = pending_page
            .or(forced_page)
            .or(bookmarked_page)
            .unwrap_or_default()
            .min(page_count.saturating_sub(1));
        if pending_page.is_some() {
            self.pending_bookmark_jump = None;
        } else if self
            .pending_bookmark_jump
            .as_ref()
            .is_some_and(|pending| pending.path == opened_path)
        {
            self.pending_bookmark_jump = None;
        }
        self.book_id = Some(book_id.clone());
        self.source = Some(source.clone());
        self.open_origin = Some(origin);
        self.opened_path = Some(opened_path);
        self.pan = Vec2::ZERO;
        self.manual_zoom = 1.0;
        self.effects = ViewEffects::default();
        self.decoded_pages.clear();
        self.decoded_bytes = 0;
        self.upscaled_pages.clear();
        self.upscaled_bytes = 0;
        self.textures.clear();
        self.bookmark_thumbnails.clear();
        self.page_errors.clear();
        self.upscale_generation = self.upscale_generation.wrapping_add(1);
        self.upscale_inflight = None;
        self.ai_upscale_queue.clear();
        self.ai_upscale_failures.clear();
        self.transition = None;
        self.last_nav_direction = NavigationDirection::Forward;
        self.worker.load_book(
            source.clone(),
            self.current_page,
            self.last_nav_direction,
            self.target_long_edge,
            self.visible_page_count(),
            self.worker_options(),
        );
        self.set_status(format!(
            "{} - {} pages - state: {}",
            source.title(),
            page_count,
            self.store.path().display()
        ));
        self.persist_current_bookmark();
        self.refresh_ai_prefetch_queue();
    }

    fn persist_current_bookmark(&mut self) {
        let Some(source) = self.source.as_ref() else {
            return;
        };
        let path = self.current_bookmark_path(source.as_ref()).to_path_buf();
        self.store.upsert_bookmark(BookmarkInput {
            book_id: source.book_id(),
            title: source.title(),
            last_page: self.current_page,
            total_pages: source.page_count(),
            path: &path,
            reading_direction: self.reading_direction,
            fit_mode: self.fit_mode,
        });
        self.bookmark_rows.clear();
        self.pending_state_save_at = None;
    }

    fn persist_current_bookmark_deferred(&mut self) {
        let Some(source) = self.source.as_ref() else {
            return;
        };
        let path = self.current_bookmark_path(source.as_ref()).to_path_buf();
        let changed = self.store.upsert_bookmark_deferred(BookmarkInput {
            book_id: source.book_id(),
            title: source.title(),
            last_page: self.current_page,
            total_pages: source.page_count(),
            path: &path,
            reading_direction: self.reading_direction,
            fit_mode: self.fit_mode,
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
                    self.page_errors.remove(&index);
                    let key = PageCacheKey {
                        index,
                        target_long_edge: page.target_long_edge,
                        decode,
                    };
                    if let Some(notice) = page.notice.as_ref() {
                        self.set_status(notice.clone());
                    }
                    self.insert_prepared_page(key, page);
                    self.prune_decoded_cache();
                    #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
                    if self
                        .page_turn_started_at
                        .as_ref()
                        .is_some_and(|(page, _started)| *page == index)
                    {
                        let (_page, started) = self
                            .page_turn_started_at
                            .take()
                            .expect("checked pending page turn");
                        perf::record_page_turn_ready(
                            started,
                            perf::PageCacheState::Miss,
                            index,
                            key.target_long_edge,
                        );
                    }
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
                    self.page_errors.insert(index, message);
                }
                _ => {}
            }
        }
    }

    fn clear_debug_compare_requests(&mut self) {
        self.debug_compare_inflight.clear();
    }

    fn drain_upscale_events(&mut self) {
        while let Some(event) = self.upscale_worker.try_recv() {
            match event {
                UpscaleEvent::Finished {
                    generation,
                    book_id,
                    page_index,
                    source_hash,
                    decode,
                    page,
                } => {
                    if self.book_id.as_deref() != Some(book_id.as_str())
                        || !self.upscale_inflight.is_some_and(|(active, index)| {
                            active == generation && index == page_index
                        })
                    {
                        continue;
                    }
                    self.upscale_inflight = None;
                    let key = PageCacheKey {
                        index: page_index,
                        target_long_edge: page.target_long_edge,
                        decode,
                    };
                    self.ai_upscale_failures.remove(&key);
                    self.insert_upscaled_page(key, page);
                    self.set_status(format!(
                        "AI upscaled page {} ({})",
                        page_index + 1,
                        &source_hash[..12]
                    ));
                    self.refresh_ai_prefetch_queue();
                }
                UpscaleEvent::Failed {
                    generation,
                    book_id,
                    page_index,
                    target_long_edge,
                    decode,
                    message,
                } => {
                    if self.book_id.as_deref() != Some(book_id.as_str())
                        || !self.upscale_inflight.is_some_and(|(active, index)| {
                            active == generation && index == page_index
                        })
                    {
                        continue;
                    }
                    self.upscale_inflight = None;
                    self.ai_upscale_failures.insert(PageCacheKey {
                        index: page_index,
                        target_long_edge,
                        decode,
                    });
                    self.set_status(format!(
                        "AI upscale failed for page {}: {message}",
                        page_index + 1
                    ));
                    self.refresh_ai_prefetch_queue();
                }
            }
        }
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
        DecodeOptions {
            strategy: match self.settings.decode_mode {
                DecodeMode::AutoFast => DecodeStrategy::Auto,
                DecodeMode::HighQuality => DecodeStrategy::ImageCrate,
            },
            resize_filter: self.settings.resize_filter,
            allow_display_upscale: self.should_allow_display_upscale(),
            apply_exif_orientation: self.settings.apply_exif_orientation,
            apply_embedded_icc: self.settings.apply_embedded_icc,
        }
    }

    fn should_allow_display_upscale(&self) -> bool {
        match self.fit_mode {
            FitMode::FitPage | FitMode::FitWidth | FitMode::FitHeight => true,
            FitMode::Manual => self.manual_zoom > 1.0,
            FitMode::Original => false,
        }
    }

    fn worker_options(&self) -> WorkerOptions {
        WorkerOptions {
            decode: self.decode_options(),
            prefetch_enabled: self.settings.prefetch_enabled,
            progressive_preview_enabled: self.settings.progressive_preview_enabled,
            cache_bytes: self.worker_cache_budget_bytes(),
        }
    }

    fn cpu_cache_budget_bytes(&self) -> usize {
        cache_budget_bytes(&self.settings)
    }

    fn worker_cache_budget_bytes(&self) -> usize {
        (self.cpu_cache_budget_bytes() / 2).clamp(32 * 1024 * 1024, 512 * 1024 * 1024)
    }

    fn insert_prepared_page(&mut self, key: PageCacheKey, page: Arc<PreparedPage>) {
        if let Some((evicted_key, evicted_page)) = self.decoded_pages.push(key, page.clone()) {
            self.decoded_bytes = self.decoded_bytes.saturating_sub(evicted_page.byte_size);
            self.drop_textures_for_page(evicted_key);
        }
        self.decoded_bytes = self.decoded_bytes.saturating_add(page.byte_size);
    }

    fn insert_upscaled_page(&mut self, key: PageCacheKey, page: Arc<PreparedPage>) {
        if let Some((evicted_key, evicted_page)) = self.upscaled_pages.push(key, page.clone()) {
            self.upscaled_bytes = self.upscaled_bytes.saturating_sub(evicted_page.byte_size);
            self.drop_textures_for_page(evicted_key);
        }
        self.upscaled_bytes = self.upscaled_bytes.saturating_add(page.byte_size);
        self.prune_upscaled_cache();
    }

    fn drop_textures_for_page(&mut self, page: PageCacheKey) {
        let stale_keys = self
            .textures
            .iter()
            .filter_map(|(key, _entry)| (key.page == page).then_some(*key))
            .collect::<Vec<_>>();
        for key in stale_keys {
            let _ = self.textures.pop(&key);
        }
    }

    fn prune_decoded_cache(&mut self) {
        let pinned = self.pinned_page_indices();
        let mut retained = Vec::new();
        let max_pops = self.decoded_pages.len();
        let mut pops = 0usize;

        let budget_bytes = self.cpu_cache_budget_bytes();
        while self.decoded_bytes > budget_bytes && pops < max_pops {
            let Some((key, page)) = self.decoded_pages.pop_lru() else {
                break;
            };
            pops += 1;

            if pinned.contains(&key) {
                retained.push((key, page));
                continue;
            }

            self.decoded_bytes = self.decoded_bytes.saturating_sub(page.byte_size);
            self.drop_textures_for_page(key);
        }

        for (key, page) in retained {
            self.decoded_pages.put(key, page);
        }
    }

    fn prune_upscaled_cache(&mut self) {
        let pinned = self.pinned_upscaled_page_indices();
        let mut retained = Vec::new();
        let max_pops = self.upscaled_pages.len();
        let mut pops = 0usize;
        let budget_bytes = (self.cpu_cache_budget_bytes() / 2).max(32 * 1024 * 1024);
        while self.upscaled_bytes > budget_bytes && pops < max_pops {
            let Some((key, page)) = self.upscaled_pages.pop_lru() else {
                break;
            };
            pops += 1;

            if pinned.contains(&key) {
                retained.push((key, page));
                continue;
            }

            self.upscaled_bytes = self.upscaled_bytes.saturating_sub(page.byte_size);
            self.drop_textures_for_page(key);
        }

        for (key, page) in retained {
            self.upscaled_pages.put(key, page);
        }
    }

    fn pinned_page_indices(&self) -> HashSet<PageCacheKey> {
        let mut pinned = self.pin_keys_for_indices(&self.spread_indices(), self.target_long_edge);
        pinned.extend(self.debug_compare_pin_keys());
        if let Some(transition) = self.transition.as_ref() {
            pinned.extend(
                self.pin_keys_for_indices(&transition.from_indices, transition.target_long_edge),
            );
        }
        pinned
    }

    fn pinned_upscaled_page_indices(&self) -> HashSet<PageCacheKey> {
        let mut pinned = self.pin_keys_for_indices(&self.spread_indices(), self.target_long_edge);
        if let Some(source) = self.source.as_ref() {
            let mode = self.settings.ai_upscale.prefetch_mode;
            let prefetch_pages = ai_prefetch_pages_for(
                self.current_page,
                source.page_count(),
                self.view_mode.step(),
                self.last_nav_direction,
                mode,
            );
            pinned.extend(self.pin_keys_for_indices(&prefetch_pages, self.target_long_edge));
        }
        pinned
    }

    fn pin_keys_for_indices(
        &self,
        indices: &[usize],
        target_long_edge: u32,
    ) -> HashSet<PageCacheKey> {
        let mut keys = HashSet::with_capacity(indices.len() * 2);
        for index in indices {
            keys.insert(PageCacheKey {
                index: *index,
                target_long_edge,
                decode: self.decode_options(),
            });
            if self.settings.progressive_preview_enabled
                && target_long_edge > PREVIEW_TARGET_LONG_EDGE
            {
                keys.insert(PageCacheKey {
                    index: *index,
                    target_long_edge: PREVIEW_TARGET_LONG_EDGE,
                    decode: self.decode_options(),
                });
            }
        }
        keys
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
        let commands = ctx.input(collect_keyboard_commands);
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
                    self.set_status("ESC exit is disabled in settings.");
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
            AppCommand::UpscaleCurrentPage => self.upscale_current_page(),
            AppCommand::ToggleCurrentPageBookmark => self.toggle_current_page_bookmark(),
            AppCommand::ToggleBookmarkPopover => self.toggle_bookmark_popover(ctx),
            AppCommand::Unsupported(feature) => {
                self.set_status(format!("{feature} is planned for a later version."));
            }
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
        self.decoded_pages.clear();
        self.decoded_bytes = 0;
        self.upscaled_pages.clear();
        self.upscaled_bytes = 0;
        self.textures.clear();
        self.clear_debug_compare_requests();
        self.bookmark_thumbnails.clear();
        self.page_errors.clear();
        self.upscale_generation = self.upscale_generation.wrapping_add(1);
        self.upscale_inflight = None;
        self.ai_upscale_queue.clear();
        self.ai_upscale_failures.clear();
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        {
            self.page_turn_started_at = None;
            self.open_to_first_visible_trace = None;
        }
        self.transition = None;
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

    fn next_page(&mut self) {
        let Some(source) = self.source.as_ref() else {
            return;
        };
        let max_page = source.page_count().saturating_sub(1);
        if self.current_page >= max_page {
            self.handle_edge_page(NavigationDirection::Forward);
            return;
        }
        let target = self
            .current_page
            .saturating_add(self.view_mode.step())
            .min(max_page);
        self.set_page(target, NavigationDirection::Forward);
    }

    fn previous_page(&mut self) {
        if self.current_page == 0 {
            self.handle_edge_page(NavigationDirection::Backward);
            return;
        }
        let step = self.view_mode.step();
        let target = self.current_page.saturating_sub(step);
        self.set_page(target, NavigationDirection::Backward);
    }

    fn move_pages(&mut self, delta: isize) {
        let Some(source) = self.source.as_ref() else {
            return;
        };
        let max_page = source.page_count().saturating_sub(1);
        let target = if delta.is_negative() {
            self.current_page.saturating_sub(delta.unsigned_abs())
        } else {
            self.current_page
                .saturating_add(delta as usize)
                .min(max_page)
        };
        let direction = if delta < 0 {
            NavigationDirection::Backward
        } else {
            NavigationDirection::Forward
        };
        if target == self.current_page && delta != 0 {
            self.handle_edge_page(direction);
            return;
        }
        self.set_page(target, direction);
    }

    fn handle_edge_page(&mut self, direction: NavigationDirection) {
        match self.settings.edge_page_action {
            EdgePageAction::Stop => {}
            EdgePageAction::Wrap => {
                let Some(source) = self.source.as_ref() else {
                    return;
                };
                let target = match direction {
                    NavigationDirection::Forward => 0,
                    NavigationDirection::Backward => source.page_count().saturating_sub(1),
                };
                self.set_page(target, direction);
            }
            EdgePageAction::NextBook => match direction {
                NavigationDirection::Forward => self.open_sibling_book(1),
                NavigationDirection::Backward => self.open_sibling_book(-1),
            },
        }
    }

    fn random_page(&mut self, direction: NavigationDirection) {
        let Some(source) = self.source.as_ref() else {
            return;
        };
        let page_count = source.page_count();
        if page_count <= 1 {
            return;
        }
        let offset = random_offset(page_count - 1);
        let target = match direction {
            NavigationDirection::Forward => (self.current_page + offset) % page_count,
            NavigationDirection::Backward => (self.current_page + page_count - offset) % page_count,
        };
        self.set_page(target, direction);
    }

    fn set_page(&mut self, target: usize, direction: NavigationDirection) {
        let Some(source) = self.source.as_ref() else {
            return;
        };
        let target = target.min(source.page_count().saturating_sub(1));
        if target == self.current_page {
            return;
        }

        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        let turn_started = Instant::now();
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        let cache_state = {
            let requested_key = PageCacheKey {
                index: target,
                target_long_edge: self.target_long_edge,
                decode: self.decode_options(),
            };
            self.page_turn_cache_state(requested_key)
        };
        let previous_indices = self.spread_indices_for(self.current_page);
        self.current_page = target;
        self.last_nav_direction = direction;
        self.pan = Vec2::ZERO;
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        if cache_state.cached() {
            self.page_turn_started_at = None;
            perf::record_page_turn_ready(turn_started, cache_state, target, self.target_long_edge);
        } else {
            self.page_turn_started_at = Some((target, turn_started));
        }

        if self.transition_effect {
            self.transition = Some(Transition {
                from_indices: previous_indices,
                target_long_edge: self.target_long_edge,
                started_at: Instant::now(),
                direction,
            });
        } else {
            self.transition = None;
        }

        self.worker.set_page(
            self.current_page,
            direction,
            self.target_long_edge,
            self.visible_page_count(),
            self.worker_options(),
        );
        self.persist_current_bookmark_deferred();
        self.refresh_ai_prefetch_queue();
    }

    fn adjust_zoom(&mut self, factor: f32) {
        let previous_decode = self.decode_options();
        self.fit_mode = FitMode::Manual;
        self.manual_zoom = (self.manual_zoom * factor).clamp(0.1, 8.0);
        self.request_page_if_decode_changed(previous_decode);
        self.persist_current_bookmark();
    }

    fn adjust_zoom_by_delta(&mut self, delta: f32) {
        let previous_decode = self.decode_options();
        self.fit_mode = FitMode::Manual;
        self.manual_zoom = (self.manual_zoom + delta).clamp(0.1, 8.0);
        self.request_page_if_decode_changed(previous_decode);
        self.persist_current_bookmark();
    }

    fn set_fit_mode(&mut self, mode: FitMode) {
        let previous_decode = self.decode_options();
        self.fit_mode = mode;
        if mode == FitMode::Original {
            self.manual_zoom = 1.0;
        }
        self.request_page_if_decode_changed(previous_decode);
        self.persist_current_bookmark();
    }

    fn request_page_if_decode_changed(&mut self, previous_decode: DecodeOptions) {
        if self.source.is_some() && previous_decode != self.decode_options() {
            self.worker.set_page(
                self.current_page,
                self.last_nav_direction,
                self.target_long_edge,
                self.visible_page_count(),
                self.worker_options(),
            );
            self.refresh_ai_prefetch_queue();
        }
    }

    fn set_double_mode(&mut self, direction: ReadingDirection) {
        self.view_mode = ViewMode::Double;
        self.reading_direction = direction;
        self.worker.set_page(
            self.current_page,
            self.last_nav_direction,
            self.target_long_edge,
            self.visible_page_count(),
            self.worker_options(),
        );
        self.persist_current_bookmark();
        self.refresh_ai_prefetch_queue();
    }

    fn toggle_double_mode(&mut self) {
        self.view_mode = match self.view_mode {
            ViewMode::Single => ViewMode::Double,
            ViewMode::Double => ViewMode::Single,
        };
        self.worker.set_page(
            self.current_page,
            self.last_nav_direction,
            self.target_long_edge,
            self.visible_page_count(),
            self.worker_options(),
        );
        self.refresh_ai_prefetch_queue();
    }

    fn set_use_ai_upscaled_pages(&mut self, enabled: bool) {
        if self.use_ai_upscaled_pages == enabled {
            return;
        }
        self.use_ai_upscaled_pages = enabled;
        if enabled {
            self.set_status("AI 업스케일 결과를 기본 표시에서 우선 사용합니다.");
        } else {
            self.set_status("AI 업스케일 결과를 숨기고 일반 표시를 사용합니다.");
        }
    }

    fn update_effects(&mut self, update: impl FnOnce(&mut ViewEffects)) {
        update(&mut self.effects);
        self.textures.clear();
        self.set_status(format!(
            "{}{}{}{}",
            self.effects.filter.label(),
            if self.effects.gamma { ", gamma" } else { "" },
            if self.effects.invert_colors {
                ", inverted"
            } else {
                ""
            },
            transform_status_suffix(self.effects.transform)
        ));
    }

    fn delete_current_file(&mut self, mode: DeleteMode) {
        let Some(target) = self.current_delete_target() else {
            self.set_status("No current file to delete.");
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
            self.set_status(
                "Background decode is still finishing; deletion was not attempted. Try again soon.",
            );
            return;
        }
        self.clear_local_book_state("Closing current book before deleting file...");
        let result = match mode {
            DeleteMode::Recycle => trash::delete(&target).map_err(|error| error.to_string()),
            DeleteMode::Permanent => fs::remove_file(&target).map_err(|error| error.to_string()),
        };

        self.set_status(match result {
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
            self.set_status("No current file to reveal.");
            return;
        };
        match reveal_in_file_manager(&target) {
            Ok(()) => {
                self.set_status(format!("Opened file location: {}", target.display()));
            }
            Err(error) => {
                self.set_status(format!("Could not open file location: {error}"));
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
            self.set_status("No current page path to copy.");
            return;
        };
        let Some(text) = source.page_display_path(self.current_page) else {
            self.set_status("No current page path to copy.");
            return;
        };
        match copy_text_to_clipboard(&text) {
            Ok(()) => self.set_status("Copied current path."),
            Err(error) => self.set_status(format!("Could not copy path: {error}")),
        }
    }

    fn copy_current_page_image(&mut self) {
        let Some(image) = self.effected_page_image(self.current_page, self.target_long_edge) else {
            self.set_status("Current page is not ready to copy.");
            return;
        };
        match copy_color_image_to_clipboard(&image) {
            Ok(()) => self.set_status("Copied current page image."),
            Err(error) => self.set_status(format!("Could not copy image: {error}")),
        }
    }

    fn copy_current_spread_image(&mut self) {
        let indices = self.spread_indices();
        let Some(image) = self.compose_spread_image(&indices, self.target_long_edge) else {
            self.set_status("Current spread is not ready to copy.");
            return;
        };
        match copy_color_image_to_clipboard(&image) {
            Ok(()) => self.set_status("Copied visible spread image."),
            Err(error) => self.set_status(format!("Could not copy spread: {error}")),
        }
    }

    fn upscale_current_page(&mut self) {
        if self.settings.ai_upscale.backend != AiUpscaleBackend::RealEsrganNcnn {
            self.set_status("AI upscale is disabled in settings.");
            return;
        }
        if self
            .settings
            .ai_upscale
            .ncnn
            .executable_path
            .trim()
            .is_empty()
        {
            self.set_status("Set the Real-ESRGAN executable path in settings first.");
            return;
        }
        if self.source.is_none() || self.book_id.is_none() {
            self.set_status("Open a book before using AI upscale.");
            return;
        }

        let page_index = self.current_page;
        if self.ai_page_has_prepared_result(page_index) {
            self.set_status(format!(
                "Page {} already has an AI upscale result.",
                page_index + 1
            ));
            return;
        }
        if self
            .upscale_inflight
            .is_some_and(|(_generation, index)| index == page_index)
        {
            self.set_status(format!(
                "AI upscale is already running for page {}.",
                page_index + 1
            ));
            return;
        }

        self.ai_upscale_queue.retain(|queued| *queued != page_index);
        let key = self.ai_page_key(page_index);
        self.ai_upscale_failures.remove(&key);
        self.enqueue_ai_upscale_page(page_index, true);
        self.pump_ai_upscale_queue();
        if self
            .upscale_inflight
            .is_some_and(|(_generation, index)| index != page_index)
        {
            self.set_status(format!(
                "AI upscale queued for page {} after the current job.",
                page_index + 1
            ));
        }
    }

    fn refresh_ai_prefetch_queue(&mut self) {
        let mode = self.settings.ai_upscale.prefetch_mode;
        if mode == AiUpscalePrefetchMode::Off || !self.ai_upscale_can_run() {
            self.ai_upscale_queue.clear();
            return;
        }

        let Some(source) = self.source.as_ref() else {
            self.ai_upscale_queue.clear();
            return;
        };
        let desired_pages = ai_prefetch_pages_for(
            self.current_page,
            source.page_count(),
            self.view_mode.step(),
            self.last_nav_direction,
            mode,
        );
        self.ai_upscale_queue.clear();
        for page_index in desired_pages {
            self.enqueue_ai_upscale_page(page_index, false);
        }
        self.pump_ai_upscale_queue();
    }

    fn ai_upscale_can_run(&self) -> bool {
        self.settings.ai_upscale.backend == AiUpscaleBackend::RealEsrganNcnn
            && !self
                .settings
                .ai_upscale
                .ncnn
                .executable_path
                .trim()
                .is_empty()
            && self.source.is_some()
            && self.book_id.is_some()
    }

    fn ai_page_key(&self, page_index: usize) -> PageCacheKey {
        PageCacheKey {
            index: page_index,
            target_long_edge: self.target_long_edge,
            decode: self.decode_options(),
        }
    }

    fn ai_page_has_prepared_result(&self, page_index: usize) -> bool {
        self.upscaled_pages
            .peek(&self.ai_page_key(page_index))
            .is_some()
    }

    fn ai_page_has_failed(&self, page_index: usize) -> bool {
        self.ai_upscale_failures
            .contains(&self.ai_page_key(page_index))
    }

    fn ai_page_is_pending(&self, page_index: usize) -> bool {
        self.upscale_inflight
            .is_some_and(|(_generation, index)| index == page_index)
            || self
                .ai_upscale_queue
                .iter()
                .any(|queued| *queued == page_index)
    }

    fn enqueue_ai_upscale_page(&mut self, page_index: usize, front: bool) {
        let Some(source) = self.source.as_ref() else {
            return;
        };
        if page_index >= source.page_count()
            || self.ai_page_has_prepared_result(page_index)
            || self.ai_page_has_failed(page_index)
            || self.ai_page_is_pending(page_index)
        {
            return;
        }
        if front {
            self.ai_upscale_queue.push_front(page_index);
        } else {
            self.ai_upscale_queue.push_back(page_index);
        }
    }

    fn pump_ai_upscale_queue(&mut self) {
        if self.upscale_inflight.is_some() {
            return;
        }
        while let Some(page_index) = self.ai_upscale_queue.pop_front() {
            if self.start_ai_upscale_page(page_index) {
                return;
            }
        }
    }

    fn start_ai_upscale_page(&mut self, page_index: usize) -> bool {
        if !self.ai_upscale_can_run()
            || self.ai_page_has_prepared_result(page_index)
            || self.ai_page_has_failed(page_index)
        {
            return false;
        }
        let Some(source) = self.source.as_ref().cloned() else {
            return false;
        };
        let Some(book_id) = self.book_id.clone() else {
            return false;
        };
        if page_index >= source.page_count() {
            return false;
        }

        self.upscale_generation = self.upscale_generation.wrapping_add(1);
        let generation = self.upscale_generation;
        self.upscale_inflight = Some((generation, page_index));
        self.upscale_worker.upscale(UpscaleRequest {
            generation,
            book_id,
            source: source.clone(),
            page_index,
            page_name: source.page_name(page_index).map(str::to_owned),
            target_long_edge: self.target_long_edge,
            decode: self.decode_options(),
            settings: self.settings.ai_upscale.ncnn.clone(),
        });
        let action = if page_index == self.current_page {
            "AI upscaling"
        } else {
            "AI prefetching"
        };
        self.set_status(format!("{action} page {} in background...", page_index + 1));
        true
    }

    fn effected_page_image(&self, index: usize, target_long_edge: u32) -> Option<ColorImage> {
        let key = PageCacheKey {
            index,
            target_long_edge,
            decode: self.decode_options(),
        };
        let page = if let Some(best_key) = self.preferred_upscaled_page_key(key) {
            self.upscaled_pages.peek(&best_key)?
        } else {
            let best_key = self.best_page_key(key)?;
            self.decoded_pages.peek(&best_key)?
        };
        Some(apply_effects_to_image(&page.image, self.effects))
    }

    fn compose_spread_image(&self, indices: &[usize], target_long_edge: u32) -> Option<ColorImage> {
        let images = indices
            .iter()
            .map(|index| self.effected_page_image(*index, target_long_edge))
            .collect::<Option<Vec<_>>>()?;
        compose_images_horizontally(&images, SPREAD_GAP_POINTS as usize)
    }

    fn open_sibling_book(&mut self, direction: isize) {
        let Some(current) = self.current_book_reference_path() else {
            self.set_status("No current book to move from.");
            return;
        };
        let Some(next) = sibling_book_path(&current, direction) else {
            self.set_status("No sibling folder, ZIP, or CBZ found.");
            return;
        };
        self.open_path(next);
    }

    fn current_book_reference_path(&self) -> Option<PathBuf> {
        let source = self.source.as_ref()?;
        match self.open_origin? {
            OpenOrigin::ZipCbz => Some(source.source_path().to_path_buf()),
            OpenOrigin::Folder | OpenOrigin::SingleImage => {
                Some(source.source_path().to_path_buf())
            }
        }
    }

    fn spread_indices(&self) -> Vec<usize> {
        self.spread_indices_for(self.current_page)
    }

    fn spread_indices_for(&self, page: usize) -> Vec<usize> {
        let Some(source) = self.source.as_ref() else {
            return Vec::new();
        };
        let page_count = source.page_count();
        if page_count == 0 {
            return Vec::new();
        }

        let mut indices = vec![page.min(page_count - 1)];
        if self.view_mode == ViewMode::Double {
            let next = page.saturating_add(1);
            if next < page_count {
                indices.push(next);
            }
        }
        if self.reading_direction == ReadingDirection::RightToLeft {
            indices.reverse();
        }
        indices
    }

    fn visible_page_count(&self) -> usize {
        self.view_mode.step()
    }

    fn page_visual(
        &mut self,
        ctx: &egui::Context,
        index: usize,
        target_long_edge: u32,
    ) -> PageVisual {
        if let Some(error) = self.page_errors.get(&index) {
            return PageVisual::Failed {
                index,
                message: error.clone(),
            };
        }

        let key = PageCacheKey {
            index,
            target_long_edge,
            decode: self.decode_options(),
        };
        let (best_key, upscaled) = if let Some(best_key) = self.preferred_upscaled_page_key(key) {
            (best_key, true)
        } else if let Some(best_key) = self.best_page_key(key) {
            (best_key, false)
        } else {
            return PageVisual::Loading { index };
        };
        let use_wgsl_effects = self.can_paint_wgsl_effects();
        let texture_key = TextureCacheKey {
            page: best_key,
            effects: self.effects,
            upscaled,
        };

        if !use_wgsl_effects {
            if let Some(texture) = self
                .textures
                .get(&texture_key)
                .map(|entry| entry.texture.clone())
            {
                let page = if upscaled {
                    self.upscaled_pages.get(&best_key)
                } else {
                    self.decoded_pages.get(&best_key)
                };
                if let Some(page) = page {
                    let size = transformed_page_size(
                        page.original_width as f32,
                        page.original_height as f32,
                        self.effects.transform,
                    );
                    #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
                    perf::record_open_to_first_visible_if_pending(
                        &mut self.open_to_first_visible_trace,
                        self.book_id.as_deref(),
                        index,
                        best_key.target_long_edge,
                        false,
                    );
                    return PageVisual::Ready { texture, size };
                }
            }
        }

        let page = if upscaled {
            self.upscaled_pages.get(&best_key)
        } else {
            self.decoded_pages.get(&best_key)
        }
        .cloned()
        .expect("best page key should exist in decoded cache");
        if use_wgsl_effects {
            let display_upscaler = self.active_display_upscaler();
            #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
            perf::record_open_to_first_visible_if_pending(
                &mut self.open_to_first_visible_trace,
                self.book_id.as_deref(),
                index,
                best_key.target_long_edge,
                true,
            );
            return PageVisual::ReadyGpu {
                source_key: GpuPaintSourceKey {
                    book: self.gpu_paint_book_key(),
                    page: best_key,
                    upscaled,
                    generation: if upscaled { self.upscale_generation } else { 0 },
                },
                image: page.image.clone(),
                size: transformed_page_size(
                    page.original_width as f32,
                    page.original_height as f32,
                    self.effects.transform,
                ),
                effects: self.effects,
                display_upscaler,
            };
        }
        let image = if self.effects == ViewEffects::default() {
            page.image.clone()
        } else {
            #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
            let effects_started = Instant::now();
            let image = Arc::new(apply_effects_to_image(&page.image, self.effects));
            #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
            perf::record_page_effects_cpu(effects_started, index, best_key.target_long_edge);
            image
        };

        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        let texture_started = Instant::now();
        let texture = ctx.load_texture(
            format!(
                "page-{index}-{}-{}-{:?}",
                best_key.target_long_edge,
                if upscaled { "ai" } else { "base" },
                self.effects
            ),
            ImageData::Color(image),
            TextureOptions::LINEAR,
        );
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        perf::record_texture_load(texture_started, index, best_key.target_long_edge, upscaled);
        self.textures.put(
            texture_key,
            TextureEntry {
                texture: texture.clone(),
            },
        );

        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        perf::record_open_to_first_visible_if_pending(
            &mut self.open_to_first_visible_trace,
            self.book_id.as_deref(),
            index,
            best_key.target_long_edge,
            false,
        );
        PageVisual::Ready {
            texture,
            size: transformed_page_size(
                page.original_width as f32,
                page.original_height as f32,
                self.effects.transform,
            ),
        }
    }

    fn best_page_key(&self, requested: PageCacheKey) -> Option<PageCacheKey> {
        if !self.settings.progressive_preview_enabled
            && requested.target_long_edge != PREVIEW_TARGET_LONG_EDGE
        {
            return self.decoded_pages.peek(&requested).map(|_| requested);
        }
        best_page_key_in_cache(&self.decoded_pages, requested)
    }

    fn best_upscaled_page_key(&self, requested: PageCacheKey) -> Option<PageCacheKey> {
        best_page_key_at_or_below_in_cache(&self.upscaled_pages, requested)
    }

    #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
    fn page_turn_cache_state(&self, requested: PageCacheKey) -> perf::PageCacheState {
        if let Some(key) = self.preferred_upscaled_page_key(requested) {
            return page_cache_state_from_hit(Some(key), requested, true);
        }
        if let Some(key) = self.best_page_key(requested) {
            return page_cache_state_from_hit(Some(key), requested, false);
        }
        perf::PageCacheState::Miss
    }

    fn preferred_upscaled_page_key(&self, requested: PageCacheKey) -> Option<PageCacheKey> {
        self.use_ai_upscaled_pages
            .then(|| best_page_key_at_or_below_in_cache(&self.upscaled_pages, requested))
            .flatten()
    }

    fn show_viewer(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let available = ui.available_size();
        let (rect, response) = ui.allocate_exact_size(available, Sense::click_and_drag());
        let painter = ui.painter_at(rect);

        painter.rect_filled(rect, 0.0, ui::theme::VIEWER_BG);
        self.show_context_menu(ctx, &response);

        if self.source.is_none() {
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                "파일이나 폴더를 열어주세요",
                FontId::proportional(22.0),
                ui::theme::TEXT_PRIMARY,
            );
            return;
        }

        self.update_target_long_edge(ctx, rect.size());
        self.handle_viewer_pointer(ui, &response);

        if self.debug_compare.enabled {
            self.transition = None;
            self.paint_debug_compare(ctx, &painter, rect);
            return;
        }

        let current_indices = self.spread_indices();
        if let Some(transition) = self.transition.as_ref() {
            let elapsed_ms = transition.started_at.elapsed().as_secs_f32() * 1000.0;
            let t = (elapsed_ms / TRANSITION_MS).clamp(0.0, 1.0);
            let sign = match transition.direction {
                NavigationDirection::Forward => -1.0,
                NavigationDirection::Backward => 1.0,
            };
            let distance = rect.width() * 0.08;
            let from_offset = Vec2::new(sign * distance * t, 0.0);
            let to_offset = Vec2::new(-sign * distance * (1.0 - t), 0.0);
            let from_indices = transition.from_indices.clone();
            let from_target_long_edge = transition.target_long_edge;
            let current_target_long_edge = self.target_long_edge;

            self.paint_spread(
                ctx,
                &painter,
                SpreadPaint {
                    viewport: rect,
                    indices: &from_indices,
                    target_long_edge: from_target_long_edge,
                    offset: from_offset,
                    alpha: 1.0 - t,
                },
            );
            self.paint_spread(
                ctx,
                &painter,
                SpreadPaint {
                    viewport: rect,
                    indices: &current_indices,
                    target_long_edge: current_target_long_edge,
                    offset: to_offset,
                    alpha: t,
                },
            );

            if t >= 1.0 {
                self.transition = None;
            }
        } else {
            self.paint_spread(
                ctx,
                &painter,
                SpreadPaint {
                    viewport: rect,
                    indices: &current_indices,
                    target_long_edge: self.target_long_edge,
                    offset: Vec2::ZERO,
                    alpha: 1.0,
                },
            );
        }
    }

    fn show_context_menu(&mut self, ctx: &egui::Context, response: &egui::Response) {
        response.context_menu(|ui| {
            ui.set_min_width(280.0);
            let has_book = self.source.is_some();

            self.context_action(ui, ctx, "열기", "F2", AppCommand::OpenFile, true);
            self.context_action(ui, ctx, "폴더 열기", "F", AppCommand::OpenFolder, true);
            self.context_action(ui, ctx, "닫기", "F4", AppCommand::CloseBook, has_book);

            ui.separator();
            self.context_filter(ui, ctx, "필터적용 안함", "U", ImageFilter::None, has_book);
            self.context_filter(ui, ctx, "부드럽게", "I", ImageFilter::Smooth, has_book);
            self.context_filter(
                ui,
                ctx,
                "부드럽게+선명하게",
                "S",
                ImageFilter::SmoothSharpen,
                has_book,
            );

            ui.separator();
            ui.menu_button("이미지 이동", |ui| {
                self.context_action(
                    ui,
                    ctx,
                    "다음 이미지",
                    "PgDn",
                    AppCommand::NextPage,
                    has_book,
                );
                self.context_action(
                    ui,
                    ctx,
                    "이전 이미지",
                    "PgUp",
                    AppCommand::PreviousPage,
                    has_book,
                );
                self.context_action(
                    ui,
                    ctx,
                    "맨 처음 이미지",
                    "Home",
                    AppCommand::Home,
                    has_book,
                );
                self.context_action(
                    ui,
                    ctx,
                    "맨 마지막 이미지",
                    "End",
                    AppCommand::End,
                    has_book,
                );
                ui.separator();
                self.context_action(
                    ui,
                    ctx,
                    "다음 10페이지",
                    "Ctrl+PgDn",
                    AppCommand::MovePages(10),
                    has_book,
                );
                self.context_action(
                    ui,
                    ctx,
                    "이전 10페이지",
                    "Ctrl+PgUp",
                    AppCommand::MovePages(-10),
                    has_book,
                );
                self.context_action(
                    ui,
                    ctx,
                    "랜덤하게 다음 페이지",
                    "Ctrl+Alt+PgDn",
                    AppCommand::RandomForward,
                    has_book,
                );
                self.context_action(
                    ui,
                    ctx,
                    "랜덤하게 이전 페이지",
                    "Ctrl+Alt+PgUp",
                    AppCommand::RandomBackward,
                    has_book,
                );
                ui.separator();
                self.context_action(
                    ui,
                    ctx,
                    "다음 폴더/압축파일",
                    "]",
                    AppCommand::NextBook,
                    has_book,
                );
                self.context_action(
                    ui,
                    ctx,
                    "이전 폴더/압축파일",
                    "[",
                    AppCommand::PreviousBook,
                    has_book,
                );
            });

            ui.menu_button("보기 모드", |ui| {
                self.context_fit_mode(ui, ctx, "원본 크기(100%)", "0", FitMode::Original, has_book);
                self.context_fit_mode(
                    ui,
                    ctx,
                    "꽉 차게 보기",
                    "1 / 9 / Z",
                    FitMode::FitPage,
                    has_book,
                );
                self.context_fit_mode(ui, ctx, "폭맞춤", "8", FitMode::FitWidth, has_book);
                ui.separator();
                self.context_action(
                    ui,
                    ctx,
                    "두장 보기(왼쪽→오른쪽)",
                    "7",
                    AppCommand::SetDouble(ReadingDirection::LeftToRight),
                    has_book,
                );
                self.context_action(
                    ui,
                    ctx,
                    "두장 보기(왼쪽←오른쪽)",
                    "6",
                    AppCommand::SetDouble(ReadingDirection::RightToLeft),
                    has_book,
                );
                self.context_action(
                    ui,
                    ctx,
                    "두장 보기 모드 전환",
                    "2",
                    AppCommand::ToggleDouble,
                    has_book,
                );
            });

            ui.menu_button("축소/확대 보기", |ui| {
                self.context_action(ui, ctx, "확대", "+", AppCommand::Zoom(1.1), has_book);
                self.context_action(ui, ctx, "축소", "-", AppCommand::Zoom(0.9), has_book);
                self.context_action(
                    ui,
                    ctx,
                    "1% 크게 보기",
                    "Ctrl++",
                    AppCommand::ZoomFine(0.01),
                    has_book,
                );
                self.context_action(
                    ui,
                    ctx,
                    "1% 작게 보기",
                    "Ctrl+-",
                    AppCommand::ZoomFine(-0.01),
                    has_book,
                );
            });

            ui.menu_button("이미지 돌려보기", |ui| {
                self.context_action(
                    ui,
                    ctx,
                    "돌려보지 않기",
                    "Alt+↑",
                    AppCommand::SetRotation(0),
                    has_book,
                );
                self.context_action(
                    ui,
                    ctx,
                    "왼쪽으로 돌려보기",
                    "Alt+←",
                    AppCommand::SetRotation(3),
                    has_book,
                );
                self.context_action(
                    ui,
                    ctx,
                    "오른쪽으로 돌려보기",
                    "Alt+→",
                    AppCommand::SetRotation(1),
                    has_book,
                );
                self.context_action(
                    ui,
                    ctx,
                    "거꾸로 돌려보기",
                    "Alt+↓",
                    AppCommand::SetRotation(2),
                    has_book,
                );
                ui.separator();
                self.context_action(
                    ui,
                    ctx,
                    "반시계 방향으로 돌려보기",
                    "Ctrl+L",
                    AppCommand::RotateCounterClockwise,
                    has_book,
                );
                self.context_action(
                    ui,
                    ctx,
                    "시계 방향으로 돌려보기",
                    "Ctrl+R",
                    AppCommand::RotateClockwise,
                    has_book,
                );
            });

            ui.menu_button("영상 처리", |ui| {
                self.context_toggle(
                    ui,
                    ctx,
                    "이미지 반전",
                    "Ctrl+I",
                    self.effects.invert_colors,
                    AppCommand::ToggleInvert,
                );
                self.context_toggle(
                    ui,
                    ctx,
                    "감마 보정",
                    "Ctrl+G",
                    self.effects.gamma,
                    AppCommand::ToggleGamma,
                );
                self.context_toggle(
                    ui,
                    ctx,
                    "상하 반전",
                    "Ctrl+F",
                    self.effects.transform.flip_vertical,
                    AppCommand::ToggleFlipVertical,
                );
                self.context_toggle(
                    ui,
                    ctx,
                    "좌우 반전",
                    "Ctrl+M",
                    self.effects.transform.flip_horizontal,
                    AppCommand::ToggleFlipHorizontal,
                );
                ui.separator();
                self.context_action(
                    ui,
                    ctx,
                    "AI 업스케일",
                    "",
                    AppCommand::UpscaleCurrentPage,
                    has_book,
                );
            });

            ui.separator();
            self.context_action(
                ui,
                ctx,
                "윈도우 탐색기 열기",
                "Ctrl+Enter",
                AppCommand::OpenExplorer,
                has_book,
            );
            self.context_action(
                ui,
                ctx,
                "파일 삭제",
                "Del",
                AppCommand::Delete(DeleteMode::Recycle),
                has_book,
            );
            self.context_action(
                ui,
                ctx,
                "파일 완전히 삭제",
                "Shift+Del",
                AppCommand::Delete(DeleteMode::Permanent),
                has_book,
            );
            self.context_action(
                ui,
                ctx,
                "클립보드로 복사하기",
                "Ctrl+C",
                AppCommand::CopyPageImage,
                has_book,
            );
            self.context_action(
                ui,
                ctx,
                "보이는 이미지 복사",
                "Ctrl+Alt+C",
                AppCommand::CopyDisplayImage,
                has_book,
            );
            self.context_action(
                ui,
                ctx,
                "파일 경로 복사",
                "Ctrl+Alt+Shift+C",
                AppCommand::CopyPath,
                has_book,
            );

            ui.separator();
            self.context_action(
                ui,
                ctx,
                "전체화면",
                "F11",
                AppCommand::ToggleFullscreen,
                true,
            );
            self.context_action(
                ui,
                ctx,
                "최대화/복원",
                "M",
                AppCommand::ToggleMaximized,
                true,
            );
            self.context_action(ui, ctx, "최소화", "Q", AppCommand::Minimize, true);
            if context_selectable(
                ui,
                self.settings.always_on_top,
                "항상 위에 표시",
                "Ctrl+A",
                true,
            )
            .clicked()
            {
                self.apply_command(ctx, AppCommand::ToggleAlwaysOnTop);
                ui.close();
            }
            self.context_action(ui, ctx, "환경설정", "F5", AppCommand::OpenSettings, true);
            self.context_action(ui, ctx, "정보", "F1", AppCommand::OpenAbout, true);
            self.context_action(ui, ctx, "종료", "X", AppCommand::Quit, true);
        });
    }

    fn context_action(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        label: &str,
        shortcut: &str,
        command: AppCommand,
        enabled: bool,
    ) {
        if context_button(ui, label, shortcut, enabled).clicked() {
            self.apply_command(ctx, command);
            ui.close();
        }
    }

    fn context_filter(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        label: &str,
        shortcut: &str,
        filter: ImageFilter,
        enabled: bool,
    ) {
        if context_selectable(ui, self.effects.filter == filter, label, shortcut, enabled).clicked()
        {
            self.apply_command(ctx, AppCommand::SetFilter(filter));
            ui.close();
        }
    }

    fn context_fit_mode(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        label: &str,
        shortcut: &str,
        mode: FitMode,
        enabled: bool,
    ) {
        if context_selectable(ui, self.fit_mode == mode, label, shortcut, enabled).clicked() {
            self.apply_command(ctx, AppCommand::SetFitMode(mode));
            ui.close();
        }
    }

    fn context_toggle(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        label: &str,
        shortcut: &str,
        selected: bool,
        command: AppCommand,
    ) {
        let enabled = self.source.is_some();
        if context_selectable(ui, selected, label, shortcut, enabled).clicked() {
            self.apply_command(ctx, command);
            ui.close();
        }
    }

    fn update_target_long_edge(&mut self, ctx: &egui::Context, viewport: Vec2) {
        if self.source.is_none() {
            return;
        }

        let next = self.target_long_edge_for(ctx, viewport);
        if next == self.target_long_edge
            || next.abs_diff(self.target_long_edge) < TARGET_EDGE_HYSTERESIS
        {
            return;
        }

        self.target_long_edge = next;
        self.worker.set_page(
            self.current_page,
            self.last_nav_direction,
            next,
            self.visible_page_count(),
            self.worker_options(),
        );
        self.refresh_ai_prefetch_queue();
        ctx.request_repaint();
    }

    fn target_long_edge_for(&self, ctx: &egui::Context, viewport: Vec2) -> u32 {
        let page_viewport = match self.view_mode {
            ViewMode::Single => viewport,
            ViewMode::Double => {
                Vec2::new((viewport.x - SPREAD_GAP_POINTS).max(1.0) * 0.5, viewport.y)
            }
        };
        let base_points = match self.fit_mode {
            FitMode::FitWidth => page_viewport.x,
            FitMode::FitHeight => page_viewport.y,
            _ => page_viewport.x.max(page_viewport.y),
        };
        let viewport_pixels = base_points * ctx.pixels_per_point();
        let zoom_multiplier = match self.fit_mode {
            FitMode::Manual => self.manual_zoom.max(1.0),
            FitMode::Original => 1.5,
            _ => 1.0,
        };
        let raw = viewport_pixels * 1.5 * zoom_multiplier;
        let quantized = ((raw / 256.0).ceil() * 256.0) as u32;
        clamp_target_long_edge(quantized.clamp(MIN_TARGET_LONG_EDGE, MAX_TARGET_LONG_EDGE))
    }

    fn handle_viewer_pointer(&mut self, ui: &egui::Ui, response: &egui::Response) {
        if self.settings.double_click_maximize && response.double_clicked() {
            self.toggle_maximized(ui.ctx());
        }

        if response.middle_clicked() {
            if ui.input(|input| input.modifiers.ctrl) {
                self.set_fit_mode(FitMode::Original);
            } else if self.settings.middle_click_fullscreen {
                self.toggle_fullscreen(ui.ctx());
            }
        }

        if response.dragged() {
            self.pan += ui.input(|input| input.pointer.delta());
        }

        if !response.hovered() {
            return;
        }

        let (scroll_y, ctrl) = ui.input(|input| (input.raw_scroll_delta.y, input.modifiers.ctrl));
        if scroll_y.abs() < 1.0 {
            return;
        }

        if ctrl {
            let factor = (1.0 + scroll_y * 0.0015).clamp(0.75, 1.25);
            self.adjust_zoom(factor);
        } else if self.settings.wheel_mode == WheelMode::ScrollWhenZoomed
            && self.fit_mode == FitMode::Manual
            && self.manual_zoom > 1.01
        {
            self.pan.y += scroll_y;
        } else if scroll_y < -30.0 {
            self.next_page();
        } else if scroll_y > 30.0 {
            self.previous_page();
        }
    }

    fn paint_spread(
        &mut self,
        ctx: &egui::Context,
        painter: &egui::Painter,
        request: SpreadPaint<'_>,
    ) {
        if request.indices.is_empty() {
            return;
        }

        let mut pages = Vec::with_capacity(request.indices.len());
        for index in request.indices {
            let visual = self.page_visual(ctx, *index, request.target_long_edge);
            let size = page_visual_size(&visual);
            pages.push((visual, size));
        }

        let gap = if pages.len() > 1 {
            SPREAD_GAP_POINTS
        } else {
            0.0
        };
        let natural_width = pages.iter().map(|(_visual, size)| size.x).sum::<f32>()
            + gap * pages.len().saturating_sub(1) as f32;
        let natural_height = pages
            .iter()
            .map(|(_visual, size)| size.y)
            .fold(1.0_f32, |left, right| left.max(right));
        let scale = self.scale_for(
            request.viewport.size(),
            Vec2::new(natural_width, natural_height),
        );
        let spread_width = natural_width * scale;
        let spread_height = natural_height * scale;
        let mut cursor = self.spread_origin(
            request.viewport,
            Vec2::new(spread_width, spread_height),
            request.offset,
        );
        let tint = Color32::from_white_alpha((request.alpha.clamp(0.0, 1.0) * 255.0) as u8);

        for (visual, size) in pages {
            let page_size = size * scale;
            let top = cursor.y + (spread_height - page_size.y) * 0.5;
            let page_rect = Rect::from_min_size(Pos2::new(cursor.x, top), page_size);

            match visual {
                PageVisual::Ready { texture, .. } => {
                    painter.image(
                        texture.id(),
                        page_rect,
                        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                        tint,
                    );
                }
                PageVisual::ReadyGpu {
                    source_key,
                    image,
                    effects,
                    display_upscaler,
                    ..
                } => {
                    if !self.paint_wgsl_effects(
                        painter,
                        GpuPaintRequest {
                            rect: page_rect,
                            source_key,
                            image,
                            effects,
                            display_upscaler,
                            opacity: request.alpha,
                        },
                    ) {
                        self.paint_placeholder(
                            painter,
                            page_rect,
                            "GPU effect fallback pending",
                            Color32::from_gray(120),
                            tint,
                        );
                    }
                }
                PageVisual::Loading { index } => {
                    self.paint_placeholder(
                        painter,
                        page_rect,
                        &format!("Loading page {}", index + 1),
                        Color32::from_gray(120),
                        tint,
                    );
                }
                PageVisual::Failed { index, message } => {
                    self.paint_placeholder(
                        painter,
                        page_rect,
                        &format!("Page {} failed\n{}", index + 1, message),
                        Color32::from_rgb(180, 80, 80),
                        tint,
                    );
                }
            }

            cursor.x += page_size.x + gap * scale;
        }
    }

    fn paint_placeholder(
        &self,
        painter: &egui::Painter,
        rect: Rect,
        text: &str,
        color: Color32,
        tint: Color32,
    ) {
        let stroke = Stroke::new(1.0, color.gamma_multiply(tint.a() as f32 / 255.0));
        painter.rect_stroke(rect, 2.0, stroke, StrokeKind::Inside);
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            text,
            FontId::proportional(16.0),
            color.gamma_multiply(tint.a() as f32 / 255.0),
        );
    }

    fn spread_origin(&self, viewport: Rect, spread_size: Vec2, offset: Vec2) -> Pos2 {
        let centered_x = viewport.center().x - spread_size.x * 0.5;
        let centered_y = viewport.center().y - spread_size.y * 0.5;
        let x = if spread_size.x > viewport.width()
            && self.settings.large_image_anchor == LargeImageAnchor::TopLeft
        {
            viewport.left()
        } else {
            centered_x
        };
        let y = if spread_size.y > viewport.height()
            && matches!(
                self.settings.large_image_anchor,
                LargeImageAnchor::Top | LargeImageAnchor::TopLeft
            ) {
            viewport.top()
        } else {
            centered_y
        };

        Pos2::new(x + self.pan.x + offset.x, y + self.pan.y + offset.y)
    }

    fn scale_for(&self, viewport: Vec2, natural: Vec2) -> f32 {
        let safe = Vec2::new(natural.x.max(1.0), natural.y.max(1.0));
        match self.fit_mode {
            FitMode::FitPage => (viewport.x / safe.x).min(viewport.y / safe.y),
            FitMode::FitWidth => viewport.x / safe.x,
            FitMode::FitHeight => viewport.y / safe.y,
            FitMode::Original => 1.0,
            FitMode::Manual => self.manual_zoom,
        }
        .clamp(0.02, 16.0)
    }
}

impl eframe::App for SuiSuiViewApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        let update_started = Instant::now();
        self.drain_loader_events();
        self.drain_worker_events();
        self.drain_debug_compare_events();
        self.drain_upscale_events();
        self.bookmark_thumbnails.drain(ctx);
        self.handle_dropped_files(ctx);
        self.handle_keyboard(ctx);
        self.maintain_native_window_state(ctx);
        self.flush_deferred_state_save_if_due();

        self.show_top_bar(ctx);
        self.show_status_surfaces(ctx);
        self.show_settings_window(ctx);
        self.show_about_window(ctx);

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ctx, |ui| self.show_viewer(ui, ctx));

        self.show_bookmark_popover(ctx);

        if self.transition.is_some() {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        perf::record_ui_update(
            update_started,
            self.source.is_some(),
            self.transition.is_some(),
        );
    }
}

impl Drop for SuiSuiViewApp {
    fn drop(&mut self) {
        let shutdown_started = Instant::now();
        let page_worker_stopped = self.worker.request_shutdown();
        let debug_compare_stopped = self.debug_compare_worker.request_shutdown();
        let thumbnails_stopped = self.bookmark_thumbnails.request_shutdown();
        let upscale_stopped = self.upscale_worker.request_shutdown();
        self.flush_deferred_state_save();
        perf::record_app_shutdown(
            shutdown_started,
            self.source.is_some(),
            page_worker_stopped,
            debug_compare_stopped,
            thumbnails_stopped,
            upscale_stopped,
        );
        perf::flush();
    }
}

fn context_button(ui: &mut egui::Ui, label: &str, shortcut: &str, enabled: bool) -> egui::Response {
    ui.add_enabled(
        enabled,
        egui::Button::new(label).shortcut_text(shortcut.to_owned()),
    )
}

fn apply_window_level(ctx: &egui::Context, always_on_top: bool) {
    let level = if always_on_top {
        egui::WindowLevel::AlwaysOnTop
    } else {
        egui::WindowLevel::Normal
    };
    ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(level));
}

fn cache_budget_bytes(settings: &AppSettings) -> usize {
    match settings.cache_memory_mode {
        CacheMemoryMode::Auto => automatic_cache_budget_bytes(),
        CacheMemoryMode::Manual => {
            (settings.manual_cache_mb.clamp(64, 2048) as usize) * 1024 * 1024
        }
    }
}

fn automatic_cache_budget_bytes() -> usize {
    static AUTO_CACHE_BUDGET: OnceLock<usize> = OnceLock::new();
    *AUTO_CACHE_BUDGET.get_or_init(|| {
        let total = sysinfo::System::new_with_specifics(
            sysinfo::RefreshKind::nothing()
                .with_memory(sysinfo::MemoryRefreshKind::nothing().with_ram()),
        )
        .total_memory() as usize;
        let target = total.saturating_mul(3) / 100;
        target.clamp(128 * 1024 * 1024, 768 * 1024 * 1024)
    })
}

fn context_selectable(
    ui: &mut egui::Ui,
    selected: bool,
    label: &str,
    shortcut: &str,
    enabled: bool,
) -> egui::Response {
    ui.add_enabled(
        enabled,
        egui::Button::new(label)
            .selected(selected)
            .shortcut_text(shortcut.to_owned()),
    )
}

fn copy_text_to_clipboard(text: &str) -> Result<(), String> {
    Clipboard::new()
        .map_err(|error| error.to_string())?
        .set_text(text.to_owned())
        .map_err(|error| error.to_string())
}

fn copy_color_image_to_clipboard(image: &ColorImage) -> Result<(), String> {
    let mut bytes = Vec::with_capacity(image.pixels.len() * 4);
    for pixel in &image.pixels {
        bytes.extend_from_slice(&[pixel.r(), pixel.g(), pixel.b(), pixel.a()]);
    }
    Clipboard::new()
        .map_err(|error| error.to_string())?
        .set_image(ClipboardImageData {
            width: image.size[0],
            height: image.size[1],
            bytes: Cow::Owned(bytes),
        })
        .map_err(|error| error.to_string())
}

fn reveal_in_file_manager(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        Command::new("explorer")
            .arg(format!("/select,{}", path.display()))
            .spawn()
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    #[cfg(target_os = "macos")]
    {
        Command::new("open")
            .arg("-R")
            .arg(path)
            .spawn()
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    {
        let target = path.parent().unwrap_or(path);
        Command::new("xdg-open")
            .arg(target)
            .spawn()
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

fn random_offset(max: usize) -> usize {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as usize)
        .unwrap_or(1);
    nanos % max + 1
}

fn ai_prefetch_pages_for(
    current_page: usize,
    page_count: usize,
    step: usize,
    direction: NavigationDirection,
    mode: AiUpscalePrefetchMode,
) -> Vec<usize> {
    if page_count == 0 || mode == AiUpscalePrefetchMode::Off {
        return Vec::new();
    }

    let current_page = current_page.min(page_count - 1);
    let mut pages = vec![current_page];
    if mode == AiUpscalePrefetchMode::CurrentAndNext {
        let step = step.max(1);
        let adjacent = match direction {
            NavigationDirection::Forward => current_page.saturating_add(step),
            NavigationDirection::Backward => current_page.saturating_sub(step),
        };
        if adjacent < page_count && adjacent != current_page {
            pages.push(adjacent);
        }
    }
    pages
}

fn sibling_book_path(current: &Path, direction: isize) -> Option<PathBuf> {
    let parent = current.parent()?;
    let mut entries = fs::read_dir(parent)
        .ok()?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| matches!(classify_path(path), SourceKind::Folder | SourceKind::ZipCbz))
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| {
        let left_name = left
            .file_name()
            .map(|name| name.to_string_lossy())
            .unwrap_or_default();
        let right_name = right
            .file_name()
            .map(|name| name.to_string_lossy())
            .unwrap_or_default();
        cmp_natural(&left_name, &right_name)
    });
    if entries.len() <= 1 {
        return None;
    }
    let current_index = entries
        .iter()
        .position(|path| same_path(path, current))
        .unwrap_or_else(|| {
            entries
                .iter()
                .position(|path| path == current)
                .unwrap_or_default()
        });
    let next_index = if direction >= 0 {
        (current_index + 1) % entries.len()
    } else {
        (current_index + entries.len() - 1) % entries.len()
    };
    Some(entries[next_index].clone())
}

fn same_path(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
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

fn page_visual_size(visual: &PageVisual) -> Vec2 {
    match visual {
        PageVisual::Ready { size, .. } => *size,
        PageVisual::ReadyGpu { size, .. } => *size,
        PageVisual::Loading { .. } | PageVisual::Failed { .. } => Vec2::new(900.0, 1300.0),
    }
}

fn best_page_key_in_cache(
    cache: &LruCache<PageCacheKey, Arc<PreparedPage>>,
    requested: PageCacheKey,
) -> Option<PageCacheKey> {
    if cache.peek(&requested).is_some() {
        return Some(requested);
    }

    let mut best_smaller = None;
    let mut smallest_any = None;
    for (key, _page) in cache.iter() {
        if key.index != requested.index || key.decode != requested.decode {
            continue;
        }
        if key.target_long_edge <= requested.target_long_edge
            && best_smaller
                .is_none_or(|best: PageCacheKey| key.target_long_edge > best.target_long_edge)
        {
            best_smaller = Some(*key);
        }
        if smallest_any
            .is_none_or(|smallest: PageCacheKey| key.target_long_edge < smallest.target_long_edge)
        {
            smallest_any = Some(*key);
        }
    }

    best_smaller.or(smallest_any)
}

fn best_page_key_at_or_below_in_cache(
    cache: &LruCache<PageCacheKey, Arc<PreparedPage>>,
    requested: PageCacheKey,
) -> Option<PageCacheKey> {
    if cache.peek(&requested).is_some() {
        return Some(requested);
    }

    cache
        .iter()
        .filter_map(|(key, _page)| {
            (key.index == requested.index
                && key.decode == requested.decode
                && key.target_long_edge <= requested.target_long_edge)
                .then_some(*key)
        })
        .max_by_key(|key| key.target_long_edge)
}

#[cfg(any(test, feature = "perf-dev", feature = "perf-diagnostics"))]
fn page_cache_state_from_hit(
    hit: Option<PageCacheKey>,
    requested: PageCacheKey,
    upscaled: bool,
) -> perf::PageCacheState {
    use std::cmp::Ordering;

    let Some(hit) = hit else {
        return perf::PageCacheState::Miss;
    };
    match (
        upscaled,
        hit.target_long_edge.cmp(&requested.target_long_edge),
    ) {
        (true, Ordering::Equal) => perf::PageCacheState::UpscaledExact,
        (true, Ordering::Less) => perf::PageCacheState::UpscaledPreview,
        (true, Ordering::Greater) => perf::PageCacheState::UpscaledFallback,
        (false, Ordering::Equal) => perf::PageCacheState::DecodedExact,
        (false, Ordering::Less) => perf::PageCacheState::DecodedPreview,
        (false, Ordering::Greater) => perf::PageCacheState::DecodedFallback,
    }
}

#[cfg(test)]
fn preferred_page_key_in_cache(
    cache: &LruCache<PageCacheKey, Arc<PreparedPage>>,
    requested: PageCacheKey,
    enabled: bool,
) -> Option<PageCacheKey> {
    enabled
        .then(|| best_page_key_in_cache(cache, requested))
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::perf::PageCacheState;
    use super::{
        ai_prefetch_pages_for, apply_effects_to_image, best_page_key_at_or_below_in_cache,
        best_page_key_in_cache, command_for_shortcut, delete_target_for, korean_font_candidates,
        load_first_existing_font, page_cache_state_from_hit, preferred_page_key_in_cache,
        sanitize_font_name, sibling_book_path, transformed_page_size, AppCommand, DeleteMode,
        ImageFilter, OpenOrigin, PageCacheKey, Shortcut, TextureCacheKey, ViewEffects,
        ViewTransform,
    };
    use crate::core::source::{BookSource, SourceError};
    use crate::core::state::{
        AiUpscalePrefetchMode, AppSettings, CacheMemoryMode, FitMode, ReadingDirection,
    };
    use crate::core::worker::{
        DecodeBackend, DecodeOptions, DecodeStrategy, NavigationDirection, PreparedPage,
        PREVIEW_TARGET_LONG_EDGE,
    };
    use eframe::egui::{Color32, ColorImage, Key};
    use lru::LruCache;
    use std::fs;
    use std::num::NonZeroUsize;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};

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
    fn page_cache_state_tracks_exact_preview_fallback_and_source() {
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
            page_cache_state_from_hit(Some(requested), requested, false),
            PageCacheState::DecodedExact
        );
        assert_eq!(
            page_cache_state_from_hit(Some(preview), requested, false),
            PageCacheState::DecodedPreview
        );
        assert_eq!(
            page_cache_state_from_hit(Some(fallback), requested, false),
            PageCacheState::DecodedFallback
        );
        assert_eq!(
            page_cache_state_from_hit(Some(requested), requested, true),
            PageCacheState::UpscaledExact
        );
        assert_eq!(
            page_cache_state_from_hit(Some(preview), requested, true),
            PageCacheState::UpscaledPreview
        );
        assert_eq!(
            page_cache_state_from_hit(Some(fallback), requested, true),
            PageCacheState::UpscaledFallback
        );
        assert_eq!(
            page_cache_state_from_hit(None, requested, false),
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
            upscaled: false,
        };
        let inverted = TextureCacheKey {
            page,
            effects: ViewEffects {
                invert_colors: true,
                ..ViewEffects::default()
            },
            upscaled: false,
        };

        assert_ne!(normal, inverted);
        assert_eq!(normal.page, inverted.page);
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
        assert_eq!(
            command_for_shortcut(Shortcut::key(Key::F)),
            Some(AppCommand::OpenFolder)
        );
        assert_eq!(
            command_for_shortcut(Shortcut::key(Key::F11)),
            Some(AppCommand::ToggleFullscreen)
        );
        assert_eq!(
            command_for_shortcut(Shortcut::alt(Key::Enter)),
            Some(AppCommand::ToggleFullscreen)
        );
        assert_eq!(
            command_for_shortcut(Shortcut::key(Key::N)),
            Some(AppCommand::ToggleFullscreen)
        );
        assert_eq!(
            command_for_shortcut(Shortcut::ctrl(Key::F)),
            Some(AppCommand::ToggleFlipVertical)
        );
        assert_eq!(
            command_for_shortcut(Shortcut::key(Key::F5)),
            Some(AppCommand::OpenSettings)
        );
        assert_eq!(
            command_for_shortcut(Shortcut::key(Key::F1)),
            Some(AppCommand::OpenAbout)
        );
        assert_eq!(
            command_for_shortcut(Shortcut::ctrl(Key::A)),
            Some(AppCommand::ToggleAlwaysOnTop)
        );
        assert_eq!(
            command_for_shortcut(Shortcut::key(Key::B)),
            Some(AppCommand::ToggleCurrentPageBookmark)
        );
        assert_eq!(
            command_for_shortcut(Shortcut::ctrl(Key::B)),
            Some(AppCommand::ToggleBookmarkPopover)
        );
    }

    #[test]
    fn shortcut_maps_view_modes_and_delete_modes() {
        assert_eq!(
            command_for_shortcut(Shortcut::key(Key::Z)),
            Some(AppCommand::SetFitMode(FitMode::FitPage))
        );
        assert_eq!(
            command_for_shortcut(Shortcut::key(Key::Num7)),
            Some(AppCommand::SetDouble(ReadingDirection::LeftToRight))
        );
        assert_eq!(
            command_for_shortcut(Shortcut::key(Key::Num6)),
            Some(AppCommand::SetDouble(ReadingDirection::RightToLeft))
        );
        assert_eq!(
            command_for_shortcut(Shortcut::key(Key::Delete)),
            Some(AppCommand::Delete(DeleteMode::Recycle))
        );
        assert_eq!(
            command_for_shortcut(Shortcut {
                key: super::ShortcutKey::Egui(Key::Delete),
                ctrl: false,
                alt: false,
                shift: true,
            }),
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
            upscaled: false,
        };
        let filtered = TextureCacheKey {
            page,
            effects: ViewEffects {
                filter: ImageFilter::SmoothSharpen,
                ..ViewEffects::default()
            },
            upscaled: false,
        };

        assert_ne!(none, filtered);
    }

    #[test]
    fn texture_cache_key_tracks_ai_upscaled_pages() {
        let page = PageCacheKey {
            index: 0,
            target_long_edge: 1024,
            decode: DecodeOptions::default(),
        };
        let base = TextureCacheKey {
            page,
            effects: ViewEffects::default(),
            upscaled: false,
        };
        let ai = TextureCacheKey {
            upscaled: true,
            ..base
        };

        assert_ne!(base, ai);
    }

    #[test]
    fn preferred_page_key_honors_ai_visibility_toggle() {
        let mut cache = LruCache::new(NonZeroUsize::new(2).unwrap());
        let key = PageCacheKey {
            index: 0,
            target_long_edge: 2048,
            decode: DecodeOptions::default(),
        };
        cache.put(key, dummy_page(2048));

        assert_eq!(preferred_page_key_in_cache(&cache, key, true), Some(key));
        assert_eq!(preferred_page_key_in_cache(&cache, key, false), None);
    }

    #[test]
    fn ai_page_key_lookup_avoids_larger_stale_targets() {
        let mut cache = LruCache::new(NonZeroUsize::new(2).unwrap());
        let small = PageCacheKey {
            index: 0,
            target_long_edge: 1024,
            decode: DecodeOptions::default(),
        };
        let large = PageCacheKey {
            target_long_edge: 4096,
            ..small
        };
        cache.put(large, dummy_page(4096));

        assert_eq!(best_page_key_at_or_below_in_cache(&cache, small), None);

        let mut cache = LruCache::new(NonZeroUsize::new(2).unwrap());
        cache.put(small, dummy_page(1024));
        assert_eq!(
            best_page_key_at_or_below_in_cache(&cache, large),
            Some(small)
        );
    }

    #[test]
    fn ai_prefetch_pages_follow_mode_and_direction() {
        assert_eq!(
            ai_prefetch_pages_for(
                5,
                10,
                1,
                NavigationDirection::Forward,
                AiUpscalePrefetchMode::Off
            ),
            Vec::<usize>::new()
        );
        assert_eq!(
            ai_prefetch_pages_for(
                5,
                10,
                1,
                NavigationDirection::Forward,
                AiUpscalePrefetchMode::CurrentOnly
            ),
            vec![5]
        );
        assert_eq!(
            ai_prefetch_pages_for(
                5,
                10,
                2,
                NavigationDirection::Forward,
                AiUpscalePrefetchMode::CurrentAndNext
            ),
            vec![5, 7]
        );
        assert_eq!(
            ai_prefetch_pages_for(
                5,
                10,
                2,
                NavigationDirection::Backward,
                AiUpscalePrefetchMode::CurrentAndNext
            ),
            vec![5, 3]
        );
        assert_eq!(
            ai_prefetch_pages_for(
                9,
                10,
                1,
                NavigationDirection::Forward,
                AiUpscalePrefetchMode::CurrentAndNext
            ),
            vec![9]
        );
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
            manual_cache_mb: 8,
            ..AppSettings::default()
        };

        assert_eq!(super::cache_budget_bytes(&settings), 64 * 1024 * 1024);
        settings.manual_cache_mb = 4096;
        assert_eq!(super::cache_budget_bytes(&settings), 2048 * 1024 * 1024);
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
            image: Arc::new(ColorImage::from_rgba_unmultiplied(
                [1, 1],
                &[255, 255, 255, 255],
            )),
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
