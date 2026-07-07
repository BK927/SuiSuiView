use super::{
    adjacent_seed::{prepare_seeded_first_page, SeedTargetView},
    perf,
    sibling_books::{same_path, sibling_book_path},
    viewer::{ViewTargetSettle, SPREAD_GAP_POINTS},
    PendingBookmarkJump, SeededPreparedPage, SiblingOpenRetry, SuiSuiViewApp,
};
use crate::core::effects::ViewEffects;
use crate::core::formats::unsupported_message_for_extension;
use crate::core::source::{
    classify_path, open_source_from_path, BookSource, SharedSource, SourceKind,
};
use crate::core::state::{
    AppSettings, DecodeMode, DecoderPreferences, FitMode, ReadingDirection, ReadingPosition,
    StateStore, WindowPlacement,
};
use crate::core::worker::{
    clamp_navigation_target_long_edge, DecodeOptions, DecodeStrategy, NavigationDirection,
    DEFAULT_TARGET_LONG_EDGE, PREVIEW_TARGET_LONG_EDGE,
};
use crossbeam_channel::{unbounded, Receiver, Sender};
use egui::Vec2;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app) enum OpenOrigin {
    Folder,
    ZipCbz,
    SingleImage,
}

impl OpenOrigin {
    pub(in crate::app) fn perf_label(self) -> &'static str {
        match self {
            Self::Folder => "folder",
            Self::ZipCbz => "zip_cbz",
            Self::SingleImage => "single_image",
        }
    }
}

