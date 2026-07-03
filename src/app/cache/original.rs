use super::{PageCacheKey, TextureCacheKey, TextureEntry};
use crate::app::perf;
use crate::app::SuiSuiViewApp;
use crate::core::worker::{PreparedPage, PreparedTargetIntent, MAX_TARGET_LONG_EDGE};
use lru::LruCache;
use std::sync::Arc;
use std::time::{Duration, Instant};

const ORIGINAL_INSPECTION_CLEANUP_DELAY: Duration = Duration::from_millis(16);

impl SuiSuiViewApp {
    pub(in crate::app) fn schedule_original_inspection_cache_cleanup(
        &mut self,
        ctx: &egui::Context,
    ) {
        let cleanup_at = Instant::now() + ORIGINAL_INSPECTION_CLEANUP_DELAY;
        self.pending_original_inspection_cache_cleanup_at = Some(cleanup_at);
        ctx.request_repaint_after(ORIGINAL_INSPECTION_CLEANUP_DELAY);
    }

    pub(in crate::app) fn original_inspection_cache_cleanup_pending(&self) -> bool {
        self.pending_original_inspection_cache_cleanup_at.is_some()
    }

    pub(in crate::app) fn drain_pending_original_inspection_cache_cleanup(
        &mut self,
        ctx: &egui::Context,
    ) {
        let Some(cleanup_at) = self.pending_original_inspection_cache_cleanup_at else {
            return;
        };
        let now = Instant::now();
        if now < cleanup_at {
            ctx.request_repaint_after(cleanup_at.saturating_duration_since(now));
            return;
        }

        self.pending_original_inspection_cache_cleanup_at = None;
        if self.current_prepared_target_intent() != PreparedTargetIntent::NormalNavigation {
            return;
        }

        self.drop_original_inspection_cache_entries();
        self.pending_gpu_original_inspection_cleanup = true;
        ctx.request_repaint();
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        self.record_cache_snapshot("leave_high_target_display");
    }

    pub(in crate::app) fn drop_original_after_texture_upload_if_enabled(
        &mut self,
        key: PageCacheKey,
    ) -> bool {
        if !perf::original_texture_only_enabled() {
            return false;
        }
        if !self
            .current_prepared_target_intent()
            .is_original_inspection()
        {
            return false;
        }
        if let Some(byte_size) =
            drop_original_page_after_texture_upload_from_cache(&mut self.decoded_pages, key)
        {
            self.decoded_bytes = self.decoded_bytes.saturating_sub(byte_size);
            true
        } else {
            false
        }
    }

    pub(in crate::app) fn request_original_texture_only_decode_if_needed(&mut self) {
        if !perf::original_texture_only_enabled()
            || !self
                .current_prepared_target_intent()
                .is_original_inspection()
            || self.target_long_edge <= MAX_TARGET_LONG_EDGE
            || self.source.is_none()
        {
            return;
        }

        let decode = self.decode_options();
        let missing_visible_original = self.spread_indices().iter().any(|index| {
            let key = PageCacheKey {
                index: *index,
                target_long_edge: self.target_long_edge,
                decode,
            };
            !self.page_errors.contains_key(&key) && self.decoded_pages.peek(&key).is_none()
        });
        if !missing_visible_original {
            return;
        }

        self.worker.set_page(
            self.worker_center_page(),
            self.last_nav_direction,
            self.target_long_edge,
            self.visible_page_count(),
            self.worker_options(),
        );
    }

    fn drop_original_inspection_cache_entries(&mut self) {
        for (key, byte_size) in drop_original_inspection_pages_from_cache(&mut self.decoded_pages) {
            self.decoded_bytes = self.decoded_bytes.saturating_sub(byte_size);
            self.drop_textures_for_page(key);
        }

        for key in original_inspection_texture_keys(&self.textures) {
            let _ = self.textures.pop(&key);
        }
    }
}

fn drop_original_page_after_texture_upload_from_cache(
    cache: &mut LruCache<PageCacheKey, Arc<PreparedPage>>,
    key: PageCacheKey,
) -> Option<usize> {
    if key.target_long_edge <= MAX_TARGET_LONG_EDGE {
        return None;
    }
    cache.pop(&key).map(|page| page.byte_size)
}

fn drop_original_inspection_pages_from_cache(
    cache: &mut LruCache<PageCacheKey, Arc<PreparedPage>>,
) -> Vec<(PageCacheKey, usize)> {
    let keys = original_inspection_page_keys(cache);
    keys.into_iter()
        .filter_map(|key| cache.pop(&key).map(|page| (key, page.byte_size)))
        .collect()
}

