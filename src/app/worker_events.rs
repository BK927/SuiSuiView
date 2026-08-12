use super::{
    refresh, should_allow_cpu_display_upscale, OpenOrigin, PageCacheKey, PageMetrics,
    SuiSuiViewApp, ViewMode, TEXTURE_PRESENT_REPAINT_DELAY,
};
use crate::core::source::{BookSource, PageId};
use crate::core::state::{DecodeMode, DecoderPreferences, WgpuUpscaleMethod};
use crate::core::worker::{
    DecodeOptions, DecodeStrategy, WorkerEvent, WorkerOptions, PREVIEW_TARGET_LONG_EDGE,
};
use std::time::{Duration, Instant};

const WORKER_EVENT_DRAIN_BUDGET: Duration = Duration::from_millis(4);
const MAX_WORKER_EVENTS_RECEIVED_PER_FRAME: usize = 128;
const MAX_DEFERRED_WORKER_EVENTS_PER_FRAME: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WorkerEventRoute {
    DropStale,
    PaintCritical,
    Background,
}

pub(super) fn worker_event_receive_allowed(received: usize, elapsed: Duration) -> bool {
    received < MAX_WORKER_EVENTS_RECEIVED_PER_FRAME
        && (received == 0 || elapsed < WORKER_EVENT_DRAIN_BUDGET)
}

pub(super) fn deferred_worker_event_allowed(processed: usize, elapsed: Duration) -> bool {
    processed < MAX_DEFERRED_WORKER_EVENTS_PER_FRAME
        && (processed == 0 || elapsed < WORKER_EVENT_DRAIN_BUDGET)
}

pub(super) fn worker_event_route_for(
    source_is_current: bool,
    decode_matches: bool,
    target_is_relevant: bool,
    page_index: Option<usize>,
    paint_critical: bool,
) -> WorkerEventRoute {
    if !source_is_current || !decode_matches || !target_is_relevant || page_index.is_none() {
        WorkerEventRoute::DropStale
    } else if paint_critical {
        WorkerEventRoute::PaintCritical
    } else {
        WorkerEventRoute::Background
    }
}

pub(super) fn worker_event_page_is_paint_critical_for(
    index: usize,
    current_page: usize,
    visible_indices: &[usize],
    pending_indices: &[usize],
    transition_indices: &[usize],
) -> bool {
    index == current_page
        || visible_indices.contains(&index)
        || pending_indices.contains(&index)
        || transition_indices.contains(&index)
}

impl SuiSuiViewApp {
    pub(in crate::app) fn drain_worker_events(&mut self) {
        let started = Instant::now();
        let mut decoded_cache_changed = false;
        let mut received = 0;
        let receive_limit_hit = loop {
            if !worker_event_receive_allowed(received, started.elapsed()) {
                break true;
            }
            let Some(event) = self.worker.try_recv() else {
                break false;
            };
            received += 1;
            match self.worker_event_route(&event) {
                WorkerEventRoute::DropStale => {}
                WorkerEventRoute::PaintCritical => {
                    decoded_cache_changed |= self.handle_worker_event(event);
                }
                WorkerEventRoute::Background => self.deferred_worker_events.push_back(event),
            }
        };

        let mut deferred_processed = 0;
        while deferred_worker_event_allowed(deferred_processed, started.elapsed()) {
            let Some(event) = self.deferred_worker_events.pop_front() else {
                break;
            };
            deferred_processed += 1;
            match self.worker_event_route(&event) {
                WorkerEventRoute::DropStale => {}
                WorkerEventRoute::PaintCritical | WorkerEventRoute::Background => {
                    decoded_cache_changed |= self.handle_worker_event(event);
                }
            }
        }

        if receive_limit_hit || !self.deferred_worker_events.is_empty() {
            self.egui_ctx
                .request_repaint_after(TEXTURE_PRESENT_REPAINT_DELAY);
        }

        if decoded_cache_changed && !self.original_inspection_cache_cleanup_pending() {
            self.prune_decoded_cache();
        }
    }

