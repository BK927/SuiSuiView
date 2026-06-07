use super::bookmark_thumbnail_worker::{run_thumbnail_worker, thumbnail_source, ThumbnailSource};
use crate::core::perf_trace::{self, PerfField};
use crate::core::source::SharedSource;
use crate::core::worker::DecodeOptions;
use crossbeam_channel::{bounded, unbounded, Receiver, Sender};
use eframe::egui::{self, Color32, ColorImage, TextureHandle, TextureOptions, Vec2};
use lru::LruCache;
use std::collections::{HashMap, HashSet};
use std::num::NonZeroUsize;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const THUMBNAIL_CACHE_LIMIT: usize = 64;
const MAX_THUMBNAIL_UPLOADS_PER_FRAME: usize = 4;
const MAX_THUMBNAIL_EVENTS_PER_FRAME: usize = 32;
const THUMBNAIL_DISK_CACHE_VERSION: &str = "suisuiview:bookmark-thumbnail-v2";
const FAILED_THUMBNAIL_RETRY_AFTER: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct BookmarkThumbnailKey {
    pub(super) book_id: String,
    pub(super) known_path: Option<String>,
    pub(super) page: usize,
    pub(super) page_name: Option<String>,
}

struct BookmarkThumbnailEntry {
    texture: TextureHandle,
    original_size: Vec2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app) enum BookmarkThumbnailFailure {
    MissingSource,
    SourceOpenFailed,
    PageMissing,
    ReadFailed,
    DecodeFailed,
}

pub(in crate::app) enum BookmarkThumbnailState {
    Ready {
        texture: TextureHandle,
        original_size: Vec2,
    },
    Loading,
    Failed,
}

pub(super) enum BookmarkThumbnailCommand {
    Request {
        key: BookmarkThumbnailKey,
        source: Option<ThumbnailSource>,
        decode: DecodeOptions,
    },
    Shutdown,
}

pub(super) enum BookmarkThumbnailEvent {
    Ready {
        key: BookmarkThumbnailKey,
        image: Arc<ColorImage>,
        original_size: Vec2,
    },
    Failed {
        key: BookmarkThumbnailKey,
        reason: BookmarkThumbnailFailure,
    },
}

pub(in crate::app) struct BookmarkThumbnails {
    command_tx: Sender<BookmarkThumbnailCommand>,
    event_rx: Receiver<BookmarkThumbnailEvent>,
    entries: LruCache<BookmarkThumbnailKey, BookmarkThumbnailEntry>,
    inflight: HashSet<BookmarkThumbnailKey>,
    failed: HashMap<BookmarkThumbnailKey, (Instant, BookmarkThumbnailFailure)>,
    shutdown_requested: Arc<AtomicBool>,
    stopped_rx: Receiver<()>,
    join: Option<JoinHandle<()>>,
}

impl BookmarkThumbnails {
    pub(in crate::app) fn new(ctx: egui::Context) -> Self {
        let (command_tx, command_rx) = unbounded();
        let (event_tx, event_rx) = unbounded();
        let (stopped_tx, stopped_rx) = bounded(1);
        let shutdown_requested = Arc::new(AtomicBool::new(false));
        let worker_shutdown_requested = shutdown_requested.clone();
        let join = thread::Builder::new()
            .name("suisuiview-bookmark-thumbnails".to_owned())
            .spawn(move || {
                run_thumbnail_worker(command_rx, event_tx, ctx, worker_shutdown_requested);
                let _ = stopped_tx.send(());
            })
            .expect("bookmark thumbnail worker should start");

        Self {
            command_tx,
            event_rx,
            entries: LruCache::new(NonZeroUsize::new(THUMBNAIL_CACHE_LIMIT).unwrap()),
            inflight: HashSet::new(),
            failed: HashMap::new(),
            shutdown_requested,
            stopped_rx,
            join: Some(join),
        }
    }

    pub(in crate::app) fn clear(&mut self) {
        self.entries.clear();
        self.inflight.clear();
        self.failed.clear();
    }

