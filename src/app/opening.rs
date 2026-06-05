use super::{
    adjacent_seed::prepare_seeded_first_page, perf, PendingBookmarkJump, SeededPreparedPage,
    SuiSuiViewApp,
};
use crate::core::effects::ViewEffects;
use crate::core::formats::unsupported_message_for_extension;
use crate::core::source::{
    classify_path, open_source_from_path, BookSource, SharedSource, SourceKind,
};
use crate::core::state::{
    AppSettings, DecodeMode, DecoderPreferences, StateStore, WindowPlacement,
};
use crate::core::worker::{
    clamp_navigation_target_long_edge, DecodeOptions, DecodeStrategy, NavigationDirection,
    DEFAULT_TARGET_LONG_EDGE, PREVIEW_TARGET_LONG_EDGE,
};
use crossbeam_channel::{unbounded, Receiver, Sender};
use eframe::egui::Vec2;
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
    pub(in crate::app) result: Result<(SharedSource, Option<usize>), String>,
    pub(in crate::app) seeded_page: Option<SeededPreparedPage>,
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
                    true,
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
        self.open_path_with_initial_direction(path, NavigationDirection::Forward);
    }

    pub(in crate::app) fn open_path_with_initial_direction(
        &mut self,
        path: PathBuf,
        initial_direction: NavigationDirection,
    ) {
        self.pending_bookmark_jump = None;
        self.clear_adjacent_seed_cache();
        self.open_path_inner(path, initial_direction);
    }

    pub(in crate::app) fn open_path_for_bookmark(
        &mut self,
        path: PathBuf,
        book_id: String,
        page: usize,
    ) {
        self.pending_bookmark_jump = Some(PendingBookmarkJump {
            book_id,
            path: path.clone(),
            page,
        });
        self.clear_adjacent_seed_cache();
        self.open_path_inner(path, NavigationDirection::Forward);
    }

    pub(in crate::app) fn open_path_inner(
        &mut self,
        path: PathBuf,
        initial_direction: NavigationDirection,
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
                self.set_status(self.i18n().text("status.opening"));

                let spawn_result = thread::Builder::new()
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
                            initial_direction,
                            result,
                            seeded_page: None,
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
                        self.notify(format!("Could not start source loader: {error}"));
                    }
                }
            }
            SourceKind::UnsupportedRar => {
                self.notify(
                    "CBR/RAR requires the restricted read-only archive backend before it can be opened.",
                );
            }
            SourceKind::Unsupported => {
                let extension = path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .unwrap_or_default();
                self.notify(
                    unsupported_message_for_extension(extension)
                        .unwrap_or_else(|| format!("Unsupported file type: {}", path.display())),
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
                ),
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
                    self.notify(format!(
                        "Could not open {}: {message}",
                        event.path.display()
                    ));
                }
            }
        }
    }

    pub(in crate::app) fn install_source(
        &mut self,
        source: SharedSource,
        forced_page: Option<usize>,
        origin: OpenOrigin,
        opened_path: PathBuf,
        seeded_page: Option<SeededPreparedPage>,
        initial_direction: NavigationDirection,
    ) {
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
        self.reading_direction = reading_position
            .as_ref()
            .map(|position| position.reading_direction)
            .unwrap_or_default();
        self.view_mode = self
            .view_mode
            .with_reading_direction(self.reading_direction);
        self.fit_mode = reading_position
            .as_ref()
            .map(|position| position.fit_mode)
            .unwrap_or_default();
        self.manual_zoom = reading_position
            .as_ref()
            .and_then(|position| position.manual_zoom)
            .filter(|_| self.settings.remember_zoom_per_book)
            .unwrap_or(1.0);

        let pending_page = self
            .pending_bookmark_jump
            .as_ref()
            .filter(|pending| pending.book_id == book_id)
            .map(|pending| pending.page);
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        let forced_page = forced_page.or_else(perf::forced_start_page_index);
        self.current_page = selected_open_page(
            source.as_ref(),
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
        self.pan = Vec2::ZERO;
        self.effects = ViewEffects::default();
        self.decoded_pages.clear();
        self.decoded_bytes = 0;
        self.page_metrics.clear();
        self.upscaled_pages.clear();
        self.upscaled_bytes = 0;
        self.textures.clear();
        self.clear_auto_kind_state();
        if let Some(thumbnails) = self.bookmark_thumbnails.as_mut() {
            thumbnails.clear();
        }
        self.page_errors.clear();
        self.upscale_generation = self.upscale_generation.wrapping_add(1);
        self.upscale_inflight = None;
        self.ai_upscale_queue.clear();
        self.ai_upscale_manual_requests.clear();
        self.ai_upscale_failures.clear();
        self.edge_prompt = None;
        self.transition = None;
        self.clear_pending_page_turns();
        self.last_nav_direction = initial_direction;
        self.pending_target_long_edge_increase = None;
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
        self.persist_current_bookmark();
        self.refresh_ai_prefetch_queue();
        self.request_adjacent_seed_prefetch();
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
    pending_page
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

#[cfg(test)]
mod tests {
    use super::startup_seed_target_long_edge;
    use crate::core::state::WindowPlacement;
    use crate::core::worker::DEFAULT_TARGET_LONG_EDGE;

    #[test]
    fn startup_seed_target_keeps_default_floor_for_normal_windows() {
        let placement = WindowPlacement {
            inner_size: Some([1280.0, 820.0]),
            outer_position: None,
            maximized: false,
        };

        assert_eq!(
            startup_seed_target_long_edge(&placement),
            DEFAULT_TARGET_LONG_EDGE
        );
    }

    #[test]
    fn startup_seed_target_uses_larger_floor_for_maximized_windows() {
        let placement = WindowPlacement {
            inner_size: Some([1280.0, 820.0]),
            outer_position: None,
            maximized: true,
        };

        assert_eq!(startup_seed_target_long_edge(&placement), 2304);
    }

    #[test]
    fn startup_seed_target_uses_default_without_stored_size() {
        let placement = WindowPlacement {
            inner_size: None,
            outer_position: None,
            maximized: false,
        };

        assert_eq!(
            startup_seed_target_long_edge(&placement),
            DEFAULT_TARGET_LONG_EDGE
        );
    }
}
