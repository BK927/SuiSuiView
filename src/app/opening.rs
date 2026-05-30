use super::{perf, PageCacheKey, PageMetrics, PendingBookmarkJump, SuiSuiViewApp};
use crate::core::effects::ViewEffects;
use crate::core::formats::unsupported_message_for_extension;
use crate::core::natural::cmp_natural;
use crate::core::source::{
    classify_path, open_source_from_path, BookSource, SharedSource, SourceKind,
};
use crate::core::state::StateStore;
use crate::core::worker::{
    prepare_image_with_options, DecodeOptions, NavigationDirection, PreparedPage,
    MAX_TARGET_LONG_EDGE, PREVIEW_TARGET_LONG_EDGE,
};
use eframe::egui::Vec2;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

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
    pub(in crate::app) result: Result<(SharedSource, Option<usize>), String>,
}

pub(in crate::app) struct SeededPreparedPage {
    pub(in crate::app) index: usize,
    pub(in crate::app) key: PageCacheKey,
    pub(in crate::app) page: Arc<PreparedPage>,
}

pub(in crate::app) struct AdjacentSeedEvent {
    pub(in crate::app) generation: u64,
    pub(in crate::app) cache: Option<AdjacentSeedCache>,
}

pub(in crate::app) struct AdjacentSeedCache {
    pub(in crate::app) path: PathBuf,
    pub(in crate::app) direction: isize,
    pub(in crate::app) origin: OpenOrigin,
    pub(in crate::app) source: SharedSource,
    pub(in crate::app) forced_page: Option<usize>,
    pub(in crate::app) target_long_edge: u32,
    pub(in crate::app) decode: DecodeOptions,
    pub(in crate::app) seeded_page: SeededPreparedPage,
}

impl SuiSuiViewApp {
    pub(in crate::app) fn open_path(&mut self, path: PathBuf) {
        self.pending_bookmark_jump = None;
        self.clear_adjacent_seed_cache();
        self.open_path_inner(path);
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
        self.open_path_inner(path);
    }

