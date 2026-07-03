use super::{PageCacheKey, SuiSuiViewApp};
use crate::core::auto_kind::{classify_rgba, AutoKind, AutoKindPrediction};
#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
use crate::core::perf_trace::{self, PerfField};
use crate::core::state::WgpuUpscaleMethod;
use crate::core::worker::PreparedPage;
use crossbeam_channel::{bounded, Receiver, Sender, TrySendError};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
use std::time::Instant;

const AUTO_KIND_QUEUE_LIMIT: usize = 64;
const MANGA_BW_ROUTE_CONFIDENCE: f32 = 0.55;
const COLOR_DRAWN_ROUTE_CONFIDENCE: f32 = 0.65;

pub(in crate::app) struct AutoKindWorker {
    tx: Sender<AutoKindRequest>,
    rx: Receiver<AutoKindEvent>,
    generation: Arc<AtomicU64>,
}

pub(in crate::app) struct AutoKindEvent {
    generation: u64,
    key: PageCacheKey,
    prediction: Option<AutoKindPrediction>,
}

struct AutoKindRequest {
    generation: u64,
    key: PageCacheKey,
    page: Arc<PreparedPage>,
}

impl AutoKindWorker {
    pub(in crate::app) fn new(ctx: egui::Context) -> Self {
        let (request_tx, request_rx) = bounded::<AutoKindRequest>(AUTO_KIND_QUEUE_LIMIT);
        let (event_tx, event_rx) = bounded::<AutoKindEvent>(AUTO_KIND_QUEUE_LIMIT);
        let generation = Arc::new(AtomicU64::new(0));
        let worker_generation = generation.clone();

        let _ = thread::Builder::new()
            .name("suisuiview-auto-kind".to_owned())
            .spawn(move || {
                while let Ok(request) = request_rx.recv() {
                    if request.generation != worker_generation.load(Ordering::Acquire) {
                        continue;
                    }
                    #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
                    let classify_started = Instant::now();
                    // The classifier assumes stride-4 RGBA. Expand transiently on this background
                    // thread; luma pages are rare enough here that the extra copy is negligible.
                    // TODO: a luma-native classifier path could skip the 3-channel duplication.
                    let rgba = request
                        .page
                        .pixels
                        .to_rgba_vec(request.page.display_width, request.page.display_height);
                    let prediction = classify_rgba(
                        &rgba,
                        request.page.display_width,
                        request.page.display_height,
                    );
                    #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
                    record_auto_kind_classify(
                        classify_started,
                        request.key,
                        request.page.display_width,
                        request.page.display_height,
                        prediction,
                    );
                    if request.generation != worker_generation.load(Ordering::Acquire) {
                        continue;
                    }
                    if event_tx
                        .send(AutoKindEvent {
                            generation: request.generation,
                            key: request.key,
                            prediction,
                        })
                        .is_ok()
                    {
                        ctx.request_repaint();
                    }
                }
            });

        Self {
            tx: request_tx,
            rx: event_rx,
            generation,
        }
    }

    pub(in crate::app) fn set_generation(&self, generation: u64) {
        self.generation.store(generation, Ordering::Release);
    }

    pub(in crate::app) fn enqueue(
        &self,
        generation: u64,
        key: PageCacheKey,
        page: Arc<PreparedPage>,
    ) -> bool {
        match self.tx.try_send(AutoKindRequest {
            generation,
            key,
            page,
        }) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => false,
        }
    }

    pub(in crate::app) fn try_recv(&self) -> Option<AutoKindEvent> {
        self.rx.try_recv().ok()
    }
}

#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
fn record_auto_kind_classify(
    started: Instant,
    key: PageCacheKey,
    width: usize,
    height: usize,
    prediction: Option<AutoKindPrediction>,
) {
    let (kind, confidence_milli) = prediction
        .map(|prediction| {
            (
                auto_kind_token(prediction.kind),
                (prediction.confidence.clamp(0.0, 1.0) * 1000.0).round() as usize,
            )
        })
        .unwrap_or(("invalid", 0));
    perf_trace::record_duration(
        "auto_kind_classify",
        started.elapsed(),
        &[
            PerfField::Usize("page", key.index),
            PerfField::U32("target_long_edge", key.target_long_edge),
            PerfField::Usize("width", width),
            PerfField::Usize("height", height),
            PerfField::Bool("success", prediction.is_some()),
            PerfField::Str("kind", kind),
            PerfField::Usize("confidence_milli", confidence_milli),
        ],
    );
}

#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
fn auto_kind_token(kind: AutoKind) -> &'static str {
    match kind {
        AutoKind::Anime => "anime",
        AutoKind::MangaBw => "manga_bw",
        AutoKind::Photo => "photo",
        AutoKind::Webtoon => "webtoon",
    }
}

impl SuiSuiViewApp {
    pub(in crate::app) fn clear_auto_kind_state(&mut self) {
        self.auto_kind_generation = self.auto_kind_generation.wrapping_add(1);
        if let Some(worker) = self.auto_kind_worker.as_ref() {
            worker.set_generation(self.auto_kind_generation);
        }
        self.auto_kind_hints.clear();
        self.auto_kind_inflight.clear();
    }

