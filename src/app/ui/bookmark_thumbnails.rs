use crate::core::perf_trace::{self, PerfField};
use crate::core::source::SharedSource;
use crate::core::worker::{prepare_image_with_options, DecodeOptions, PREVIEW_TARGET_LONG_EDGE};
use crossbeam_channel::{bounded, unbounded, Receiver, Sender};
use eframe::egui::{self, Color32, ColorImage, TextureHandle, TextureOptions, Vec2};
use lru::LruCache;
use std::collections::HashSet;
use std::num::NonZeroUsize;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const THUMBNAIL_TEXTURE_LONG_EDGE: usize = 96;
const THUMBNAIL_CACHE_LIMIT: usize = 64;
const MAX_THUMBNAIL_UPLOADS_PER_FRAME: usize = 4;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct BookmarkThumbnailKey {
    book_id: String,
    page: usize,
    decode: DecodeOptions,
}

struct BookmarkThumbnailEntry {
    texture: TextureHandle,
    original_size: Vec2,
}

pub(in crate::app) enum BookmarkThumbnailState {
    Ready {
        texture: TextureHandle,
        original_size: Vec2,
    },
    Loading,
    Failed,
}

enum BookmarkThumbnailCommand {
    Request {
        key: BookmarkThumbnailKey,
        source: SharedSource,
    },
    Shutdown,
}

enum BookmarkThumbnailEvent {
    Ready {
        key: BookmarkThumbnailKey,
        image: Arc<ColorImage>,
        original_size: Vec2,
    },
    Failed {
        key: BookmarkThumbnailKey,
    },
}

pub(in crate::app) struct BookmarkThumbnails {
    command_tx: Sender<BookmarkThumbnailCommand>,
    event_rx: Receiver<BookmarkThumbnailEvent>,
    entries: LruCache<BookmarkThumbnailKey, BookmarkThumbnailEntry>,
    inflight: HashSet<BookmarkThumbnailKey>,
    failed: HashSet<BookmarkThumbnailKey>,
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
            failed: HashSet::new(),
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

    pub(in crate::app) fn drain(
        &mut self,
        ctx: &egui::Context,
        active_book_id: Option<&str>,
        active_decode: DecodeOptions,
    ) {
        let mut uploads = 0;
        while uploads < MAX_THUMBNAIL_UPLOADS_PER_FRAME {
            let Ok(event) = self.event_rx.try_recv() else {
                return;
            };
            match event {
                BookmarkThumbnailEvent::Ready {
                    key,
                    image,
                    original_size,
                } if is_active_thumbnail_key(&key, active_book_id, active_decode) => {
                    self.inflight.remove(&key);
                    let texture = ctx.load_texture(
                        format!(
                            "bookmark-thumb-{}-{}-{}",
                            key.book_id,
                            key.page,
                            key.decode.cache_token()
                        ),
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
                BookmarkThumbnailEvent::Failed { key }
                    if is_active_thumbnail_key(&key, active_book_id, active_decode) =>
                {
                    self.inflight.remove(&key);
                    self.failed.insert(key);
                }
                BookmarkThumbnailEvent::Ready { key, .. }
                | BookmarkThumbnailEvent::Failed { key } => {
                    self.inflight.remove(&key);
                }
            }
        }
        ctx.request_repaint();
    }

    pub(in crate::app) fn thumbnail(
        &mut self,
        source: SharedSource,
        book_id: &str,
        page: usize,
        decode: DecodeOptions,
    ) -> BookmarkThumbnailState {
        let key = BookmarkThumbnailKey {
            book_id: book_id.to_owned(),
            page,
            decode,
        };
        if let Some(entry) = self.entries.get(&key) {
            return BookmarkThumbnailState::Ready {
                texture: entry.texture.clone(),
                original_size: entry.original_size,
            };
        }
        if self.failed.contains(&key) {
            return BookmarkThumbnailState::Failed;
        }
        if self.inflight.insert(key.clone())
            && self
                .command_tx
                .send(BookmarkThumbnailCommand::Request {
                    key: key.clone(),
                    source,
                })
                .is_err()
        {
            self.inflight.remove(&key);
            self.failed.insert(key);
            return BookmarkThumbnailState::Failed;
        }
        BookmarkThumbnailState::Loading
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

impl Drop for BookmarkThumbnails {
    fn drop(&mut self) {
        self.request_shutdown();
    }
}

fn run_thumbnail_worker(
    command_rx: Receiver<BookmarkThumbnailCommand>,
    event_tx: Sender<BookmarkThumbnailEvent>,
    ctx: egui::Context,
    shutdown_requested: Arc<AtomicBool>,
) {
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
                let event = match load_thumbnail(&key, &source) {
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

fn load_thumbnail(
    key: &BookmarkThumbnailKey,
    source: &SharedSource,
) -> Result<(Arc<ColorImage>, Vec2), ()> {
    let bytes = source.read_page(key.page).map_err(|_| ())?;
    let page =
        prepare_image_with_options(&bytes, PREVIEW_TARGET_LONG_EDGE, key.decode).map_err(|_| ())?;
    let original_size = egui::vec2(page.original_width as f32, page.original_height as f32);
    let image = thumbnail_color_image(&page.image);
    Ok((Arc::new(image), original_size))
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

fn is_active_thumbnail_key(
    key: &BookmarkThumbnailKey,
    active_book_id: Option<&str>,
    active_decode: DecodeOptions,
) -> bool {
    active_book_id == Some(key.book_id.as_str()) && key.decode == active_decode
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
    use super::{is_active_thumbnail_key, thumbnail_color_image, BookmarkThumbnailKey};
    use crate::core::worker::{DecodeOptions, DecodeStrategy};
    use eframe::egui::{Color32, ColorImage};

    #[test]
    fn thumbnail_key_tracks_book_page_and_decode() {
        let base = BookmarkThumbnailKey {
            book_id: "book".to_owned(),
            page: 1,
            decode: DecodeOptions::default(),
        };
        let other_page = BookmarkThumbnailKey {
            page: 2,
            ..base.clone()
        };
        let other_decode = BookmarkThumbnailKey {
            decode: DecodeOptions {
                strategy: DecodeStrategy::ImageCrate,
                ..DecodeOptions::default()
            },
            ..base.clone()
        };

        assert_ne!(base, other_page);
        assert_ne!(base, other_decode);
    }

    #[test]
    fn active_check_rejects_stale_book_or_decode() {
        let key = BookmarkThumbnailKey {
            book_id: "book".to_owned(),
            page: 1,
            decode: DecodeOptions::default(),
        };
        let other_decode = DecodeOptions {
            strategy: DecodeStrategy::ImageCrate,
            ..DecodeOptions::default()
        };

        assert!(is_active_thumbnail_key(
            &key,
            Some("book"),
            DecodeOptions::default()
        ));
        assert!(!is_active_thumbnail_key(
            &key,
            Some("other"),
            DecodeOptions::default()
        ));
        assert!(!is_active_thumbnail_key(&key, Some("book"), other_decode));
        assert!(!is_active_thumbnail_key(
            &key,
            None,
            DecodeOptions::default()
        ));
    }

    #[test]
    fn thumbnail_image_downscales_long_edge() {
        let source = ColorImage::new([360, 180], vec![Color32::WHITE; 360 * 180]);
        let thumbnail = thumbnail_color_image(&source);

        assert_eq!(thumbnail.size, [96, 48]);
    }
}