    pub(in crate::app) fn open_path_inner(&mut self, path: PathBuf) {
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
                self.set_status("파일을 여는 중입니다...");

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

            match event.result {
                Ok((source, forced_page)) => {
                    self.install_source(source, forced_page, event.origin, event.path, None)
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
        self.bookmark_thumbnails.clear();
        self.page_errors.clear();
        self.upscale_generation = self.upscale_generation.wrapping_add(1);
        self.upscale_inflight = None;
        self.ai_upscale_queue.clear();
        self.ai_upscale_manual_requests.clear();
        self.ai_upscale_failures.clear();
        self.edge_prompt = None;
        self.transition = None;
        self.clear_pending_page_turns();
        self.last_nav_direction = NavigationDirection::Forward;
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
        self.set_status(format!(
            "열림: {} [{} / {}]",
            source.title(),
            self.current_page + 1,
            page_count
        ));
        self.persist_current_bookmark();
        self.refresh_ai_prefetch_queue();
        self.request_adjacent_seed_prefetch();
    }

    pub(in crate::app) fn insert_seeded_page_if_current(
        &mut self,
        seeded_page: Option<SeededPreparedPage>,
    ) {
        let Some(seed) = seeded_page else {
            return;
        };
        if seed.index != self.current_page {
            return;
        }
        if seed.key.target_long_edge != self.target_long_edge
            || seed.key.decode != self.decode_options()
        {
            return;
        }
        self.page_metrics
            .insert(seed.index, PageMetrics::from_page(&seed.page));
        self.insert_prepared_page(seed.key, seed.page);
        self.prune_decoded_cache();
    }

    pub(in crate::app) fn drain_adjacent_seed_events(&mut self) {
        let mut dropped = Vec::new();
        while let Ok(event) = self.adjacent_seed_rx.try_recv() {
            if event.generation != self.adjacent_seed_generation {
                if let Some(cache) = event.cache {
                    dropped.push(cache);
                }
                continue;
            }
            let Some(cache) = event.cache else {
                continue;
            };

            let mut retained = Vec::with_capacity(self.adjacent_seed_cache.len());
            for cached in self.adjacent_seed_cache.drain(..) {
                if cached.direction == cache.direction {
                    dropped.push(cached);
                } else {
                    retained.push(cached);
                }
            }
            self.adjacent_seed_cache = retained;
            self.adjacent_seed_cache.push(cache);
            if self.adjacent_seed_cache.len() > 2 {
                dropped.push(self.adjacent_seed_cache.remove(0));
            }
        }
        drop_adjacent_seed_caches_off_thread(dropped);
    }

    pub(in crate::app) fn clear_adjacent_seed_cache(&mut self) {
        self.adjacent_seed_generation = self.adjacent_seed_generation.wrapping_add(1);
        self.adjacent_seed_generation_token
            .store(self.adjacent_seed_generation, Ordering::Relaxed);
        self.pending_adjacent_seed_prefetch_at = None;
        drop_adjacent_seed_caches_off_thread(std::mem::take(&mut self.adjacent_seed_cache));
    }

    pub(in crate::app) fn request_adjacent_seed_prefetch(&mut self) {
        if !perf::adjacent_seed_prefetch_enabled() || self.source.is_none() {
            return;
        }
        if self.target_long_edge > MAX_TARGET_LONG_EDGE {
            self.clear_adjacent_seed_cache();
            return;
        }
        if self.pending_adjacent_seed_prefetch_at.is_none() {
            self.egui_ctx
                .request_repaint_after(Duration::from_millis(1));
        }
        self.pending_adjacent_seed_prefetch_at = Some(Instant::now() + Duration::from_millis(1));
    }

    pub(in crate::app) fn run_pending_adjacent_seed_prefetch(&mut self) {
        let Some(schedule_at) = self.pending_adjacent_seed_prefetch_at else {
            return;
        };
        let now = Instant::now();
        if now < schedule_at {
            self.egui_ctx.request_repaint_after(schedule_at - now);
            return;
        }
        self.pending_adjacent_seed_prefetch_at = None;
        self.schedule_adjacent_seed_prefetches();
    }

    pub(in crate::app) fn schedule_adjacent_seed_prefetches(&mut self) {
        if !perf::adjacent_seed_prefetch_enabled() {
            return;
        }
        let Some(current) = self.current_book_reference_path() else {
            return;
        };

        self.adjacent_seed_generation = self.adjacent_seed_generation.wrapping_add(1);
        self.adjacent_seed_generation_token
            .store(self.adjacent_seed_generation, Ordering::Relaxed);
        drop_adjacent_seed_caches_off_thread(std::mem::take(&mut self.adjacent_seed_cache));
        let generation = self.adjacent_seed_generation;
        let generation_token = self.adjacent_seed_generation_token.clone();
        let target_long_edge = self.target_long_edge;
        let decode = self.decode_options();
        let store = self.store.clone();
        let resume_by_file_identity = self.settings.resume_by_file_identity;
        let tx = self.adjacent_seed_tx.clone();
        let ctx = self.egui_ctx.clone();

        let _ = thread::Builder::new()
            .name("suisuiview-adjacent-seed".to_owned())
            .spawn(move || {
                for (path, direction, label) in adjacent_sibling_book_paths(&current) {
                    if !adjacent_seed_generation_matches(&generation_token, generation) {
                        break;
                    }
                    let Some(origin) = open_origin_for_source_kind(classify_path(&path)) else {
                        continue;
                    };
                    let started = Instant::now();
                    let cache = prepare_adjacent_seed_cache(
                        path,
                        direction,
                        origin,
                        target_long_edge,
                        decode,
                        &store,
                        resume_by_file_identity,
                        &generation_token,
                        generation,
                    );
                    perf::record_adjacent_seed_prefetch_prepare(
                        started,
                        origin.perf_label(),
                        label,
                        cache.as_ref().map_or(0, |cache| cache.seeded_page.index),
                        target_long_edge,
                        cache.is_some(),
                    );
                    let _ = tx.send(AdjacentSeedEvent { generation, cache });
                    ctx.request_repaint();
                }
            });
    }

    pub(in crate::app) fn take_adjacent_seed_for_direction(
        &mut self,
        direction: isize,
    ) -> Option<AdjacentSeedCache> {
        if !perf::adjacent_seed_prefetch_enabled() {
            return None;
        }
        let position = self
            .adjacent_seed_cache
            .iter()
            .position(|cache| cache.direction == direction.signum())?;
        let mut caches = std::mem::take(&mut self.adjacent_seed_cache);
        let cache = caches.remove(position);
        drop_adjacent_seed_caches_off_thread(caches);
        if cache.target_long_edge != self.target_long_edge || cache.decode != self.decode_options()
        {
            drop_adjacent_seed_caches_off_thread(vec![cache]);
            return None;
        }

        let reading_position = reading_position_for_open(
            &self.store,
            cache.source.as_ref(),
            cache.origin,
            &cache.path,
            self.settings.resume_by_file_identity,
        );
        let selected_page = selected_open_page(
            cache.source.as_ref(),
            cache.forced_page,
            reading_position.as_ref(),
            None,
        );
        if selected_page == cache.seeded_page.index {
            Some(cache)
        } else {
            drop_adjacent_seed_caches_off_thread(vec![cache]);
            None
        }
    }
}

pub(in crate::app) fn sibling_book_path(current: &Path, direction: isize) -> Option<PathBuf> {
    let entries = sibling_book_entries(current)?;
    if entries.len() <= 1 {
        return None;
    }
    let current_index = sibling_book_current_index(&entries, current);
    let next_index = if direction >= 0 {
        (current_index + 1) % entries.len()
    } else {
        (current_index + entries.len() - 1) % entries.len()
    };
    Some(entries[next_index].clone())
}

pub(in crate::app) fn adjacent_sibling_book_paths(
    current: &Path,
) -> Vec<(PathBuf, isize, &'static str)> {
    let Some(entries) = sibling_book_entries(current) else {
        return Vec::new();
    };
    if entries.len() <= 1 {
        return Vec::new();
    }
    let current_index = sibling_book_current_index(&entries, current);
    let mut siblings = Vec::with_capacity(2);
    for (direction, label) in [(1, "next"), (-1, "previous")] {
        let index = if direction >= 0 {
            (current_index + 1) % entries.len()
        } else {
            (current_index + entries.len() - 1) % entries.len()
        };
        let path = entries[index].clone();
        if siblings
            .iter()
            .any(|(existing, _, _): &(PathBuf, isize, &'static str)| same_path(existing, &path))
        {
            continue;
        }
        siblings.push((path, direction, label));
    }
    siblings
}

pub(in crate::app) fn sibling_book_entries(current: &Path) -> Option<Vec<PathBuf>> {
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
    Some(entries)
}

pub(in crate::app) fn sibling_book_current_index(entries: &[PathBuf], current: &Path) -> usize {
    entries
        .iter()
        .position(|path| same_path(path, current))
        .unwrap_or_else(|| {
            entries
                .iter()
                .position(|path| path == current)
                .unwrap_or_default()
        })
}

pub(in crate::app) fn same_path(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
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

pub(in crate::app) fn prepare_seeded_first_page(
    source: &dyn BookSource,
    index: usize,
    target_long_edge: u32,
    decode: DecodeOptions,
) -> Option<SeededPreparedPage> {
    let page_count = source.page_count();
    if page_count == 0 {
        return None;
    }
    let index = index.min(page_count - 1);
    let bytes = source.read_page(index).ok()?;
    let page = Arc::new(prepare_image_with_options(&bytes, target_long_edge, decode).ok()?);
    Some(SeededPreparedPage {
        index,
        key: PageCacheKey {
            index,
            target_long_edge,
            decode,
        },
        page,
    })
}

pub(in crate::app) fn prepare_adjacent_seed_cache(
    path: PathBuf,
    direction: isize,
    origin: OpenOrigin,
    target_long_edge: u32,
    decode: DecodeOptions,
    store: &StateStore,
    resume_by_file_identity: bool,
    generation_token: &AtomicU64,
    generation: u64,
) -> Option<AdjacentSeedCache> {
    if !adjacent_seed_generation_matches(generation_token, generation) {
        return None;
    }
    let (source, forced_page) = open_source_from_path(&path).ok()?;
    if !adjacent_seed_generation_matches(generation_token, generation) {
        return None;
    }
    let reading_position = reading_position_for_open(
        store,
        source.as_ref(),
        origin,
        &path,
        resume_by_file_identity,
    );
    let seed_page = selected_open_page(
        source.as_ref(),
        forced_page,
        reading_position.as_ref(),
        None,
    );
    if !adjacent_seed_generation_matches(generation_token, generation) {
        return None;
    }
    let seeded_page =
        prepare_seeded_first_page(source.as_ref(), seed_page, target_long_edge, decode)?;
    if !adjacent_seed_generation_matches(generation_token, generation) {
        return None;
    }

    Some(AdjacentSeedCache {
        path,
        direction,
        origin,
        source,
        forced_page,
        target_long_edge,
        decode,
        seeded_page,
    })
}

pub(in crate::app) fn adjacent_seed_generation_matches(
    generation_token: &AtomicU64,
    generation: u64,
) -> bool {
    generation_token.load(Ordering::Relaxed) == generation
}

pub(in crate::app) fn drop_adjacent_seed_caches_off_thread(caches: Vec<AdjacentSeedCache>) {
    if caches.is_empty() {
        return;
    }
    let _ = thread::Builder::new()
        .name("suisuiview-adjacent-seed-drop".to_owned())
        .spawn(move || drop(caches));
}

pub(in crate::app) fn page_index_for_name(
    source: &dyn BookSource,
    page_name: &str,
) -> Option<usize> {
    (0..source.page_count()).find(|index| source.page_name(*index) == Some(page_name))
}