pub(in crate::app) struct LoaderEvent {
    pub(in crate::app) generation: u64,
    pub(in crate::app) path: PathBuf,
    pub(in crate::app) origin: OpenOrigin,
    pub(in crate::app) initial_direction: NavigationDirection,
    pub(in crate::app) view_fallback: Option<OpenViewFallback>,
    pub(in crate::app) explicit_page: Option<usize>,
    pub(in crate::app) failure_action: OpenFailureAction,
    pub(in crate::app) result: Result<(SharedSource, Option<usize>), String>,
    pub(in crate::app) seeded_page: Option<SeededPreparedPage>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app) enum OpenFailureAction {
    KeepCurrent,
    ClearCurrent,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(in crate::app) struct OpenViewFallback {
    pub(in crate::app) reading_direction: ReadingDirection,
    pub(in crate::app) fit_mode: FitMode,
    pub(in crate::app) manual_zoom: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct ResolvedOpenView {
    reading_direction: ReadingDirection,
    fit_mode: FitMode,
    manual_zoom: f32,
}

pub(crate) struct StartupOpen {
    loader_tx: Sender<LoaderEvent>,
    loader_rx: Receiver<LoaderEvent>,
    generation: u64,
    origin: OpenOrigin,
    started_at: Instant,
}

pub(in crate::app) struct StartupOpenParts {
    pub(in crate::app) loader_tx: Sender<LoaderEvent>,
    pub(in crate::app) loader_rx: Receiver<LoaderEvent>,
    pub(in crate::app) generation: u64,
    pub(in crate::app) origin: OpenOrigin,
    pub(in crate::app) started_at: Instant,
}

impl StartupOpen {
    pub(in crate::app) fn into_parts(self) -> StartupOpenParts {
        StartupOpenParts {
            loader_tx: self.loader_tx,
            loader_rx: self.loader_rx,
            generation: self.generation,
            origin: self.origin,
            started_at: self.started_at,
        }
    }
}

pub(crate) fn start_startup_open_loader(path: PathBuf, store: &StateStore) -> Option<StartupOpen> {
    let origin = open_origin_for_source_kind(classify_path(&path))?;
    let (loader_tx, loader_rx) = unbounded();
    let event_tx = loader_tx.clone();
    let generation = 1;
    let started_at = Instant::now();
    let load_path = path.clone();
    let store = store.clone();
    let settings = store.settings().clone();
    let target_long_edge = startup_seed_target_long_edge(store.window_placement());
    let decode = startup_decode_options(&settings);
    let resume_by_file_identity = settings.resume_by_file_identity;
    perf::record_startup_open_preload(origin.perf_label());
    thread::Builder::new()
        .name("suisuiview-startup-source-loader".to_owned())
        .spawn(move || {
            let started = Instant::now();
            let result = open_source_from_path(&load_path).map_err(|error| error.to_string());
            perf::record_open_source(started, origin.perf_label(), result.is_ok());
            let seeded_page = result.as_ref().ok().and_then(|(source, forced_page)| {
                let reading_position = reading_position_for_open(
                    &store,
                    source.as_ref(),
                    origin,
                    &load_path,
                    resume_by_file_identity,
                );
                let page_index = selected_open_page(
                    source.as_ref(),
                    None,
                    *forced_page,
                    reading_position.as_ref(),
                    None,
                );
                let started = Instant::now();
                let seeded = prepare_seeded_first_page(
                    source.as_ref(),
                    page_index,
                    target_long_edge,
                    decode,
                    false,
                    None,
                );
                perf::record_startup_seed_prepare(
                    started,
                    origin.perf_label(),
                    page_index,
                    target_long_edge,
                    seeded.is_some(),
                );
                seeded
            });
            let _ = event_tx.send(LoaderEvent {
                generation,
                path: load_path,
                origin,
                initial_direction: NavigationDirection::Forward,
                view_fallback: None,
                explicit_page: None,
                failure_action: OpenFailureAction::KeepCurrent,
                result,
                seeded_page,
            });
        })
        .ok()?;

    Some(StartupOpen {
        loader_tx,
        loader_rx,
        generation,
        origin,
        started_at,
    })
}

impl SuiSuiViewApp {
    pub(in crate::app) fn open_path(&mut self, path: PathBuf) {
        self.sibling_open_retry = None;
        self.clear_pending_sibling_book_turns();
        self.open_path_with_initial_direction(path, NavigationDirection::Forward);
    }

    pub(in crate::app) fn open_path_with_initial_direction(
        &mut self,
        path: PathBuf,
        initial_direction: NavigationDirection,
    ) {
        self.pending_bookmark_jump = None;
        self.clear_adjacent_seed_cache();
        self.open_path_inner(
            path,
            initial_direction,
            None,
            None,
            OpenFailureAction::KeepCurrent,
        );
    }

    pub(in crate::app) fn open_sibling_path_with_initial_direction(
        &mut self,
        path: PathBuf,
        initial_direction: NavigationDirection,
    ) {
        let view_fallback = Some(self.open_view_fallback());
        self.pending_bookmark_jump = None;
        self.clear_adjacent_seed_cache();
        self.open_path_inner(
            path,
            initial_direction,
            view_fallback,
            None,
            OpenFailureAction::KeepCurrent,
        );
    }

    pub(in crate::app) fn open_path_for_bookmark(
        &mut self,
        path: PathBuf,
        book_id: String,
        page: usize,
    ) {
        self.sibling_open_retry = None;
        self.clear_pending_sibling_book_turns();
        self.pending_bookmark_jump = Some(PendingBookmarkJump {
            book_id,
            path: path.clone(),
            page,
        });
        self.clear_adjacent_seed_cache();
        self.open_path_inner(
            path,
            NavigationDirection::Forward,
            None,
            None,
            OpenFailureAction::KeepCurrent,
        );
    }

    pub(in crate::app) fn open_path_with_explicit_page(
        &mut self,
        path: PathBuf,
        initial_direction: NavigationDirection,
        explicit_page: Option<usize>,
        view_fallback: Option<OpenViewFallback>,
    ) {
        self.sibling_open_retry = None;
        self.pending_bookmark_jump = None;
        self.clear_adjacent_seed_cache();
        self.open_path_inner(
            path,
            initial_direction,
            view_fallback,
            explicit_page,
            OpenFailureAction::KeepCurrent,
        );
    }

    pub(in crate::app) fn open_path_after_successful_delete(
        &mut self,
        path: PathBuf,
        initial_direction: NavigationDirection,
        explicit_page: Option<usize>,
        view_fallback: Option<OpenViewFallback>,
    ) {
        self.sibling_open_retry = None;
        self.pending_bookmark_jump = None;
        self.clear_adjacent_seed_cache();
        self.open_path_inner(
            path,
            initial_direction,
            view_fallback,
            explicit_page,
            OpenFailureAction::ClearCurrent,
        );
    }

    pub(in crate::app) fn open_path_inner(
        &mut self,
        path: PathBuf,
        initial_direction: NavigationDirection,
        view_fallback: Option<OpenViewFallback>,
        explicit_page: Option<usize>,
        failure_action: OpenFailureAction,
    ) {
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
                let store = self.store.clone();
                let settings = self.settings.clone();
                let seed_target_long_edge = open_seed_target_long_edge(self.target_long_edge);
                let seed_target_view = self.seed_target_view_for_open(view_fallback);
                let decode = startup_decode_options(&settings);
                let resume_by_file_identity = settings.resume_by_file_identity;
                let pending_bookmark_jump = self.pending_bookmark_jump.clone();
                self.set_status(self.i18n().text("status.opening"));

                let spawn_result = thread::Builder::new()
                    .name("suisuiview-source-loader".to_owned())
                    .spawn(move || {
                        let started = Instant::now();
                        let result =
                            open_source_from_path(&load_path).map_err(|error| error.to_string());
                        perf::record_open_source(started, origin.perf_label(), result.is_ok());
                        let seeded_page = result.as_ref().ok().and_then(|(source, forced_page)| {
                            let reading_position = reading_position_for_open(
                                &store,
                                source.as_ref(),
                                origin,
                                &load_path,
                                resume_by_file_identity,
                            );
                            let pending_page = pending_bookmark_jump
                                .as_ref()
                                .filter(|pending| pending.book_id == source.book_id())
                                .map(|pending| pending.page);
                            let page_index = selected_open_page(
                                source.as_ref(),
                                explicit_page,
                                *forced_page,
                                reading_position.as_ref(),
                                pending_page,
                            );
                            let started = Instant::now();
                            let seeded = prepare_seeded_first_page(
                                source.as_ref(),
                                page_index,
                                seed_target_long_edge,
                                decode,
                                false,
                                seed_target_view,
                            );
                            perf::record_startup_seed_prepare(
                                started,
                                origin.perf_label(),
                                page_index,
                                seed_target_long_edge,
                                seeded.is_some(),
                            );
                            seeded
                        });
                        let _ = tx.send(LoaderEvent {
                            generation,
                            path: load_path,
                            origin,
                            initial_direction,
                            view_fallback,
                            explicit_page,
                            failure_action,
                            result,
                            seeded_page,
                        });
                        ctx.request_repaint();
                    });
                match spawn_result {
                    Ok(_) => {
                        self.loader_pending = true;
                    }
                    Err(error) => {
                        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
                        {
                            self.open_to_first_visible_trace = None;
                        }
                        self.sibling_book_visual_pending = false;
                        self.clear_pending_sibling_book_turns();
                        self.handle_open_failure(
                            format!("Could not start source loader: {error}"),
                            failure_action,
                        );
                    }
                }
            }
            SourceKind::UnsupportedRar => {
                self.handle_open_failure(
                    "CBR/RAR requires the restricted read-only archive backend before it can be opened."
                        .to_owned(),
                    failure_action,
                );
            }
            SourceKind::Unsupported => {
                let extension = path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .unwrap_or_default();
                self.handle_open_failure(
                    unsupported_message_for_extension(extension)
                        .unwrap_or_else(|| format!("Unsupported file type: {}", path.display())),
                    failure_action,
                );
            }
        }
    }

    pub(in crate::app) fn drain_loader_events(&mut self) {
        while let Ok(event) = self.loader_rx.try_recv() {
            if event.generation != self.loader_generation {
                continue;
            }
            self.loader_pending = false;

            match event.result {
                Ok((source, forced_page)) => self.install_source(
                    source,
                    forced_page,
                    event.origin,
                    event.path,
                    event.seeded_page,
                    event.initial_direction,
                    event.view_fallback,
                    event.explicit_page,
                ),
                Err(message) => {
                    if let Some(retry) = self.sibling_open_retry.take() {
                        if retry.attempts_left > 0 {
                            if let Some(next) = sibling_book_path(&event.path, retry.direction) {
                                let full_circle = retry
                                    .origin_book
                                    .as_ref()
                                    .is_some_and(|origin| same_path(&next, origin));
                                if !full_circle {
                                    let skipped = event
                                        .path
                                        .file_name()
                                        .map(|name| name.to_string_lossy().into_owned())
                                        .unwrap_or_default();
                                    self.set_status(self.i18n().with_vars(
                                        "status.sibling_book_skipped",
                                        &[("name", skipped)],
                                    ));
                                    self.sibling_open_retry = Some(SiblingOpenRetry {
                                        attempts_left: retry.attempts_left - 1,
                                        ..retry
                                    });
                                    self.open_sibling_path_with_initial_direction(
                                        next,
                                        event.initial_direction,
                                    );
                                    continue;
                                }
                            }
                        }
                        // exhausted / no candidate / full circle: state stays cleared
                        // (take()) and control falls through to the failure handling below.
                    }
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
                    self.sibling_book_visual_pending = false;
                    self.clear_pending_sibling_book_turns();
                    self.handle_open_failure(
                        format!("Could not open {}: {message}", event.path.display()),
                        event.failure_action,
                    );
                }
            }
        }
    }

    fn handle_open_failure(&mut self, message: String, failure_action: OpenFailureAction) {
        match failure_action {
            OpenFailureAction::KeepCurrent => self.notify(message),
            OpenFailureAction::ClearCurrent => self.clear_local_book_state(message),
        }
    }

    // established call surface; a params struct would be pure boilerplate
    #[allow(clippy::too_many_arguments)]
    pub(in crate::app) fn install_source(
        &mut self,
        source: SharedSource,
        forced_page: Option<usize>,
        origin: OpenOrigin,
        opened_path: PathBuf,
        seeded_page: Option<SeededPreparedPage>,
        initial_direction: NavigationDirection,
        view_fallback: Option<OpenViewFallback>,
        explicit_page: Option<usize>,
    ) {
        self.sibling_open_retry = None;
        let book_id = source.book_id().to_owned();
        let page_count = source.page_count();
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        perf::arm_open_to_first_visible(&mut self.open_to_first_visible_trace, &book_id);
        let reading_position = reading_position_for_open(
            &self.store,
            source.as_ref(),
            origin,
            &opened_path,
            self.settings.resume_by_file_identity,
        );
        let resolved_view = resolve_open_view(
            reading_position.as_ref(),
            view_fallback,
            self.settings.remember_zoom_per_book,
        );
        self.reading_direction = resolved_view.reading_direction;
        self.view_mode = self
            .view_mode
            .with_reading_direction(self.reading_direction);
        self.fit_mode = resolved_view.fit_mode;
        self.manual_zoom = resolved_view.manual_zoom;

        let pending_page = self
            .pending_bookmark_jump
            .as_ref()
            .filter(|pending| pending.book_id == book_id)
            .map(|pending| pending.page);
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        let forced_page = forced_page.or_else(perf::forced_start_page_index);
        self.current_page = selected_open_page(
            source.as_ref(),
            explicit_page,
            forced_page,
            reading_position.as_ref(),
            pending_page,
        );
        let clear_pending_bookmark_jump = pending_page.is_some()
            || self
                .pending_bookmark_jump
                .as_ref()
                .is_some_and(|pending| pending.path == opened_path);
        if clear_pending_bookmark_jump {
            self.pending_bookmark_jump = None;
        }
        self.book_id = Some(book_id.clone());
        self.source = Some(source.clone());
        self.open_origin = Some(origin);
        self.opened_path = Some(opened_path);
        self.sibling_book_visual_pending = true;
        self.sibling_book_wgpu_present_wait = None;
        self.sibling_book_visual_hold_until = None;
        self.pan = Vec2::ZERO;
        self.effects = ViewEffects::default();
        self.current_view_state = None;
        self.deferred_worker_events.clear();
        self.decoded_pages.clear();
        self.decoded_bytes = 0;
        self.page_metrics.clear();
        self.textures.clear();
        self.clear_upscale_probe_state();
        self.seed_upscale_decision_from_record(&book_id);
        if let Some(thumbnails) = self.bookmark_thumbnails.as_mut() {
            thumbnails.clear();
        }
        self.page_errors.clear();
        self.edge_prompt = None;
        self.transition = None;
        self.clear_pending_page_turns();
        self.last_nav_direction = initial_direction;
        self.view_target_settle = ViewTargetSettle::default();
        self.target_long_edge = seeded_page
            .as_ref()
            .map_or(PREVIEW_TARGET_LONG_EDGE, |seed| seed.key.target_long_edge);
        self.insert_seeded_page_if_current(seeded_page);
        self.worker.load_book(
            source.clone(),
            self.worker_center_page(),
            self.last_nav_direction,
            self.target_long_edge,
            self.visible_page_count(),
            self.worker_options(),
        );
        self.set_status(self.i18n().with_vars(
            "status.opened",
            &[
                ("title", source.title().to_owned()),
                ("page", (self.current_page + 1).to_string()),
                ("count", page_count.to_string()),
            ],
        ));
        self.persist_reading_position();
        if self.queued_sibling_book_turns.is_empty() {
            self.request_adjacent_seed_prefetch();
        }
    }

    pub(in crate::app) fn open_view_fallback(&self) -> OpenViewFallback {
        OpenViewFallback {
            reading_direction: self.reading_direction,
            fit_mode: self.fit_mode,
            manual_zoom: self.manual_zoom,
        }
    }

    pub(in crate::app) fn seed_target_view_for_open(
        &self,
        view_fallback: Option<OpenViewFallback>,
    ) -> Option<SeedTargetView> {
        let viewport = self.last_viewer_size_points?;
        let page_viewport = if self.view_mode.step() <= 1 {
            viewport
        } else {
            Vec2::new((viewport.x - SPREAD_GAP_POINTS).max(1.0) * 0.5, viewport.y)
        };
        let fallback = view_fallback.unwrap_or_else(|| self.open_view_fallback());
        Some(SeedTargetView {
            fit_mode: fallback.fit_mode,
            manual_zoom: fallback.manual_zoom,
            page_viewport,
            pixels_per_point: self.egui_ctx.pixels_per_point(),
        })
    }
}