fn original_inspection_page_keys(
    cache: &LruCache<PageCacheKey, Arc<PreparedPage>>,
) -> Vec<PageCacheKey> {
    cache
        .iter()
        .filter_map(|(key, _page)| (key.target_long_edge > MAX_TARGET_LONG_EDGE).then_some(*key))
        .collect()
}

fn original_inspection_texture_keys(
    cache: &LruCache<TextureCacheKey, TextureEntry>,
) -> Vec<TextureCacheKey> {
    cache
        .iter()
        .filter_map(|(key, _entry)| {
            (key.page.target_long_edge > MAX_TARGET_LONG_EDGE).then_some(*key)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        drop_original_inspection_pages_from_cache,
        drop_original_page_after_texture_upload_from_cache, original_inspection_page_keys,
    };
    use crate::app::PageCacheKey;
    use crate::core::worker::{
        DecodeBackend, DecodeOptions, PagePixels, PreparedPage, MAX_TARGET_LONG_EDGE,
        PREVIEW_TARGET_LONG_EDGE,
    };
    use lru::LruCache;
    use std::num::NonZeroUsize;
    use std::sync::Arc;

    #[test]
    fn original_inspection_cache_drop_removes_only_original_targets() {
        let mut cache = LruCache::new(NonZeroUsize::new(4).unwrap());
        let decode = DecodeOptions::default();
        let preview = PageCacheKey {
            index: 7,
            target_long_edge: PREVIEW_TARGET_LONG_EDGE,
            decode,
        };
        let navigation = PageCacheKey {
            target_long_edge: MAX_TARGET_LONG_EDGE,
            ..preview
        };
        let original = PageCacheKey {
            target_long_edge: MAX_TARGET_LONG_EDGE + 1,
            ..preview
        };
        let other_original = PageCacheKey {
            index: 8,
            ..original
        };

        cache.put(preview, test_prepared_page(PREVIEW_TARGET_LONG_EDGE, 4));
        cache.put(navigation, test_prepared_page(MAX_TARGET_LONG_EDGE, 8));
        cache.put(original, test_prepared_page(MAX_TARGET_LONG_EDGE + 1, 16));
        cache.put(
            other_original,
            test_prepared_page(MAX_TARGET_LONG_EDGE + 1, 32),
        );

        let mut keys = original_inspection_page_keys(&cache);
        keys.sort_by_key(|key| key.index);
        assert_eq!(keys, vec![original, other_original]);

        let mut dropped = drop_original_inspection_pages_from_cache(&mut cache);
        dropped.sort_by_key(|(key, _byte_size)| key.index);

        assert_eq!(dropped, vec![(original, 16), (other_original, 32)]);
        assert!(cache.peek(&preview).is_some());
        assert!(cache.peek(&navigation).is_some());
        assert!(cache.peek(&original).is_none());
        assert!(cache.peek(&other_original).is_none());
    }

    #[test]
    fn original_texture_upload_drop_removes_only_decoded_original() {
        let mut cache = LruCache::new(NonZeroUsize::new(4).unwrap());
        let decode = DecodeOptions::default();
        let original = PageCacheKey {
            index: 3,
            target_long_edge: MAX_TARGET_LONG_EDGE + 1,
            decode,
        };
        let normal = PageCacheKey {
            target_long_edge: MAX_TARGET_LONG_EDGE,
            ..original
        };

        cache.put(normal, test_prepared_page(MAX_TARGET_LONG_EDGE, 8));
        cache.put(original, test_prepared_page(MAX_TARGET_LONG_EDGE + 1, 16));

        assert_eq!(
            drop_original_page_after_texture_upload_from_cache(&mut cache, normal),
            None
        );
        assert_eq!(
            drop_original_page_after_texture_upload_from_cache(&mut cache, original),
            Some(16)
        );
        assert!(cache.peek(&normal).is_some());
        assert!(cache.peek(&original).is_none());
    }

    fn test_prepared_page(target_long_edge: u32, byte_size: usize) -> Arc<PreparedPage> {
        Arc::new(PreparedPage {
            pixels: PagePixels::Rgba(Arc::<[u8]>::from([255, 255, 255, 255])),
            original_width: 1,
            original_height: 1,
            display_width: 1,
            display_height: 1,
            byte_size,
            target_long_edge,
            decode_backend: DecodeBackend::ImageCrate,
            notice: None,
        })
    }
}
