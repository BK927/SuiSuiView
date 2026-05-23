use super::SuiSuiViewApp;
use crate::core::image_info::{analyze_image_info, ImageInfo};
use crate::core::source::SharedSource;
use crossbeam_channel::{unbounded, Receiver, Sender};
use eframe::egui;
use std::sync::Arc;
use std::thread;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ImageInfoKey {
    book_id: String,
    page: usize,
}

impl ImageInfoKey {
    fn new(book_id: String, page: usize) -> Self {
        Self { book_id, page }
    }
}

#[derive(Debug, Clone)]
pub(super) enum ImageInfoStatus {
    Empty,
    Loading,
    Ready(Arc<ImageInfo>),
    Failed(String),
}

#[derive(Debug)]
struct ImageInfoEvent {
    key: ImageInfoKey,
    result: Result<Arc<ImageInfo>, String>,
}

pub(super) struct ImageInfoState {
    tx: Sender<ImageInfoEvent>,
    rx: Receiver<ImageInfoEvent>,
    inflight: Option<ImageInfoKey>,
    cached: Option<(ImageInfoKey, Result<Arc<ImageInfo>, String>)>,
}

impl ImageInfoState {
    pub(super) fn new() -> Self {
        let (tx, rx) = unbounded();
        Self {
            tx,
            rx,
            inflight: None,
            cached: None,
        }
    }

    fn request(&mut self, ctx: egui::Context, source: SharedSource, key: ImageInfoKey) {
        if self.inflight.as_ref() == Some(&key) {
            return;
        }
        self.inflight = Some(key.clone());
        let failed_key = key.clone();
        let tx = self.tx.clone();
        if let Err(error) = thread::Builder::new()
            .name("suisuiview-image-info".to_owned())
            .spawn(move || {
                let result = source
                    .read_page(key.page)
                    .map_err(|error| error.to_string())
                    .and_then(|bytes| analyze_image_info(&bytes).map(Arc::new));
                let _ = tx.send(ImageInfoEvent { key, result });
                ctx.request_repaint();
            })
        {
            self.inflight = None;
            self.cached = Some((failed_key, Err(error.to_string())));
        }
    }

    fn drain(&mut self, active_key: &ImageInfoKey) {
        while let Ok(event) = self.rx.try_recv() {
            self.accept_event(active_key, event);
        }
    }

    fn status(&self, key: &ImageInfoKey) -> Option<ImageInfoStatus> {
        let (cached_key, result) = self.cached.as_ref()?;
        if cached_key != key {
            return None;
        }
        Some(match result {
            Ok(info) => ImageInfoStatus::Ready(info.clone()),
            Err(error) => ImageInfoStatus::Failed(error.clone()),
        })
    }

    fn clear(&mut self) {
        self.inflight = None;
        self.cached = None;
        while self.rx.try_recv().is_ok() {}
    }

    fn accept_event(&mut self, active_key: &ImageInfoKey, event: ImageInfoEvent) {
        if self.inflight.as_ref() == Some(&event.key) {
            self.inflight = None;
        }
        if &event.key == active_key {
            self.cached = Some((event.key, event.result));
        }
    }
}

impl SuiSuiViewApp {
    pub(super) fn current_image_info_status(&mut self, ctx: &egui::Context) -> ImageInfoStatus {
        let Some(source) = self.source.clone() else {
            self.image_info.clear();
            return ImageInfoStatus::Empty;
        };
        let Some(book_id) = self.book_id.clone() else {
            self.image_info.clear();
            return ImageInfoStatus::Empty;
        };

        let key = ImageInfoKey::new(book_id, self.current_page);
        self.image_info.drain(&key);
        if let Some(status) = self.image_info.status(&key) {
            return status;
        }

        self.image_info.request(ctx.clone(), source, key);
        ImageInfoStatus::Loading
    }
}

#[cfg(test)]
mod tests {
    use super::{ImageInfoEvent, ImageInfoKey, ImageInfoState};

    #[test]
    fn image_info_key_distinguishes_book_and_page() {
        assert_ne!(
            ImageInfoKey::new("book-a".to_owned(), 1),
            ImageInfoKey::new("book-b".to_owned(), 1)
        );
        assert_ne!(
            ImageInfoKey::new("book-a".to_owned(), 1),
            ImageInfoKey::new("book-a".to_owned(), 2)
        );
    }

    #[test]
    fn stale_image_info_event_is_ignored() {
        let mut state = ImageInfoState::new();
        let active = ImageInfoKey::new("book-a".to_owned(), 1);
        let stale = ImageInfoKey::new("book-a".to_owned(), 0);

        state.accept_event(
            &active,
            ImageInfoEvent {
                key: stale,
                result: Err("stale".to_owned()),
            },
        );

        assert!(state.status(&active).is_none());
    }

    #[test]
    fn active_image_info_event_is_cached() {
        let mut state = ImageInfoState::new();
        let active = ImageInfoKey::new("book-a".to_owned(), 1);

        state.accept_event(
            &active,
            ImageInfoEvent {
                key: active.clone(),
                result: Err("failed".to_owned()),
            },
        );

        assert!(state.status(&active).is_some());
    }
}
