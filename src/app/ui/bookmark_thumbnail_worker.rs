use super::bookmark_thumbnail_cache::{
    read_cached_thumbnail, store_cached_thumbnail, BookmarkThumbnailDiskEntry,
};
use super::bookmark_thumbnails::{
    bookmark_thumbnail_cache_key, BookmarkThumbnailCommand, BookmarkThumbnailEvent,
    BookmarkThumbnailKey,
};
#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
use crate::core::perf_trace::{self, PerfField};
use crate::core::source::{open_source_from_path, BookSource, SharedSource};
use crate::core::worker::{prepare_image_with_options, PREVIEW_TARGET_LONG_EDGE};
use crossbeam_channel::{Receiver, Sender};
use eframe::egui::{self, ColorImage, Vec2};
use lru::LruCache;
use std::fs;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
use std::time::{Duration, Instant};
use std::time::{SystemTime, UNIX_EPOCH};

const THUMBNAIL_TEXTURE_LONG_EDGE: usize = 96;
const THUMBNAIL_SOURCE_CACHE_LIMIT: usize = 8;
const THUMBNAIL_CACHE_PRUNE_INTERVAL: usize = 32;

#[derive(Clone)]
pub(super) enum ThumbnailSource {
    Opened(SharedSource),
    KnownPath(PathBuf),
}

pub(super) fn run_thumbnail_worker(
    command_rx: Receiver<BookmarkThumbnailCommand>,
    event_tx: Sender<BookmarkThumbnailEvent>,
    ctx: egui::Context,
    shutdown_requested: Arc<AtomicBool>,
) {
    let mut worker = ThumbnailWorkerState::new();
    while !shutdown_requested.load(Ordering::Acquire) {
        let Ok(command) = command_rx.recv() else {
            break;
        };
        if shutdown_requested.load(Ordering::Acquire) {
            break;
        }
        match command {
            BookmarkThumbnailCommand::Request { key, source } => {
                #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
                let started = Instant::now();
                let event = match worker.load_thumbnail(&key, source) {
                    Ok((image, original_size)) => BookmarkThumbnailEvent::Ready {
                        key,
                        image,
                        original_size,
                    },
                    Err(()) => BookmarkThumbnailEvent::Failed { key },
                };
                #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
                perf_trace::record_duration_if_at_least(
                    "bookmark_thumbnail_prepare",
                    started.elapsed(),
                    Duration::from_millis(40),
                    &[PerfField::Bool(
                        "cancel_requested",
                        shutdown_requested.load(Ordering::Acquire),
                    )],
                );
                if shutdown_requested.load(Ordering::Acquire) {
                    break;
                }
                let _ = event_tx.send(event);
                ctx.request_repaint();
            }
            BookmarkThumbnailCommand::Shutdown => break,
        }
    }
}

struct ThumbnailWorkerState {
    sources: LruCache<PathBuf, CachedThumbnailSource>,
    thumbnail_writes_since_prune: usize,
}

impl ThumbnailWorkerState {
    fn new() -> Self {
        Self {
            sources: LruCache::new(NonZeroUsize::new(THUMBNAIL_SOURCE_CACHE_LIMIT).unwrap()),
            thumbnail_writes_since_prune: THUMBNAIL_CACHE_PRUNE_INTERVAL.saturating_sub(1),
        }
    }

    fn load_thumbnail(
        &mut self,
        key: &BookmarkThumbnailKey,
        source: ThumbnailSource,
    ) -> Result<(Arc<ColorImage>, Vec2), ()> {
        let (source, forced_page) = self.open_thumbnail_source(source)?;
        let page_index = resolve_thumbnail_page(
            source.as_ref(),
            key.page_name.as_deref(),
            key.page,
            forced_page,
        )
        .ok_or(())?;
        let fingerprint = source_fingerprint(source.as_ref(), page_index)?;
        let disk_entry = BookmarkThumbnailDiskEntry::new(bookmark_thumbnail_cache_key(
            key,
            page_index,
            &fingerprint,
        ));
        if let Ok(Some(image)) = read_cached_thumbnail(&disk_entry) {
            let original_size = egui::vec2(image.size[0] as f32, image.size[1] as f32);
            return Ok((Arc::new(image), original_size));
        }

        let bytes = source.read_page(page_index).map_err(|_| ())?;
        let page = prepare_image_with_options(&bytes, PREVIEW_TARGET_LONG_EDGE, key.decode)
            .map_err(|_| ())?;
        let original_size = egui::vec2(page.original_width as f32, page.original_height as f32);
        let image = thumbnail_color_image(&page.color_image());
        let _ = store_cached_thumbnail(&disk_entry, &image, self.should_prune_after_store());
        Ok((Arc::new(image), original_size))
    }

    fn open_thumbnail_source(
        &mut self,
        source: ThumbnailSource,
    ) -> Result<(SharedSource, Option<usize>), ()> {
        match source {
            ThumbnailSource::Opened(source) => Ok((source, None)),
            ThumbnailSource::KnownPath(path) => {
                if let Some(cached) = self.sources.get(&path) {
                    return Ok((cached.source.clone(), cached.forced_page));
                }
                let (source, forced_page) = open_source_from_path(&path).map_err(|_| ())?;
                self.sources.put(
                    path,
                    CachedThumbnailSource {
                        source: source.clone(),
                        forced_page,
                    },
                );
                Ok((source, forced_page))
            }
        }
    }