    fn worker_event_route(&self, event: &WorkerEvent) -> WorkerEventRoute {
        let (book_id, source_instance_id, page_id, target_long_edge, decode) = match event {
            WorkerEvent::PageReady {
                book_id,
                source_instance_id,
                page_id,
                decode,
                page,
            } => (
                book_id,
                *source_instance_id,
                *page_id,
                page.target_long_edge,
                *decode,
            ),
            WorkerEvent::PageFailed {
                book_id,
                source_instance_id,
                page_id,
                target_long_edge,
                decode,
                ..
            } => (
                book_id,
                *source_instance_id,
                *page_id,
                *target_long_edge,
                *decode,
            ),
        };
        let source_is_current = worker_event_source_is_current(
            self.book_id.as_deref(),
            self.source.as_deref(),
            book_id,
            source_instance_id,
        );
        if !source_is_current
            || decode != self.decode_options()
            || !self.target_is_relevant(target_long_edge)
        {
            return WorkerEventRoute::DropStale;
        }
        let Some(page_index) = resolve_worker_event_index(self.source.as_deref(), page_id) else {
            return WorkerEventRoute::DropStale;
        };
        worker_event_route_for(
            true,
            true,
            true,
            Some(page_index),
            self.worker_event_page_is_paint_critical(page_index),
        )
    }

    fn worker_event_page_is_paint_critical(&self, index: usize) -> bool {
        let paged_visible;
        let visible_indices = if self.view_mode == ViewMode::VerticalStrip {
            self.strip_visible_indices.as_slice()
        } else {
            paged_visible = self.spread_indices();
            paged_visible.as_slice()
        };
        let pending_indices = self
            .pending_page_turn
            .map(|pending| self.spread_indices_for(pending.target))
            .unwrap_or_default();
        let transition_indices = self
            .transition
            .as_ref()
            .map(|transition| transition.from_indices.as_slice())
            .unwrap_or_default();
        worker_event_page_is_paint_critical_for(
            index,
            self.current_page,
            visible_indices,
            &pending_indices,
            transition_indices,
        )
    }

    fn handle_worker_event(&mut self, event: WorkerEvent) -> bool {
        let mut decoded_cache_changed = false;
        match event {
            WorkerEvent::PageReady {
                book_id,
                source_instance_id,
                page_id,
                decode,
                page,
            } if worker_event_source_is_current(
                self.book_id.as_deref(),
                self.source.as_deref(),
                &book_id,
                source_instance_id,
            ) && decode == self.decode_options()
                && self.target_is_relevant(page.target_long_edge) =>
            {
                // Drop events for pages that vanished from the current snapshot
                // mid-flight so orphaned ids never enter the cache.
                let Some(index) = resolve_worker_event_index(self.source.as_deref(), page_id)
                else {
                    return false;
                };
                let key = PageCacheKey {
                    page_id,
                    target_long_edge: page.target_long_edge,
                    decode,
                };
                self.page_errors.remove(&key);
                if let Some(notice) = page.notice.as_ref() {
                    self.set_status(notice.clone());
                }
                self.page_metrics
                    .insert(page_id, PageMetrics::from_page(&page));
                self.note_strip_dims_changed();
                self.insert_prepared_page(key, page.clone());
                decoded_cache_changed = true;
                self.maybe_enqueue_upscale_probe(key, page);
                self.commit_pending_page_turn_if_ready();
                if self.spread_indices().contains(&index) {
                    self.egui_ctx
                        .request_repaint_after(TEXTURE_PRESENT_REPAINT_DELAY);
                }
                #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
                self.record_cache_snapshot("page_ready");
            }
            WorkerEvent::PageFailed {
                book_id,
                source_instance_id,
                page_id,
                target_long_edge,
                decode,
                message,
            } if worker_event_source_is_current(
                self.book_id.as_deref(),
                self.source.as_deref(),
                &book_id,
                source_instance_id,
            ) && decode == self.decode_options()
                && self.target_is_relevant(target_long_edge) =>
            {
                let Some(index) = resolve_worker_event_index(self.source.as_deref(), page_id)
                else {
                    return false;
                };
                self.page_errors.insert(
                    PageCacheKey {
                        page_id,
                        target_long_edge,
                        decode,
                    },
                    message,
                );
                // A folder page that failed because its file vanished (not a
                // corrupt decode) means the snapshot is stale; rebuild it.
                if self.open_origin == Some(OpenOrigin::Folder)
                    && self
                        .source
                        .as_deref()
                        .is_some_and(|source| refresh::folder_page_file_vanished(source, index))
                {
                    self.request_folder_refresh();
                }
                self.commit_pending_page_turn_if_ready();
            }
            _ => {}
        }
        decoded_cache_changed
    }