pub(in crate::app) fn bookmark_path_for_open<'a>(
    origin: OpenOrigin,
    opened_path: &'a Path,
    source: &'a dyn BookSource,
) -> &'a Path {
    if origin == OpenOrigin::SingleImage {
        opened_path
    } else {
        source.source_path()
    }
}

pub(in crate::app) fn open_origin_for_source_kind(kind: SourceKind) -> Option<OpenOrigin> {
    match kind {
        SourceKind::Folder => Some(OpenOrigin::Folder),
        SourceKind::ZipCbz => Some(OpenOrigin::ZipCbz),
        SourceKind::SingleImage => Some(OpenOrigin::SingleImage),
        SourceKind::Unsupported | SourceKind::UnsupportedRar => None,
    }
}

fn startup_seed_target_long_edge(placement: &WindowPlacement) -> u32 {
    let stored_edge = placement
        .inner_size
        .map(|[width, height]| width.max(height).ceil().max(1.0) as u32)
        .unwrap_or(DEFAULT_TARGET_LONG_EDGE);
    let estimated_edge = if placement.maximized {
        stored_edge.max(2304)
    } else {
        stored_edge.max(DEFAULT_TARGET_LONG_EDGE)
    };
    let quantized = estimated_edge.div_ceil(256) * 256;
    clamp_navigation_target_long_edge(quantized)
}