    fn should_prune_after_store(&mut self) -> bool {
        self.thumbnail_writes_since_prune += 1;
        if self.thumbnail_writes_since_prune < THUMBNAIL_CACHE_PRUNE_INTERVAL {
            return false;
        }
        self.thumbnail_writes_since_prune = 0;
        true
    }
}

#[derive(Clone)]
struct CachedThumbnailSource {
    source: SharedSource,
    forced_page: Option<usize>,
}

pub(super) fn thumbnail_source(
    source: Option<SharedSource>,
    known_path: Option<&str>,
) -> Option<ThumbnailSource> {
    source
        .map(ThumbnailSource::Opened)
        .or_else(|| known_path.map(|path| ThumbnailSource::KnownPath(PathBuf::from(path))))
}

fn resolve_thumbnail_page(
    source: &dyn BookSource,
    page_name: Option<&str>,
    page: usize,
    forced_page: Option<usize>,
) -> Option<usize> {
    if let Some(page_name) = page_name {
        if let Some(index) = (0..source.page_count()).find(|&index| {
            source
                .page_name(index)
                .is_some_and(|candidate| candidate == page_name)
        }) {
            return Some(index);
        }
    }
    if page < source.page_count() {
        return Some(page);
    }
    forced_page.filter(|index| *index < source.page_count())
}

fn source_fingerprint(source: &dyn BookSource, page: usize) -> Result<String, ()> {
    if let Some(path) = source.page_file_path(page) {
        return file_fingerprint("file", &path, None);
    }
    file_fingerprint("source", source.source_path(), source.page_name(page))
}

fn file_fingerprint(kind: &str, path: &Path, extra: Option<&str>) -> Result<String, ()> {
    let metadata = fs::metadata(path).map_err(|_| ())?;
    let modified = metadata
        .modified()
        .ok()
        .and_then(system_time_nanos)
        .unwrap_or_default();
    Ok(format!(
        "{kind}\0{}\0{}\0{}\0{}",
        path.display(),
        metadata.len(),
        modified,
        extra.unwrap_or_default()
    ))
}

fn system_time_nanos(time: SystemTime) -> Option<u128> {
    time.duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_nanos())
}

fn thumbnail_color_image(source: &ColorImage) -> ColorImage {
    let [source_width, source_height] = source.size;
    let longest = source_width.max(source_height);
    if longest <= THUMBNAIL_TEXTURE_LONG_EDGE {
        return source.clone();
    }

    let scale = THUMBNAIL_TEXTURE_LONG_EDGE as f32 / longest as f32;
    let width = ((source_width as f32 * scale).round() as usize).max(1);
    let height = ((source_height as f32 * scale).round() as usize).max(1);
    let mut pixels = Vec::with_capacity(width * height);
    for y in 0..height {
        let source_y = ((y * source_height) / height).min(source_height.saturating_sub(1));
        for x in 0..width {
            let source_x = ((x * source_width) / width).min(source_width.saturating_sub(1));
            pixels.push(source.pixels[source_y * source_width + source_x]);
        }
    }
    ColorImage::new([width, height], pixels)
}

#[cfg(test)]
mod tests {
    use super::{resolve_thumbnail_page, thumbnail_color_image};
    use crate::core::source::{BookSource, SourceError};
    use eframe::egui::{Color32, ColorImage};
    use std::path::{Path, PathBuf};

    #[test]
    fn page_resolution_prefers_page_name_before_index() {
        let source = MockSource {
            names: vec![
                "001.png".to_owned(),
                "002.png".to_owned(),
                "target.png".to_owned(),
            ],
        };

        assert_eq!(
            resolve_thumbnail_page(&source, Some("target.png"), 0, None),
            Some(2)
        );
        assert_eq!(
            resolve_thumbnail_page(&source, Some("missing.png"), 1, None),
            Some(1)
        );
    }

    #[test]
    fn thumbnail_image_downscales_long_edge() {
        let source = ColorImage::new([360, 180], vec![Color32::WHITE; 360 * 180]);
        let thumbnail = thumbnail_color_image(&source);

        assert_eq!(thumbnail.size, [96, 48]);
    }

    struct MockSource {
        names: Vec<String>,
    }

    impl BookSource for MockSource {
        fn title(&self) -> &str {
            "mock"
        }

        fn source_path(&self) -> &Path {
            Path::new("mock")
        }

        fn book_id(&self) -> &str {
            "mock"
        }

        fn page_count(&self) -> usize {
            self.names.len()
        }

        fn page_name(&self, index: usize) -> Option<&str> {
            self.names.get(index).map(String::as_str)
        }

        fn page_file_path(&self, _index: usize) -> Option<PathBuf> {
            None
        }

        fn read_page(&self, _index: usize) -> Result<Vec<u8>, SourceError> {
            unreachable!("page resolution tests do not read image bytes")
        }
    }
}
