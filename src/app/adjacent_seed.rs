use super::{
    adjacent_sibling_book_paths_ordered, image_header,
    opening::{open_origin_for_source_kind, reading_position_for_open, selected_open_page},
    perf,
    viewer::{target_long_edge_for_view, OriginalPageSize},
    OpenOrigin, PageCacheKey, PageMetrics, SuiSuiViewApp,
};
use crate::core::source::{classify_path, open_source_from_path, BookSource, SharedSource};
use crate::core::state::{FitMode, StateStore};
use crate::core::worker::{
    prepare_image_with_options, DecodeOptions, NavigationDirection, PreparedPage,
    MAX_TARGET_LONG_EDGE,
};
use eframe::egui::Vec2;
use image::ImageReader;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Instant;

const ADJACENT_SEED_LARGE_SOURCE_BYTES: u64 = 4 * 1024 * 1024;
const ADJACENT_SEED_HEADER_BYTES: usize = 1024 * 1024;
const ADJACENT_SEED_LARGE_BOOK_BYTES: u64 = 128 * 1024 * 1024;
const ADJACENT_SEED_LARGE_SOURCE_LONG_EDGE: u32 = 8192;
const ADJACENT_SEED_FOLLOWUP_MAX_TARGET_LONG_EDGE: u32 = 2048;
const ADJACENT_SEED_FOLLOWUP_MAX_BYTES: usize = 20 * 1024 * 1024;

#[derive(Clone)]
pub(in crate::app) struct SeededPreparedPage {
    pub(in crate::app) index: usize,
    pub(in crate::app) key: PageCacheKey,
    pub(in crate::app) page: Arc<PreparedPage>,
}

#[derive(Clone, Copy)]
pub(in crate::app) struct SeedTargetView {
    pub(in crate::app) fit_mode: FitMode,
    pub(in crate::app) manual_zoom: f32,
    pub(in crate::app) page_viewport: Vec2,
    pub(in crate::app) pixels_per_point: f32,
}

pub(in crate::app) struct AdjacentSeedEvent {
    pub(in crate::app) generation: u64,
    pub(in crate::app) cache: Option<AdjacentSeedCache>,
}

#[derive(Clone)]
pub(in crate::app) struct AdjacentSeedCache {
    pub(in crate::app) base_path: PathBuf,
    pub(in crate::app) path: PathBuf,
    pub(in crate::app) direction: isize,
    pub(in crate::app) origin: OpenOrigin,
    pub(in crate::app) source: SharedSource,
    pub(in crate::app) forced_page: Option<usize>,
    pub(in crate::app) target_long_edge: u32,
    pub(in crate::app) decode: DecodeOptions,
    pub(in crate::app) seeded_page: SeededPreparedPage,
    pub(in crate::app) seeded_followup_page: Option<SeededPreparedPage>,
}