fn open_seed_target_long_edge(current_target_long_edge: u32) -> u32 {
    clamp_navigation_target_long_edge(current_target_long_edge)
}

fn startup_decode_options(settings: &AppSettings) -> DecodeOptions {
    let strategy = match settings.decode_mode {
        DecodeMode::AutoFast => DecodeStrategy::Auto,
        DecodeMode::Compatibility => DecodeStrategy::ImageCrate,
        DecodeMode::Custom => DecodeStrategy::Auto,
    };
    let decoder_preferences = if matches!(settings.decode_mode, DecodeMode::Custom) {
        settings.decoder_preferences
    } else {
        DecoderPreferences::default()
    };
    DecodeOptions {
        strategy,
        decoder_preferences,
        fast_sampled_scaled_decode: settings.fast_sampled_scaled_decode,
        cpu_upscale_filter: settings.cpu_upscale_filter,
        cpu_downscale_filter: settings.cpu_downscale_filter,
        allow_display_upscale: false,
        apply_exif_orientation: settings.apply_exif_orientation,
        apply_embedded_icc: settings.apply_embedded_icc,
    }
}

pub(in crate::app) fn reading_position_for_open(
    store: &StateStore,
    source: &dyn BookSource,
    origin: OpenOrigin,
    opened_path: &Path,
    resume_by_file_identity: bool,
) -> Option<crate::core::state::ReadingPosition> {
    let bookmark_path = bookmark_path_for_open(origin, opened_path, source);
    store.reading_position(source.book_id(), bookmark_path, resume_by_file_identity)
}