    pub(in crate::app) fn drain(&mut self, ctx: &egui::Context) {
        let mut uploads = 0;
        let mut processed = 0;
        while uploads < MAX_THUMBNAIL_UPLOADS_PER_FRAME
            && processed < MAX_THUMBNAIL_EVENTS_PER_FRAME
        {
            let Ok(event) = self.event_rx.try_recv() else {
                break;
            };
            processed += 1;
            match event {
                BookmarkThumbnailEvent::Ready {
                    key,
                    image,
                    original_size,
                } => {
                    self.inflight.remove(&key);
                    self.failed.remove(&key);
                    let texture = ctx.load_texture(
                        key.texture_label(),
                        egui::ImageData::Color(image),
                        TextureOptions::LINEAR,
                    );
                    self.entries.put(
                        key,
                        BookmarkThumbnailEntry {
                            texture,
                            original_size,
                        },
                    );
                    uploads += 1;
                }
                BookmarkThumbnailEvent::Failed { key, reason } => {
                    self.inflight.remove(&key);
                    self.failed.insert(key, (Instant::now(), reason));
                }
            }
        }
        if processed > 0 {
            ctx.request_repaint();
        }
    }

    pub(in crate::app) fn thumbnail(
        &mut self,
        source: Option<SharedSource>,
        book_id: &str,
        known_path: Option<&str>,
        page: usize,
        page_name: Option<&str>,
        decode: DecodeOptions,
    ) -> BookmarkThumbnailState {
        let key = BookmarkThumbnailKey::new(book_id, known_path, page, page_name);
        if let Some(entry) = self.entries.get(&key) {
            return BookmarkThumbnailState::Ready {
                texture: entry.texture.clone(),
                original_size: entry.original_size,
            };
        }
        if let Some((failed_at, _reason)) = self.failed.get(&key).copied() {
            if failed_at.elapsed() < FAILED_THUMBNAIL_RETRY_AFTER {
                return BookmarkThumbnailState::Failed;
            }
            self.failed.remove(&key);
        }
        let source = thumbnail_source(source, key.known_path.as_deref());
        if let Err(_reason) = self.enqueue_request(key, source, decode) {
            return BookmarkThumbnailState::Failed;
        }
        BookmarkThumbnailState::Loading
    }

    pub(in crate::app) fn prewarm(
        &mut self,
        source: Option<SharedSource>,
        book_id: &str,
        known_path: Option<&str>,
        page: usize,
        page_name: Option<&str>,
        decode: DecodeOptions,
    ) {
        let key = BookmarkThumbnailKey::new(book_id, known_path, page, page_name);
        if self.entries.contains(&key) {
            return;
        }
        self.failed.remove(&key);
        let source = thumbnail_source(source, key.known_path.as_deref());
        let _ = self.enqueue_request(key, source, decode);
    }

    fn enqueue_request(
        &mut self,
        key: BookmarkThumbnailKey,
        source: Option<ThumbnailSource>,
        decode: DecodeOptions,
    ) -> Result<(), BookmarkThumbnailFailure> {
        if !self.inflight.insert(key.clone()) {
            return Ok(());
        }
        if self
            .command_tx
            .send(BookmarkThumbnailCommand::Request {
                key: key.clone(),
                source,
                decode,
            })
            .is_err()
        {
            self.inflight.remove(&key);
            let reason = BookmarkThumbnailFailure::SourceOpenFailed;
            self.failed.insert(key, (Instant::now(), reason));
            return Err(reason);
        }
        Ok(())
    }

    pub(in crate::app) fn request_shutdown(&mut self) -> bool {
        if self.shutdown_requested.swap(true, Ordering::AcqRel) {
            return self.join.is_none();
        }
        let started = Instant::now();
        let sent = self
            .command_tx
            .send(BookmarkThumbnailCommand::Shutdown)
            .is_ok();
        let had_thread = self.join.take().is_some();
        let stopped = self
            .stopped_rx
            .recv_timeout(Duration::from_millis(30))
            .is_ok();
        perf_trace::record_duration(
            "shutdown_request",
            started.elapsed(),
            &[
                PerfField::Str("component", "bookmark_thumbnails"),
                PerfField::Bool("command_sent", sent),
                PerfField::Bool("thread_detached", had_thread && !stopped),
                PerfField::Bool("thread_stopped", stopped),
            ],
        );
        stopped
    }
}