    pub(in crate::app) fn target_is_relevant(&self, target_long_edge: u32) -> bool {
        (self.settings.progressive_preview_enabled && target_long_edge == PREVIEW_TARGET_LONG_EDGE)
            || target_long_edge == self.target_long_edge
            || self
                .transition
                .as_ref()
                .is_some_and(|transition| target_long_edge == transition.target_long_edge)
    }

    pub(in crate::app) fn decode_options(&self) -> DecodeOptions {
        let strategy = match self.settings.decode_mode {
            DecodeMode::AutoFast => DecodeStrategy::Auto,
            DecodeMode::Compatibility => DecodeStrategy::ImageCrate,
            DecodeMode::Custom => DecodeStrategy::Auto,
        };
        let decoder_preferences = if matches!(self.settings.decode_mode, DecodeMode::Custom) {
            self.settings.decoder_preferences
        } else {
            DecoderPreferences::default()
        };
        DecodeOptions {
            strategy,
            decoder_preferences,
            fast_sampled_scaled_decode: self.settings.fast_sampled_scaled_decode,
            cpu_upscale_filter: self.settings.cpu_upscale_filter,
            cpu_downscale_filter: crate::core::state::CPU_DOWNSCALE_FILTER,
            allow_display_upscale: self.should_allow_display_upscale(),
            apply_exif_orientation: self.settings.apply_exif_orientation,
            apply_embedded_icc: self.settings.apply_embedded_icc,
        }
    }

    fn should_allow_display_upscale(&self) -> bool {
        should_allow_cpu_display_upscale(
            self.fit_mode,
            self.manual_zoom,
            self.gpu_display_upscale_can_own_upscale(),
            self.glow_kernel_available(),
            self.settings.cpu_upscale_filter,
        )
    }

    fn gpu_display_upscale_can_own_upscale(&self) -> bool {
        self.active_wgpu_upscale_method() != WgpuUpscaleMethod::None
    }

    pub(in crate::app) fn worker_options(&self) -> WorkerOptions {
        WorkerOptions {
            decode: self.decode_options(),
            target_intent: self.current_prepared_target_intent(),
            prefetch_enabled: self.settings.prefetch_enabled,
            progressive_preview_enabled: self.settings.progressive_preview_enabled,
            cache_bytes: self.worker_cache_budget_bytes(),
            app_cached_pages: self.app_cached_page_keys(),
        }
    }
}

/// Current index of a worker event's page in `source`, or None when the page
/// vanished from the snapshot mid-flight (the event must then be dropped so an
/// orphaned id never enters the cache).
pub(super) fn resolve_worker_event_index(
    source: Option<&dyn BookSource>,
    page_id: PageId,
) -> Option<usize> {
    source?.page_index_for_id(page_id)
}

