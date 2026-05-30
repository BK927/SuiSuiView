use super::{PageCacheKey, TextureCacheKey, TextureEntry};
use crate::app::SuiSuiViewApp;
use crate::core::worker::{PreparedPage, MAX_TARGET_LONG_EDGE};
use eframe::egui;
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
        if self.target_long_edge > MAX_TARGET_LONG_EDGE {
            return;
        }

        self.drop_original_inspection_cache_entries();
        self.pending_gpu_original_inspection_cleanup = true;
        ctx.request_repaint();
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        self.record_cache_snapshot("leave_original_inspection");
    }

    fn drop_original_inspection_cache_entries(&mut self) {
        for (key, byte_size) in drop_original_inspection_pages_from_cache(&mut self.decoded_pages) {
            self.decoded_bytes = self.decoded_bytes.saturating_sub(byte_size);
            self.drop_textures_for_page(key);
        }

        for (key, byte_size) in drop_original_inspection_pages_from_cache(&mut self.upscaled_pages)
        {
            self.upscaled_bytes = self.upscaled_bytes.saturating_sub(byte_size);
            self.drop_textures_for_page(key);
        }

        for key in original_inspection_texture_keys(&self.textures) {
            let _ = self.textures.pop(&key);
        }
    }
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
    use super::{drop_original_inspection_pages_from_cache, original_inspection_page_keys};
    use crate::app::PageCacheKey;
    use crate::core::worker::{
        DecodeBackend, DecodeOptions, PreparedPage, MAX_TARGET_LONG_EDGE, PREVIEW_TARGET_LONG_EDGE,
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

    fn test_prepared_page(target_long_edge: u32, byte_size: usize) -> Arc<PreparedPage> {
        Arc::new(PreparedPage {
            rgba: Arc::<[u8]>::from([255, 255, 255, 255]),
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
