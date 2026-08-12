use super::{clamp_target_long_edge, CachedPageKey, DecodeOptions, PreparedPage};
#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
use crate::core::perf_trace::{self, PerfField};
use crate::core::source::{PageId, SharedSource};
use lru::LruCache;
use std::collections::VecDeque;
use std::sync::Arc;
#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
use std::time::Duration;

const PUBLISHED_APP_CACHE_HINT_LIMIT: usize = 64;

pub(super) type PublishedAppCacheHints = VecDeque<CachedPageKey>;

pub(super) fn page_cache_key(
    book_id: &str,
    source_cache_id: u64,
    page_id: PageId,
    target_long_edge: u32,
    decode: DecodeOptions,
) -> String {
    format!(
        "{book_id}:{source_cache_id}:{}:{}:{}",
        page_id.0,
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

pub(super) fn clear_cache_on_source_or_decode_change(
    source: &Option<SharedSource>,
    previous_book_id: Option<&str>,
    previous_source_cache_id: Option<u64>,
    previous_decode: DecodeOptions,
    current_decode: DecodeOptions,
    cache: &mut LruCache<String, Arc<PreparedPage>>,
    cache_bytes: &mut usize,
) {
    let current_book_id = source.as_ref().map(|source| source.book_id());
    let current_source_cache_id = source.as_ref().map(|source| source.source_cache_id());
    if previous_book_id != current_book_id
        || previous_source_cache_id != current_source_cache_id
        || previous_decode != current_decode
    {
        cache.clear();
        *cache_bytes = 0;
    }
}

pub(super) fn clear_published_app_cache_hints_on_context_change(
    source: &Option<SharedSource>,
    previous_book_id: Option<&str>,
    previous_source_cache_id: Option<u64>,
    previous_decode: DecodeOptions,
    previous_target_long_edge: u32,
    current_decode: DecodeOptions,
    current_target_long_edge: u32,
    hints: &mut PublishedAppCacheHints,
) {
    let current_book_id = source.as_ref().map(|source| source.book_id());
    let current_source_cache_id = source.as_ref().map(|source| source.source_cache_id());
    if previous_book_id != current_book_id
        || previous_source_cache_id != current_source_cache_id
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
    page_id: PageId,
    target_long_edge: u32,
    decode: DecodeOptions,
) -> bool {
    !visible
        && hints
            .iter()
            .any(|cached| cached.covers(page_id, target_long_edge, decode))
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
    previous_instance_id: Option<u64>,
) {
    let current_book_id = source.as_ref().map(|source| source.book_id());
    let current_instance_id = source.as_ref().map(|source| source.source_instance_id());
    // Bump when the instance id changes (a same-book_id snapshot refresh, which
    // must kill in-flight results keyed to the old snapshot) OR when book_id
    // changes (a book switch). Test fakes all report instance 0, so the book_id
    // change still bumps and preserves existing behavior.
    if current_book_id.is_some()
        && (previous_book_id != current_book_id || previous_instance_id != current_instance_id)
    {
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
        should_skip_published_app_cache_hint, update_book_epoch, PublishedAppCacheHints,
        PUBLISHED_APP_CACHE_HINT_LIMIT,
    };
    use crate::core::source::{BookSource, PageId, SharedSource, SourceError};
    use crate::core::state::{CpuScaleFilter, DecoderPreference, DecoderPreferences};
    use crate::core::worker::{
        CachedPageKey, DecodeBackend, DecodeOptions, PagePixels, PreparedPage, MAX_TARGET_LONG_EDGE,
    };
    use lru::LruCache;
    use std::num::NonZeroUsize;
    use std::path::Path;
    use std::sync::Arc;

    #[test]
    fn worker_cache_key_tracks_decode_options() {
        let normal = page_cache_key("book", 7, PageId(1), 2048, DecodeOptions::default());
        let exif = page_cache_key(
            "book",
            7,
            PageId(1),
            2048,
            DecodeOptions {
                apply_exif_orientation: true,
                ..DecodeOptions::default()
            },
        );
        let icc = page_cache_key(
            "book",
            7,
            PageId(1),
            2048,
            DecodeOptions {
                apply_embedded_icc: true,
                ..DecodeOptions::default()
            },
        );
        let lanczos = page_cache_key(
            "book",
            7,
            PageId(1),
            2048,
            DecodeOptions {
                cpu_downscale_filter: CpuScaleFilter::Lanczos3,
                ..DecodeOptions::default()
            },
        );
        let upscaled = page_cache_key(
            "book",
            7,
            PageId(1),
            2048,
            DecodeOptions {
                allow_display_upscale: true,
                ..DecodeOptions::default()
            },
        );
        let conservative_prepare = page_cache_key(
            "book",
            7,
            PageId(1),
            2048,
            DecodeOptions {
                fast_sampled_scaled_decode: false,
                ..DecodeOptions::default()
            },
        );

        assert_ne!(normal, exif);
        assert_ne!(normal, icc);
        assert_ne!(normal, lanczos);
        assert_ne!(normal, upscaled);
        assert_ne!(normal, conservative_prepare);

        let zune_jpeg = page_cache_key(
            "book",
            7,
            PageId(1),
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
        let small_key = page_cache_key("book", 7, PageId(1), 2048, decode);
        let huge_key = page_cache_key("book", 7, PageId(1), MAX_TARGET_LONG_EDGE + 1, decode);

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
        remember_published_app_cache_hint(&mut hints, CachedPageKey::new(PageId(6), 4096, decode));

        assert!(should_skip_published_app_cache_hint(
            &hints,
            false,
            PageId(6),
            4096,
            decode
        ));
        assert!(!should_skip_published_app_cache_hint(
            &hints,
            true,
            PageId(6),
            4096,
            decode
        ));
    }

    #[test]
    fn published_app_cache_hint_is_bounded_and_recent() {
        let decode = DecodeOptions::default();
        let mut hints = PublishedAppCacheHints::new();
        for index in 0..=PUBLISHED_APP_CACHE_HINT_LIMIT {
            remember_published_app_cache_hint(
                &mut hints,
                CachedPageKey::new(PageId(index as u32), 4096, decode),
            );
        }

        assert_eq!(hints.len(), PUBLISHED_APP_CACHE_HINT_LIMIT);
        assert!(!should_skip_published_app_cache_hint(
            &hints,
            false,
            PageId(0),
            4096,
            decode
        ));
        assert!(should_skip_published_app_cache_hint(
            &hints,
            false,
            PageId(PUBLISHED_APP_CACHE_HINT_LIMIT as u32),
            4096,
            decode
        ));
    }

    #[test]
    fn worker_cache_key_survives_same_book_snapshot_swap() {
        // A refresh inserts a new page at the front: the page interned as id 0
        // keeps id 0 but moves from index 0 to index 1. Its worker LRU key is
        // built from (book_id, page_id), so it must be identical across the swap
        // even though the index changed.
        let before: SharedSource = Arc::new(RemapSource {
            book_id: "same-book".to_owned(),
            instance_id: 1,
            cache_id: 7,
            index_to_id: vec![0, 1],
        });
        let after: SharedSource = Arc::new(RemapSource {
            book_id: "same-book".to_owned(),
            instance_id: 2,
            cache_id: 7,
            index_to_id: vec![2, 0, 1],
        });
        let decode = DecodeOptions::default();

        let before_index = 0;
        let after_index = 1;
        assert_ne!(before_index, after_index);
        let before_id = before.page_id(before_index).unwrap();
        let after_id = after.page_id(after_index).unwrap();
        assert_eq!(before_id, after_id);

        let before_key = page_cache_key(
            before.book_id(),
            before.source_cache_id(),
            before_id,
            2048,
            decode,
        );
        let after_key = page_cache_key(
            after.book_id(),
            after.source_cache_id(),
            after_id,
            2048,
            decode,
        );
        assert_eq!(before_key, after_key);
        assert_ne!(
            before_key,
            page_cache_key(before.book_id(), 8, before_id, 2048, decode)
        );
    }

    #[test]
    fn update_book_epoch_bumps_on_instance_change_and_book_change_only() {
        let decode_source = |book_id: &str, instance_id: u64| -> SharedSource {
            Arc::new(RemapSource {
                book_id: book_id.to_owned(),
                instance_id,
                cache_id: instance_id,
                index_to_id: vec![0],
            })
        };

        // Same book_id, instance id changes (a real refresh): bump.
        let mut epoch = 0;
        let source = Some(decode_source("book", 2));
        update_book_epoch(&mut epoch, &source, Some("book"), Some(1));
        assert_eq!(epoch, 1);

        // book_id changes with test-fake instance 0 on both sides: bump.
        let mut epoch = 0;
        let source = Some(decode_source("next", 0));
        update_book_epoch(&mut epoch, &source, Some("book"), Some(0));
        assert_eq!(epoch, 1);

        // Nothing changed: no bump.
        let mut epoch = 0;
        let source = Some(decode_source("book", 5));
        update_book_epoch(&mut epoch, &source, Some("book"), Some(5));
        assert_eq!(epoch, 0);
    }

    struct RemapSource {
        book_id: String,
        instance_id: u64,
        cache_id: u64,
        index_to_id: Vec<u32>,
    }

    impl BookSource for RemapSource {
        fn title(&self) -> &str {
            "remap"
        }

        fn source_path(&self) -> &Path {
            Path::new("remap-source")
        }

        fn book_id(&self) -> &str {
            &self.book_id
        }

        fn page_count(&self) -> usize {
            self.index_to_id.len()
        }

        fn page_name(&self, index: usize) -> Option<&str> {
            (index < self.index_to_id.len()).then_some("page.png")
        }

        fn read_page(&self, index: usize) -> Result<Vec<u8>, SourceError> {
            if index < self.index_to_id.len() {
                Ok(vec![0])
            } else {
                Err(SourceError::InvalidPage {
                    index,
                    page_count: self.index_to_id.len(),
                })
            }
        }

        fn page_id(&self, index: usize) -> Option<PageId> {
            self.index_to_id.get(index).copied().map(PageId)
        }

        fn page_index_for_id(&self, id: PageId) -> Option<usize> {
            self.index_to_id.iter().position(|&mapped| mapped == id.0)
        }

        fn source_instance_id(&self) -> u64 {
            self.instance_id
        }

        fn source_cache_id(&self) -> u64 {
            self.cache_id
        }
    }

    fn test_prepared_page(byte_size: usize, target_long_edge: u32) -> PreparedPage {
        PreparedPage {
            pixels: PagePixels::Rgba(vec![0u8; byte_size].into()),
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