pub(super) fn worker_event_source_is_current(
    current_book_id: Option<&str>,
    source: Option<&dyn BookSource>,
    event_book_id: &str,
    event_source_instance_id: u64,
) -> bool {
    current_book_id == Some(event_book_id)
        && source.is_some_and(|source| source.source_instance_id() == event_source_instance_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::source::SourceError;
    use std::path::Path;

    #[test]
    fn unmappable_worker_event_index_is_dropped() {
        // A source whose only page is id 0: an event for a vanished id (5)
        // resolves to None so `handle_worker_event` drops it before caching.
        struct OnePageSource;
        impl BookSource for OnePageSource {
            fn title(&self) -> &str {
                "one"
            }
            fn source_path(&self) -> &Path {
                Path::new("one")
            }
            fn book_id(&self) -> &str {
                "one"
            }
            fn page_count(&self) -> usize {
                1
            }
            fn page_name(&self, _index: usize) -> Option<&str> {
                Some("page.png")
            }
            fn read_page(&self, _index: usize) -> Result<Vec<u8>, SourceError> {
                Ok(Vec::new())
            }
        }

        let source = OnePageSource;
        assert_eq!(
            resolve_worker_event_index(Some(&source), PageId(0)),
            Some(0)
        );
        assert_eq!(resolve_worker_event_index(Some(&source), PageId(5)), None);
        assert_eq!(resolve_worker_event_index(None, PageId(0)), None);
    }

    #[test]
    fn drain_limits_allow_one_event_but_bound_later_work() {
        assert!(worker_event_receive_allowed(0, Duration::from_secs(1)));
        assert!(worker_event_receive_allowed(127, Duration::ZERO));
        assert!(!worker_event_receive_allowed(128, Duration::ZERO));
        assert!(!worker_event_receive_allowed(1, Duration::from_millis(4)));

        assert!(deferred_worker_event_allowed(0, Duration::from_secs(1)));
        assert!(deferred_worker_event_allowed(127, Duration::ZERO));
        assert!(!deferred_worker_event_allowed(128, Duration::ZERO));
        assert!(!deferred_worker_event_allowed(1, Duration::from_millis(4)));
    }

    #[test]
    fn routing_drops_stale_and_prioritizes_paint_dependencies() {
        for stale in [
            (false, true, true, Some(0)),
            (true, false, true, Some(0)),
            (true, true, false, Some(0)),
            (true, true, true, None),
        ] {
            assert_eq!(
                worker_event_route_for(stale.0, stale.1, stale.2, stale.3, true),
                WorkerEventRoute::DropStale
            );
        }
        assert_eq!(
            worker_event_route_for(true, true, true, Some(0), true),
            WorkerEventRoute::PaintCritical
        );
        assert_eq!(
            worker_event_route_for(true, true, true, Some(4), false),
            WorkerEventRoute::Background
        );

        assert!(worker_event_page_is_paint_critical_for(2, 2, &[], &[], &[]));
        assert!(worker_event_page_is_paint_critical_for(
            5,
            2,
            &[4, 5, 6],
            &[],
            &[]
        ));
        assert!(worker_event_page_is_paint_critical_for(
            8,
            2,
            &[2, 3],
            &[7, 8],
            &[]
        ));
        assert!(worker_event_page_is_paint_critical_for(
            1,
            2,
            &[2, 3],
            &[],
            &[0, 1]
        ));
        assert!(!worker_event_page_is_paint_critical_for(
            9,
            2,
            &[2, 3],
            &[7, 8],
            &[0, 1]
        ));
    }

    #[test]
    fn event_must_match_the_current_source_instance() {
        struct InstanceSource;
        impl BookSource for InstanceSource {
            fn title(&self) -> &str {
                "current"
            }
            fn source_path(&self) -> &Path {
                Path::new("current")
            }
            fn book_id(&self) -> &str {
                "colliding-book"
            }
            fn page_count(&self) -> usize {
                1
            }
            fn page_name(&self, _index: usize) -> Option<&str> {
                Some("page.png")
            }
            fn read_page(&self, _index: usize) -> Result<Vec<u8>, SourceError> {
                Ok(Vec::new())
            }
            fn source_instance_id(&self) -> u64 {
                22
            }
        }

        let source = InstanceSource;
        assert!(worker_event_source_is_current(
            Some("colliding-book"),
            Some(&source),
            "colliding-book",
            22,
        ));
        assert!(!worker_event_source_is_current(
            Some("colliding-book"),
            Some(&source),
            "colliding-book",
            21,
        ));
        assert!(!worker_event_source_is_current(
            Some("different-book"),
            Some(&source),
            "colliding-book",
            22,
        ));
    }
}