impl BookmarkThumbnailKey {
    fn new(book_id: &str, known_path: Option<&str>, page: usize, page_name: Option<&str>) -> Self {
        Self {
            book_id: book_id.to_owned(),
            known_path: normalized_key_part(known_path),
            page,
            page_name: normalized_key_part(page_name),
        }
    }

    fn texture_label(&self) -> String {
        format!("bookmark-thumb-{}", bookmark_thumbnail_identity_key(self))
    }
}

impl Drop for BookmarkThumbnails {
    fn drop(&mut self) {
        self.request_shutdown();
    }
}

fn bookmark_thumbnail_identity_key(key: &BookmarkThumbnailKey) -> String {
    let mut hasher = blake3::Hasher::new();
    hash_thumbnail_identity(&mut hasher, key);
    hasher.finalize().to_hex().to_string()
}

pub(super) fn bookmark_thumbnail_cache_key(key: &BookmarkThumbnailKey) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(THUMBNAIL_DISK_CACHE_VERSION.as_bytes());
    hasher.update(&[0]);
    hash_thumbnail_identity(&mut hasher, key);
    hasher.finalize().to_hex().to_string()
}

fn hash_thumbnail_identity(hasher: &mut blake3::Hasher, key: &BookmarkThumbnailKey) {
    hasher.update(key.book_id.as_bytes());
    hasher.update(&[0]);
    hasher.update(key.known_path.as_deref().unwrap_or_default().as_bytes());
    hasher.update(&[0]);
    hasher.update(&key.page.to_le_bytes());
    hasher.update(&[0]);
    hasher.update(key.page_name.as_deref().unwrap_or_default().as_bytes());
    hasher.update(&[0]);
}

fn normalized_key_part(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

pub(in crate::app) fn thumbnail_tint_for_state(state: &BookmarkThumbnailState) -> Color32 {
    match state {
        BookmarkThumbnailState::Ready { .. } => Color32::WHITE,
        BookmarkThumbnailState::Loading => Color32::from_rgb(92, 98, 108),
        BookmarkThumbnailState::Failed => Color32::from_rgb(120, 86, 92),
    }
}

#[cfg(test)]
mod tests {
    use super::{bookmark_thumbnail_cache_key, BookmarkThumbnailKey};
    use crate::app::ui::bookmark_thumbnail_worker::thumbnail_source;

    #[test]
    fn thumbnail_key_tracks_book_path_and_page_name() {
        let base = sample_key();
        let other_page = BookmarkThumbnailKey {
            page: 2,
            ..base.clone()
        };
        let other_path = BookmarkThumbnailKey {
            known_path: Some("C:/other/book.cbz".to_owned()),
            ..base.clone()
        };
        let other_page_name = BookmarkThumbnailKey {
            page_name: Some("other/page.png".to_owned()),
            ..base.clone()
        };

        assert_ne!(base, other_page);
        assert_ne!(base, other_path);
        assert_ne!(base, other_page_name);
    }

    #[test]
    fn cache_key_uses_stable_bookmark_identity() {
        let key = sample_key();
        let base = bookmark_thumbnail_cache_key(&key);
        let same = bookmark_thumbnail_cache_key(&key);
        let other_page = bookmark_thumbnail_cache_key(&BookmarkThumbnailKey {
            page: 2,
            ..key.clone()
        });
        let other_name = bookmark_thumbnail_cache_key(&BookmarkThumbnailKey {
            page_name: Some("chapter/other.png".to_owned()),
            ..key.clone()
        });
        let other_path = bookmark_thumbnail_cache_key(&BookmarkThumbnailKey {
            known_path: Some("C:/other/book.cbz".to_owned()),
            ..key
        });

        assert_eq!(base, same);
        assert_ne!(base, other_page);
        assert_ne!(base, other_name);
        assert_ne!(base, other_path);
    }

    #[test]
    fn missing_global_path_has_no_request_source() {
        assert!(thumbnail_source(None, None).is_none());
    }

    fn sample_key() -> BookmarkThumbnailKey {
        BookmarkThumbnailKey {
            book_id: "book".to_owned(),
            known_path: Some("C:/books/book.cbz".to_owned()),
            page: 1,
            page_name: Some("chapter/page.png".to_owned()),
        }
    }
}