impl SuiSuiViewApp {
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
        self.insert_seeded_page_if_matching_target(Some(seed));
    }

    pub(in crate::app) fn insert_seeded_page_if_matching_target(
        &mut self,
        seeded_page: Option<SeededPreparedPage>,
    ) {
        let Some(seed) = seeded_page else {
            return;
        };
        if seed.key.target_long_edge != self.target_long_edge
            || seed.key.decode != self.decode_options()
        {
            return;
        }
        let Some(source) = self.source.as_ref() else {
            return;
        };
        if seed.index >= source.page_count() {
            return;
        }
        self.page_metrics
            .insert(seed.index, PageMetrics::from_page(&seed.page));
        self.insert_prepared_page(seed.key, seed.page);
        self.prune_decoded_cache();
    }

    pub(in crate::app) fn drain_adjacent_seed_events(&mut self) {
        let mut dropped = Vec::new();
        let current = self.current_book_reference_path();
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
            if current
                .as_deref()
                .is_some_and(|current| cache.base_path != current)
            {
                dropped.push(cache);
                continue;
            }

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
            self.egui_ctx.request_repaint();
        }
        self.pending_adjacent_seed_prefetch_at = Some(Instant::now());
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
        let target_long_edge = self.target_long_edge;
        let decode = self.decode_options();
        self.retain_adjacent_seed_caches_for_current(&current, target_long_edge, decode);

        self.adjacent_seed_generation = self.adjacent_seed_generation.wrapping_add(1);
        self.adjacent_seed_generation_token
            .store(self.adjacent_seed_generation, Ordering::Relaxed);
        let generation = self.adjacent_seed_generation;
        let generation_token = self.adjacent_seed_generation_token.clone();
        let store = self.store.clone();
        let resume_by_file_identity = self.settings.resume_by_file_identity;
        let large_source_guard = perf::adjacent_seed_memory_guard_enabled();
        let seed_target_view = self.seed_target_view_for_open(None);
        let tx = self.adjacent_seed_tx.clone();
        let seed_order = self.last_nav_direction;
        let ctx = self.egui_ctx.clone();

        let _ = thread::Builder::new()
            .name("suisuiview-adjacent-seed".to_owned())
            .spawn(move || {
                let primary_direction = match seed_order {
                    NavigationDirection::Forward => 1,
                    NavigationDirection::Backward => -1,
                };
                let mut followup_candidate = None;

                for (path, direction, label) in
                    adjacent_sibling_book_paths_ordered(&current, seed_order)
                {
                    if !adjacent_seed_generation_matches(&generation_token, generation) {
                        break;
                    }
                    let Some(origin) = open_origin_for_source_kind(classify_path(&path)) else {
                        continue;
                    };
                    let started = Instant::now();
                    let cache = prepare_adjacent_seed_cache(
                        current.clone(),
                        path,
                        direction,
                        origin,
                        target_long_edge,
                        decode,
                        &store,
                        resume_by_file_identity,
                        &generation_token,
                        generation,
                        large_source_guard,
                        seed_target_view,
                    );
                    let success = cache.is_some();
                    perf::record_adjacent_seed_prefetch_prepare(
                        started,
                        origin.perf_label(),
                        label,
                        cache.as_ref().map_or(0, |cache| cache.seeded_page.index),
                        target_long_edge,
                        success,
                    );
                    let Some(cache) = cache else {
                        continue;
                    };
                    if direction == primary_direction && followup_candidate.is_none() {
                        let _ = tx.send(AdjacentSeedEvent {
                            generation,
                            cache: Some(cache.clone()),
                        });
                        followup_candidate = Some((cache, origin));
                    } else {
                        let _ = tx.send(AdjacentSeedEvent {
                            generation,
                            cache: Some(cache),
                        });
                    }
                    ctx.request_repaint();
                }

                let Some((mut cache, origin)) = followup_candidate else {
                    return;
                };
                if !adjacent_seed_generation_matches(&generation_token, generation) {
                    return;
                }
                let followup_started = Instant::now();
                cache.seeded_followup_page = prepare_seeded_followup_page(
                    cache.source.as_ref(),
                    cache.seeded_page.index,
                    target_long_edge,
                    decode,
                    large_source_guard,
                    seed_target_view,
                );
                perf::record_adjacent_seed_prefetch_prepare(
                    followup_started,
                    origin.perf_label(),
                    "followup_page",
                    cache
                        .seeded_followup_page
                        .as_ref()
                        .map_or(0, |seed| seed.index),
                    target_long_edge,
                    cache.seeded_followup_page.is_some(),
                );
                if cache.seeded_followup_page.is_some()
                    && adjacent_seed_generation_matches(&generation_token, generation)
                {
                    let _ = tx.send(AdjacentSeedEvent {
                        generation,
                        cache: Some(cache),
                    });
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
        let current = self.current_book_reference_path()?;
        let position = self.adjacent_seed_cache.iter().position(|cache| {
            cache.direction == direction.signum() && cache.base_path == current
        })?;
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
            None,
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

    fn retain_adjacent_seed_caches_for_current(
        &mut self,
        current: &Path,
        target_long_edge: u32,
        decode: DecodeOptions,
    ) {
        let mut dropped = Vec::new();
        let mut retained = Vec::with_capacity(self.adjacent_seed_cache.len());
        for cache in self.adjacent_seed_cache.drain(..) {
            if cache.base_path == current
                && cache.target_long_edge == target_long_edge
                && cache.decode == decode
            {
                retained.push(cache);
            } else {
                dropped.push(cache);
            }
        }
        self.adjacent_seed_cache = retained;
        drop_adjacent_seed_caches_off_thread(dropped);
    }
}

pub(in crate::app) fn prepare_seeded_first_page(
    source: &dyn BookSource,
    index: usize,
    target_long_edge: u32,
    decode: DecodeOptions,
    large_source_guard: bool,
    target_view: Option<SeedTargetView>,
) -> Option<SeededPreparedPage> {
    let page_count = source.page_count();
    if page_count == 0 {
        return None;
    }
    let index = index.min(page_count - 1);
    if large_source_guard && should_skip_memory_aware_adjacent_seed_source(source, index) {
        return None;
    }
    let bytes = source.read_page(index).ok()?;
    if large_source_guard && should_skip_memory_aware_adjacent_seed(&bytes) {
        return None;
    }
    let target_long_edge = seed_target_long_edge_from_view(&bytes, target_long_edge, target_view);
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

pub(in crate::app) fn prepare_seeded_followup_page(
    source: &dyn BookSource,
    current_index: usize,
    target_long_edge: u32,
    decode: DecodeOptions,
    large_source_guard: bool,
    target_view: Option<SeedTargetView>,
) -> Option<SeededPreparedPage> {
    if target_long_edge > ADJACENT_SEED_FOLLOWUP_MAX_TARGET_LONG_EDGE {
        return None;
    }
    let index = current_index.checked_add(1)?;
    if index >= source.page_count() {
        return None;
    }
    let page = prepare_seeded_first_page(
        source,
        index,
        target_long_edge,
        decode,
        large_source_guard,
        target_view,
    )?;
    if page.page.byte_size > ADJACENT_SEED_FOLLOWUP_MAX_BYTES {
        return None;
    }
    Some(page)
}

pub(in crate::app) fn prepare_adjacent_seed_cache(
    base_path: PathBuf,
    path: PathBuf,
    direction: isize,
    origin: OpenOrigin,
    target_long_edge: u32,
    decode: DecodeOptions,
    store: &StateStore,
    resume_by_file_identity: bool,
    generation_token: &AtomicU64,
    generation: u64,
    large_source_guard: bool,
    target_view: Option<SeedTargetView>,
) -> Option<AdjacentSeedCache> {
    if !adjacent_seed_generation_matches(generation_token, generation) {
        return None;
    }
    if large_source_guard && should_skip_memory_aware_adjacent_seed_path(&path) {
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
        None,
        forced_page,
        reading_position.as_ref(),
        None,
    );
    if !adjacent_seed_generation_matches(generation_token, generation) {
        return None;
    }
    let seeded_page = prepare_seeded_first_page(
        source.as_ref(),
        seed_page,
        target_long_edge,
        decode,
        large_source_guard,
        target_view,
    )?;
    if !adjacent_seed_generation_matches(generation_token, generation) {
        return None;
    }

    Some(AdjacentSeedCache {
        base_path,
        path,
        direction,
        origin,
        source,
        forced_page,
        target_long_edge,
        decode,
        seeded_page,
        seeded_followup_page: None,
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

fn should_skip_memory_aware_adjacent_seed(bytes: &[u8]) -> bool {
    source_dimensions_from_bytes(bytes)
        .is_some_and(|(width, height)| width.max(height) >= ADJACENT_SEED_LARGE_SOURCE_LONG_EDGE)
}

fn should_skip_memory_aware_adjacent_seed_source(source: &dyn BookSource, index: usize) -> bool {
    let Some(byte_size) = source.page_byte_size(index) else {
        return false;
    };
    if byte_size < ADJACENT_SEED_LARGE_SOURCE_BYTES {
        return false;
    }
    let Ok(header) = source.read_page_prefix(index, ADJACENT_SEED_HEADER_BYTES) else {
        return true;
    };
    image_header::dimensions_from_header(&header).map_or(true, |(width, height)| {
        width.max(height) >= ADJACENT_SEED_LARGE_SOURCE_LONG_EDGE
    })
}

fn should_skip_memory_aware_adjacent_seed_path(path: &Path) -> bool {
    path.is_file()
        && fs::metadata(path)
            .ok()
            .is_some_and(|metadata| metadata.len() >= ADJACENT_SEED_LARGE_BOOK_BYTES)
}

fn source_dimensions_from_bytes(bytes: &[u8]) -> Option<(u32, u32)> {
    image_header::dimensions_from_header(bytes).or_else(|| {
        ImageReader::new(Cursor::new(bytes))
            .with_guessed_format()
            .ok()?
            .into_dimensions()
            .ok()
    })
}

fn seed_target_long_edge_from_view(
    bytes: &[u8],
    fallback_target_long_edge: u32,
    target_view: Option<SeedTargetView>,
) -> u32 {
    let Some(target_view) = target_view else {
        return fallback_target_long_edge;
    };
    let Some((width, height)) = source_dimensions_from_bytes(bytes) else {
        return fallback_target_long_edge;
    };
    target_long_edge_for_view(
        target_view.fit_mode,
        target_view.manual_zoom,
        target_view.page_viewport,
        target_view.pixels_per_point,
        &[OriginalPageSize {
            width: width as f32,
            height: height as f32,
        }],
    )
}

#[cfg(test)]
mod tests {
    use super::{
        should_skip_memory_aware_adjacent_seed, should_skip_memory_aware_adjacent_seed_source,
        ADJACENT_SEED_LARGE_SOURCE_BYTES, ADJACENT_SEED_LARGE_SOURCE_LONG_EDGE,
    };
    use crate::core::source::{BookSource, SourceError};
    use image::{codecs::png::PngEncoder, ColorType, ImageEncoder};
    use std::path::{Path, PathBuf};

    #[test]
    fn memory_aware_adjacent_seed_skips_8192px_sources() {
        let bytes = png_bytes(ADJACENT_SEED_LARGE_SOURCE_LONG_EDGE, 1);

        assert!(should_skip_memory_aware_adjacent_seed(&bytes));
    }

    #[test]
    fn memory_aware_adjacent_seed_keeps_smaller_sources() {
        let bytes = png_bytes(ADJACENT_SEED_LARGE_SOURCE_LONG_EDGE - 1, 1);

        assert!(!should_skip_memory_aware_adjacent_seed(&bytes));
    }

    #[test]
    fn memory_aware_adjacent_seed_keeps_unknown_dimensions() {
        assert!(!should_skip_memory_aware_adjacent_seed(b"not an image"));
    }

    #[test]
    fn memory_aware_adjacent_seed_skips_large_known_source_bytes() {
        let source = TestSource {
            byte_size: Some(ADJACENT_SEED_LARGE_SOURCE_BYTES),
            bytes: Vec::new(),
        };

        assert!(should_skip_memory_aware_adjacent_seed_source(&source, 0));
    }

    #[test]
    fn memory_aware_adjacent_seed_keeps_large_bytes_with_smaller_dimensions() {
        let source = TestSource {
            byte_size: Some(ADJACENT_SEED_LARGE_SOURCE_BYTES),
            bytes: png_bytes(ADJACENT_SEED_LARGE_SOURCE_LONG_EDGE - 1, 1),
        };

        assert!(!should_skip_memory_aware_adjacent_seed_source(&source, 0));
    }

    #[test]
    fn memory_aware_adjacent_seed_keeps_small_or_unknown_source_bytes() {
        let small = TestSource {
            byte_size: Some(ADJACENT_SEED_LARGE_SOURCE_BYTES - 1),
            bytes: Vec::new(),
        };
        let unknown = TestSource {
            byte_size: None,
            bytes: Vec::new(),
        };

        assert!(!should_skip_memory_aware_adjacent_seed_source(&small, 0));
        assert!(!should_skip_memory_aware_adjacent_seed_source(&unknown, 0));
    }

    fn png_bytes(width: u32, height: u32) -> Vec<u8> {
        let pixels = vec![0; width as usize * height as usize * 4];
        let mut bytes = Vec::new();
        PngEncoder::new(&mut bytes)
            .write_image(&pixels, width, height, ColorType::Rgba8.into())
            .expect("test PNG should encode");
        bytes
    }

    struct TestSource {
        byte_size: Option<u64>,
        bytes: Vec<u8>,
    }

    impl BookSource for TestSource {
        fn title(&self) -> &str {
            "test"
        }

        fn source_path(&self) -> &Path {
            Path::new("test")
        }

        fn book_id(&self) -> &str {
            "test"
        }

        fn page_count(&self) -> usize {
            1
        }

        fn page_name(&self, _index: usize) -> Option<&str> {
            Some("page.png")
        }

        fn page_file_path(&self, _index: usize) -> Option<PathBuf> {
            None
        }

        fn page_byte_size(&self, _index: usize) -> Option<u64> {
            self.byte_size
        }

        fn read_page(&self, _index: usize) -> Result<Vec<u8>, SourceError> {
            Ok(self.bytes.clone())
        }

        fn read_page_prefix(
            &self,
            _index: usize,
            max_bytes: usize,
        ) -> Result<Vec<u8>, SourceError> {
            Ok(self.bytes[..self.bytes.len().min(max_bytes)].to_vec())
        }
    }
}
