use super::bookmark_thumbnail_cache::{
    read_cached_thumbnail, store_cached_thumbnail, BookmarkThumbnailDiskEntry,
};
use super::bookmark_thumbnails::{
    bookmark_thumbnail_cache_key, BookmarkThumbnailCommand, BookmarkThumbnailEvent,
    BookmarkThumbnailFailure, BookmarkThumbnailKey,
};
#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
use crate::core::perf_trace::{self, PerfField};
use crate::core::source::{open_source_from_path, BookSource, SharedSource};
use crate::core::worker::{prepare_image_with_options, DecodeOptions, PREVIEW_TARGET_LONG_EDGE};
use crossbeam_channel::{Receiver, Sender};
use eframe::egui::{self, ColorImage, Vec2};
use lru::LruCache;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
use std::time::{Duration, Instant};

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
            BookmarkThumbnailCommand::Request {
                key,
                source,
                decode,
            } => {
                #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
                let started = Instant::now();
                let event = match worker.load_thumbnail(&key, source, decode) {
                    Ok((image, original_size)) => BookmarkThumbnailEvent::Ready {
                        key,
                        image,
                        original_size,
                    },
                    Err(reason) => BookmarkThumbnailEvent::Failed { key, reason },
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
        source: Option<ThumbnailSource>,
        decode: DecodeOptions,
    ) -> Result<(Arc<ColorImage>, Vec2), BookmarkThumbnailFailure> {
        let disk_entry = BookmarkThumbnailDiskEntry::new(bookmark_thumbnail_cache_key(key));
        if let Ok(Some(image)) = read_cached_thumbnail(&disk_entry) {
            let original_size = egui::vec2(image.size[0] as f32, image.size[1] as f32);
            return Ok((Arc::new(image), original_size));
        }

        let source = source.ok_or(BookmarkThumbnailFailure::MissingSource)?;
        let (source, forced_page) = self
            .open_thumbnail_source(source)
            .map_err(|_| BookmarkThumbnailFailure::SourceOpenFailed)?;
        let page_index = resolve_thumbnail_page(
            source.as_ref(),
            key.page_name.as_deref(),
            key.page,
            forced_page,
        )
        .ok_or(BookmarkThumbnailFailure::PageMissing)?;

        let bytes = source
            .read_page(page_index)
            .map_err(|_| BookmarkThumbnailFailure::ReadFailed)?;
        let page = prepare_image_with_options(&bytes, PREVIEW_TARGET_LONG_EDGE, decode)
            .map_err(|_| BookmarkThumbnailFailure::DecodeFailed)?;
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
    use super::{
        resolve_thumbnail_page, thumbnail_color_image, ThumbnailSource, ThumbnailWorkerState,
    };
    use crate::app::ui::bookmark_thumbnail_cache::{
        store_cached_thumbnail, BookmarkThumbnailDiskEntry,
    };
    use crate::app::ui::bookmark_thumbnails::{
        bookmark_thumbnail_cache_key, BookmarkThumbnailFailure, BookmarkThumbnailKey,
    };
    use crate::core::source::{BookSource, SharedSource, SourceError};
    use crate::core::worker::DecodeOptions;
    use eframe::egui::{self, Color32, ColorImage};
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    #[test]
    fn page_resolution_prefers_page_name_before_index() {
        let source = MockSource::new(vec![
            "001.png".to_owned(),
            "002.png".to_owned(),
            "target.png".to_owned(),
        ]);

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

    #[test]
    fn cached_thumbnail_is_returned_without_source() {
        let key = sample_key("disk-first");
        let entry = BookmarkThumbnailDiskEntry::new(bookmark_thumbnail_cache_key(&key));
        let source_image = ColorImage::new([3, 2], vec![Color32::WHITE; 6]);
        store_cached_thumbnail(&entry, &source_image, false).unwrap();

        let mut worker = ThumbnailWorkerState::new();
        let (image, original_size) = worker
            .load_thumbnail(&key, None, DecodeOptions::default())
            .unwrap();

        assert_eq!(image.size, [3, 2]);
        assert_eq!(image.pixels, source_image.pixels);
        assert_eq!(original_size, egui::vec2(3.0, 2.0));
        let _ = std::fs::remove_file(entry.path_for_test());
    }

    #[test]
    fn missing_source_reports_failure_reason() {
        let mut worker = ThumbnailWorkerState::new();
        let reason = worker
            .load_thumbnail(
                &sample_key("missing-source"),
                None,
                DecodeOptions::default(),
            )
            .unwrap_err();

        assert_eq!(reason, BookmarkThumbnailFailure::MissingSource);
    }

    #[test]
    fn page_missing_reports_failure_reason() {
        let mut worker = ThumbnailWorkerState::new();
        let source = Arc::new(MockSource::new(Vec::new())) as SharedSource;
        let reason = worker
            .load_thumbnail(
                &sample_key("page-missing"),
                Some(ThumbnailSource::Opened(source)),
                DecodeOptions::default(),
            )
            .unwrap_err();

        assert_eq!(reason, BookmarkThumbnailFailure::PageMissing);
    }

    #[test]
    fn read_failure_reports_failure_reason() {
        let mut worker = ThumbnailWorkerState::new();
        let source = Arc::new(MockSource::read_failing(vec!["001.png".to_owned()])) as SharedSource;
        let reason = worker
            .load_thumbnail(
                &sample_key("read-failed"),
                Some(ThumbnailSource::Opened(source)),
                DecodeOptions::default(),
            )
            .unwrap_err();

        assert_eq!(reason, BookmarkThumbnailFailure::ReadFailed);
    }

    #[test]
    fn decode_failure_reports_failure_reason() {
        let mut worker = ThumbnailWorkerState::new();
        let source = Arc::new(MockSource::with_bytes(
            vec!["001.png".to_owned()],
            b"not an image".to_vec(),
        )) as SharedSource;
        let reason = worker
            .load_thumbnail(
                &sample_key("decode-failed"),
                Some(ThumbnailSource::Opened(source)),
                DecodeOptions::default(),
            )
            .unwrap_err();

        assert_eq!(reason, BookmarkThumbnailFailure::DecodeFailed);
    }

    struct MockSource {
        names: Vec<String>,
        read: MockRead,
    }

    impl MockSource {
        fn new(names: Vec<String>) -> Self {
            Self {
                names,
                read: MockRead::Unused,
            }
        }

        fn read_failing(names: Vec<String>) -> Self {
            Self {
                names,
                read: MockRead::Fail,
            }
        }

        fn with_bytes(names: Vec<String>, bytes: Vec<u8>) -> Self {
            Self {
                names,
                read: MockRead::Bytes(bytes),
            }
        }
    }

    enum MockRead {
        Unused,
        Fail,
        Bytes(Vec<u8>),
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
            match &self.read {
                MockRead::Unused => unreachable!("page resolution tests do not read image bytes"),
                MockRead::Fail => Err(SourceError::Unsupported("read failed".to_owned())),
                MockRead::Bytes(bytes) => Ok(bytes.clone()),
            }
        }
    }

    fn sample_key(label: &str) -> BookmarkThumbnailKey {
        BookmarkThumbnailKey {
            book_id: format!("book-{label}-{:?}", std::thread::current().id()),
            known_path: Some(format!("C:/books/{label}.cbz")),
            page: 0,
            page_name: Some("001.png".to_owned()),
        }
    }
}
