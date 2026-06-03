use super::{clamp_target_long_edge, CachedPageKey, DecodeOptions, PreparedPage};
#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
use crate::core::perf_trace::{self, PerfField};
use crate::core::source::SharedSource;
use lru::LruCache;
use std::collections::VecDeque;
use std::sync::Arc;
#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
use std::time::Duration;

const PUBLISHED_APP_CACHE_HINT_LIMIT: usize = 64;

pub(super) type PublishedAppCacheHints = VecDeque<CachedPageKey>;

pub(super) fn page_cache_key(
    book_id: &str,
    index: usize,
    target_long_edge: u32,
    decode: DecodeOptions,
) -> String {
    format!(
        "{book_id}:{index}:{}:{}",
        clamp_target_long_edge(target_long_edge),
        decode.cache_token()
    )
}

pub(super) fn insert_worker_cache_with_budget(
    cache: &mut LruCache<String, Arc<PreparedPage>>,
    cache_bytes: &mut usize,
    key: String,
    page: &Arc<PreparedPage>,
    budget_bytes: usize,
) -> bool {
    let byte_size = page.byte_size;
    if byte_size > budget_bytes {
        return false;
    }

    if let Some((_evicted_key, evicted_page)) = cache.push(key, page.clone()) {
        *cache_bytes = (*cache_bytes).saturating_sub(evicted_page.byte_size);
    }
    *cache_bytes = (*cache_bytes).saturating_add(byte_size);
    true
}

pub(super) fn clear_cache_on_book_or_decode_change(
    source: &Option<SharedSource>,
    previous_book_id: Option<&str>,
    previous_decode: DecodeOptions,
    current_decode: DecodeOptions,
    cache: &mut LruCache<String, Arc<PreparedPage>>,
    cache_bytes: &mut usize,
) {
    let current_book_id = source.as_ref().map(|source| source.book_id());
    if previous_book_id != current_book_id || previous_decode != current_decode {
        cache.clear();
        *cache_bytes = 0;
    }
}

pub(super) fn clear_published_app_cache_hints_on_context_change(
    source: &Option<SharedSource>,
    previous_book_id: Option<&str>,
    previous_decode: DecodeOptions,
    previous_target_long_edge: u32,
    current_decode: DecodeOptions,
    current_target_long_edge: u32,
    hints: &mut PublishedAppCacheHints,
) {
    let current_book_id = source.as_ref().map(|source| source.book_id());
    if previous_book_id != current_book_id
        || previous_decode != current_decode
        || clamp_target_long_edge(previous_target_long_edge)
            != clamp_target_long_edge(current_target_long_edge)
    {
        hints.clear();
    }
}

pub(super) fn should_skip_published_app_cache_hint(
    hints: &PublishedAppCacheHints,
    visible: bool,
    index: usize,
    target_long_edge: u32,
    decode: DecodeOptions,
) -> bool {
    !visible
        && hints
            .iter()
            .any(|cached| cached.covers(index, target_long_edge, decode))
}

pub(super) fn remember_published_app_cache_hint(
    hints: &mut PublishedAppCacheHints,
    key: CachedPageKey,
) {
    if let Some(position) = hints.iter().position(|existing| *existing == key) {
        let _ = hints.remove(position);
    }
    hints.push_back(key);
    while hints.len() > PUBLISHED_APP_CACHE_HINT_LIMIT {
        let _ = hints.pop_front();
    }
}

pub(super) fn update_book_epoch(
    book_epoch: &mut usize,
    source: &Option<SharedSource>,
    previous_book_id: Option<&str>,
) {
    let current_book_id = source.as_ref().map(|source| source.book_id());
    if current_book_id.is_some() && previous_book_id != current_book_id {
        *book_epoch = book_epoch.saturating_add(1);
    }
}

pub(super) fn prune_worker_cache(
    cache: &mut LruCache<String, Arc<PreparedPage>>,
    cache_bytes: &mut usize,
    budget_bytes: usize,
) {
    while *cache_bytes > budget_bytes {
        let Some((_key, page)) = cache.pop_lru() else {
            break;
        };
        *cache_bytes = (*cache_bytes).saturating_sub(page.byte_size);
    }
}

#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
pub(super) fn record_worker_cache_snapshot(
    reason: &'static str,
    page: usize,
    target_long_edge: u32,
    cache_pages: usize,
    cache_bytes: usize,
    cache_budget_bytes: usize,
    cache_hit: bool,
) {
    perf_trace::record_duration(
        "page_worker_cache_snapshot",
        Duration::ZERO,
        &[
            PerfField::Str("reason", reason),
            PerfField::Usize("page", page),
            PerfField::U32("target_long_edge", target_long_edge),
            PerfField::Usize("cache_pages", cache_pages),
            PerfField::Usize("cache_bytes", cache_bytes),
            PerfField::Usize("cache_budget_bytes", cache_budget_bytes),
            PerfField::Bool("cache_hit", cache_hit),
        ],
    );
}