pub(in crate::app) fn selected_open_page(
    source: &dyn BookSource,
    explicit_page: Option<usize>,
    forced_page: Option<usize>,
    reading_position: Option<&crate::core::state::ReadingPosition>,
    pending_page: Option<usize>,
) -> usize {
    let page_count = source.page_count();
    if page_count == 0 {
        return 0;
    }
    let bookmarked_page = reading_position.and_then(|position| {
        position
            .last_page_name
            .as_deref()
            .and_then(|page_name| page_index_for_name(source, page_name))
            .or(Some(position.last_page))
    });
    explicit_page
        .or(pending_page)
        .or(forced_page)
        .or(bookmarked_page)
        .unwrap_or_default()
        .min(page_count.saturating_sub(1))
}

pub(in crate::app) fn page_index_for_name(
    source: &dyn BookSource,
    page_name: &str,
) -> Option<usize> {
    (0..source.page_count()).find(|index| source.page_name(*index) == Some(page_name))
}

fn resolve_open_view(
    reading_position: Option<&ReadingPosition>,
    view_fallback: Option<OpenViewFallback>,
    remember_zoom_per_book: bool,
) -> ResolvedOpenView {
    match reading_position {
        Some(position) => ResolvedOpenView {
            reading_direction: position.reading_direction,
            fit_mode: position.fit_mode,
            manual_zoom: if remember_zoom_per_book {
                position.manual_zoom.unwrap_or(1.0)
            } else {
                1.0
            },
        },
        None => view_fallback.map_or_else(
            || ResolvedOpenView {
                reading_direction: ReadingDirection::default(),
                fit_mode: FitMode::default(),
                manual_zoom: 1.0,
            },
            |fallback| ResolvedOpenView {
                reading_direction: fallback.reading_direction,
                fit_mode: fallback.fit_mode,
                manual_zoom: fallback.manual_zoom,
            },
        ),
    }
}

#[cfg(test)]
mod tests;