    pub(in crate::app) fn maybe_enqueue_auto_kind(
        &mut self,
        key: PageCacheKey,
        page: Arc<PreparedPage>,
    ) {
        if self.settings.wgpu_upscale_method != WgpuUpscaleMethod::Auto
            || self.active_wgpu_upscale_method() != WgpuUpscaleMethod::Auto
            || self.auto_kind_hints.contains_key(&key)
            || self.auto_kind_inflight.contains(&key)
        {
            return;
        }

        self.auto_kind_inflight.insert(key);
        let generation = self.auto_kind_generation;
        if !self
            .ensure_auto_kind_worker()
            .enqueue(generation, key, page)
        {
            self.auto_kind_inflight.remove(&key);
        }
    }

    pub(in crate::app) fn drain_auto_kind_events(&mut self) {
        let events = self
            .auto_kind_worker
            .as_ref()
            .map(|worker| {
                let mut events = Vec::new();
                while let Some(event) = worker.try_recv() {
                    events.push(event);
                }
                events
            })
            .unwrap_or_default();
        for event in events {
            if event.generation != self.auto_kind_generation {
                continue;
            }
            self.auto_kind_inflight.remove(&event.key);
            if let Some(prediction) = event.prediction {
                self.auto_kind_hints.insert(event.key, prediction);
                self.egui_ctx.request_repaint();
            }
        }
    }

    pub(in crate::app) fn content_aware_wgpu_upscale_method(
        &self,
        key: PageCacheKey,
        fallback: WgpuUpscaleMethod,
    ) -> WgpuUpscaleMethod {
        route_auto_kind_hint(fallback, self.auto_kind_hints.get(&key))
    }
}

pub(in crate::app) fn route_auto_kind_hint(
    fallback: WgpuUpscaleMethod,
    prediction: Option<&AutoKindPrediction>,
) -> WgpuUpscaleMethod {
    if fallback != WgpuUpscaleMethod::Auto {
        return fallback;
    }

    let Some(prediction) = prediction else {
        return fallback;
    };

    match prediction.kind {
        AutoKind::MangaBw if prediction.confidence >= MANGA_BW_ROUTE_CONFIDENCE => {
            WgpuUpscaleMethod::WgslAnime4kV32CnnX2M
        }
        AutoKind::Anime | AutoKind::Webtoon
            if prediction.confidence >= COLOR_DRAWN_ROUTE_CONFIDENCE =>
        {
            WgpuUpscaleMethod::WgslAnime4kV32CnnX2M
        }
        _ => fallback,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prediction(kind: AutoKind, confidence: f32) -> AutoKindPrediction {
        AutoKindPrediction {
            kind,
            confidence,
            probabilities: [0.0; 4],
        }
    }

    #[test]
    fn route_keeps_non_auto_fallbacks_unchanged() {
        assert_eq!(
            route_auto_kind_hint(
                WgpuUpscaleMethod::WgslFsr1EasuRcas,
                Some(&prediction(AutoKind::MangaBw, 0.99))
            ),
            WgpuUpscaleMethod::WgslFsr1EasuRcas
        );
        assert_eq!(
            route_auto_kind_hint(
                WgpuUpscaleMethod::None,
                Some(&prediction(AutoKind::Anime, 0.99))
            ),
            WgpuUpscaleMethod::None
        );
    }

    #[test]
    fn route_uses_anime4k_m_for_confident_drawn_content() {
        assert_eq!(
            route_auto_kind_hint(
                WgpuUpscaleMethod::Auto,
                Some(&prediction(AutoKind::MangaBw, 0.55))
            ),
            WgpuUpscaleMethod::WgslAnime4kV32CnnX2M
        );
        assert_eq!(
            route_auto_kind_hint(
                WgpuUpscaleMethod::Auto,
                Some(&prediction(AutoKind::Anime, 0.65))
            ),
            WgpuUpscaleMethod::WgslAnime4kV32CnnX2M
        );
        assert_eq!(
            route_auto_kind_hint(
                WgpuUpscaleMethod::Auto,
                Some(&prediction(AutoKind::Webtoon, 0.65))
            ),
            WgpuUpscaleMethod::WgslAnime4kV32CnnX2M
        );
    }

    #[test]
    fn route_falls_back_for_photo_and_low_confidence() {
        assert_eq!(
            route_auto_kind_hint(
                WgpuUpscaleMethod::Auto,
                Some(&prediction(AutoKind::Photo, 0.99))
            ),
            WgpuUpscaleMethod::Auto
        );
        assert_eq!(
            route_auto_kind_hint(
                WgpuUpscaleMethod::Auto,
                Some(&prediction(AutoKind::Anime, 0.64))
            ),
            WgpuUpscaleMethod::Auto
        );
        assert_eq!(
            route_auto_kind_hint(WgpuUpscaleMethod::Auto, None),
            WgpuUpscaleMethod::Auto
        );
    }
}