#[cfg(test)]
mod tests {
    use super::{
        insert_worker_cache_with_budget, page_cache_key, remember_published_app_cache_hint,
        should_skip_published_app_cache_hint, PublishedAppCacheHints,
        PUBLISHED_APP_CACHE_HINT_LIMIT,
    };
    use crate::core::state::{CpuScaleFilter, DecoderPreference, DecoderPreferences};
    use crate::core::worker::{
        CachedPageKey, DecodeBackend, DecodeOptions, PreparedPage, MAX_TARGET_LONG_EDGE,
    };
    use lru::LruCache;
    use std::num::NonZeroUsize;
    use std::sync::Arc;

    #[test]
    fn worker_cache_key_tracks_decode_options() {
        let normal = page_cache_key("book", 1, 2048, DecodeOptions::default());
        let exif = page_cache_key(
            "book",
            1,
            2048,
            DecodeOptions {
                apply_exif_orientation: true,
                ..DecodeOptions::default()
            },
        );
        let icc = page_cache_key(
            "book",
            1,
            2048,
            DecodeOptions {
                apply_embedded_icc: true,
                ..DecodeOptions::default()
            },
        );
        let lanczos = page_cache_key(
            "book",
            1,
            2048,
            DecodeOptions {
                cpu_downscaler: CpuScaleFilter::Lanczos3,
                ..DecodeOptions::default()
            },
        );
        let upscaled = page_cache_key(
            "book",
            1,
            2048,
            DecodeOptions {
                allow_display_upscale: true,
                ..DecodeOptions::default()
            },
        );

        assert_ne!(normal, exif);
        assert_ne!(normal, icc);
        assert_ne!(normal, lanczos);
        assert_ne!(normal, upscaled);

        let zune_jpeg = page_cache_key(
            "book",
            1,
            2048,
            DecodeOptions {
                decoder_preferences: DecoderPreferences {
                    jpeg: DecoderPreference::ZuneJpeg,
                    ..DecoderPreferences::default()
                },
                ..DecodeOptions::default()
            },
        );
        assert_ne!(normal, zune_jpeg);
    }

    #[test]
    fn worker_cache_skips_pages_larger_than_budget() {
        let mut cache = LruCache::new(NonZeroUsize::new(4).unwrap());
        let mut cache_bytes = 0usize;
        let decode = DecodeOptions::default();
        let small_key = page_cache_key("book", 1, 2048, decode);
        let huge_key = page_cache_key("book", 1, MAX_TARGET_LONG_EDGE + 1, decode);

        assert!(insert_worker_cache_with_budget(
            &mut cache,
            &mut cache_bytes,
            small_key.clone(),
            &Arc::new(test_prepared_page(4, 2048)),
            8,
        ));
        assert_eq!(cache_bytes, 4);
        assert!(cache.peek(&small_key).is_some());

        assert!(!insert_worker_cache_with_budget(
            &mut cache,
            &mut cache_bytes,
            huge_key.clone(),
            &Arc::new(test_prepared_page(16, MAX_TARGET_LONG_EDGE + 1)),
            8,
        ));
        assert_eq!(cache_bytes, 4);
        assert!(cache.peek(&small_key).is_some());
        assert!(cache.peek(&huge_key).is_none());
    }

    #[test]
    fn published_app_cache_hint_skips_only_prefetch_pages() {
        let decode = DecodeOptions::default();
        let mut hints = PublishedAppCacheHints::new();
        remember_published_app_cache_hint(&mut hints, CachedPageKey::new(6, 4096, decode));

        assert!(should_skip_published_app_cache_hint(
            &hints, false, 6, 4096, decode
        ));
        assert!(!should_skip_published_app_cache_hint(
            &hints, true, 6, 4096, decode
        ));
    }

    #[test]
    fn published_app_cache_hint_is_bounded_and_recent() {
        let decode = DecodeOptions::default();
        let mut hints = PublishedAppCacheHints::new();
        for index in 0..=PUBLISHED_APP_CACHE_HINT_LIMIT {
            remember_published_app_cache_hint(&mut hints, CachedPageKey::new(index, 4096, decode));
        }

        assert_eq!(hints.len(), PUBLISHED_APP_CACHE_HINT_LIMIT);
        assert!(!should_skip_published_app_cache_hint(
            &hints, false, 0, 4096, decode
        ));
        assert!(should_skip_published_app_cache_hint(
            &hints,
            false,
            PUBLISHED_APP_CACHE_HINT_LIMIT,
            4096,
            decode
        ));
    }

    fn test_prepared_page(byte_size: usize, target_long_edge: u32) -> PreparedPage {
        PreparedPage {
            rgba: vec![0u8; byte_size].into(),
            original_width: 1,
            original_height: 1,
            display_width: 1,
            display_height: 1,
            byte_size,
            target_long_edge,
            decode_backend: DecodeBackend::ImageCrate,
            notice: None,
        }
    }
}
