use super::read_ahead::{clear_matching as clear_matching_read_ahead, consume_matching, ReadAhead};
#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
use crate::core::source::PageReadHint;
use crate::core::source::SharedSource;
#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
use crate::core::{perf_trace, perf_trace::PerfField};
use lru::LruCache;
use std::env;
use std::num::NonZeroUsize;
use std::sync::Arc;
#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
use std::time::{Duration, Instant};

// Experiment status (2026-07): opt-in, default OFF. This cache only runs inside
// the page worker loop, so the CLI perf harnesses never exercise it and cannot
// judge it. Its expected beneficiaries are repeat source reads: the progressive
// preview + full-quality double read of the same page and the settle-to-exact
// re-prepare burst after a window resize. Promote to default-on or remove once
// an in-worker benchmark exists.
const SOURCE_BYTES_CACHE_ENV: &str = "SUISUIVIEW_EXPERIMENT_SOURCE_BYTES_CACHE";
const SOURCE_BYTES_CACHE_MB_ENV: &str = "SUISUIVIEW_EXPERIMENT_SOURCE_BYTES_CACHE_MB";
const DEFAULT_SOURCE_BYTES_CACHE_MB: usize = 64;
const MIN_SOURCE_BYTES_CACHE_MB: usize = 8;
const MAX_SOURCE_BYTES_CACHE_MB: usize = 256;
const SOURCE_BYTES_CACHE_ENTRY_LIMIT: usize = 128;

pub(super) enum SourcePageBytes {
    Owned(Vec<u8>),
    Shared(Arc<[u8]>),
}

impl AsRef<[u8]> for SourcePageBytes {
    fn as_ref(&self) -> &[u8] {
        match self {
            Self::Owned(bytes) => bytes,
            Self::Shared(bytes) => bytes,
        }
    }
}

pub(super) struct SourceBytesCache {
    entries: LruCache<String, Arc<[u8]>>,
    bytes: usize,
    budget_bytes: usize,
}

impl SourceBytesCache {
    pub(super) fn from_env() -> Option<Self> {
        source_bytes_cache_budget_from_env().map(Self::new)
    }

    fn new(budget_bytes: usize) -> Self {
        Self {
            entries: LruCache::new(NonZeroUsize::new(SOURCE_BYTES_CACHE_ENTRY_LIMIT).unwrap()),
            bytes: 0,
            budget_bytes,
        }
    }

    fn get(&mut self, book_id: &str, index: usize, book_epoch: usize) -> Option<Arc<[u8]>> {
        let key = source_bytes_key(book_id, index);
        let hit = self.entries.get(&key).cloned();
        record_source_bytes_cache(
            if hit.is_some() { "hit" } else { "miss" },
            index,
            book_epoch,
            hit.as_ref().map_or(0, |bytes| bytes.len()),
            self.entries.len(),
            self.bytes,
            self.budget_bytes,
        );
        hit
    }

    fn insert(
        &mut self,
        book_id: &str,
        index: usize,
        book_epoch: usize,
        bytes: Vec<u8>,
    ) -> SourcePageBytes {
        let byte_len = bytes.len();
        if byte_len > self.budget_bytes {
            record_source_bytes_cache(
                "oversize",
                index,
                book_epoch,
                byte_len,
                self.entries.len(),
                self.bytes,
                self.budget_bytes,
            );
            return SourcePageBytes::Owned(bytes);
        }

        let shared = Arc::<[u8]>::from(bytes);
        let key = source_bytes_key(book_id, index);
        if let Some((_evicted_key, evicted_bytes)) = self.entries.push(key, shared.clone()) {
            self.bytes = self.bytes.saturating_sub(evicted_bytes.len());
        }
        self.bytes = self.bytes.saturating_add(byte_len);
        while self.bytes > self.budget_bytes {
            let Some((_key, evicted_bytes)) = self.entries.pop_lru() else {
                break;
            };
            self.bytes = self.bytes.saturating_sub(evicted_bytes.len());
        }
        record_source_bytes_cache(
            "insert",
            index,
            book_epoch,
            byte_len,
            self.entries.len(),
            self.bytes,
            self.budget_bytes,
        );
        SourcePageBytes::Shared(shared)
    }

    pub(super) fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
    }
}

pub(super) fn read_source_bytes(
    cache: Option<&mut SourceBytesCache>,
    read_ahead: &mut Option<ReadAhead>,
    source: &SharedSource,
    book_id: &str,
    book_epoch: usize,
    index: usize,
) -> Result<SourcePageBytes, String> {
    if let Some(cache) = cache {
        if let Some(bytes) = cache.get(book_id, index, book_epoch) {
            clear_matching_read_ahead(
                read_ahead,
                book_id,
                book_epoch,
                index,
                "source_bytes_cache_hit",
            );
            return Ok(SourcePageBytes::Shared(bytes));
        }
        let bytes = read_uncached_source_bytes(read_ahead, source, book_id, book_epoch, index)?;
        return Ok(cache.insert(book_id, index, book_epoch, bytes));
    }

    read_uncached_source_bytes(read_ahead, source, book_id, book_epoch, index)
        .map(SourcePageBytes::Owned)
}

fn read_uncached_source_bytes(
    read_ahead: &mut Option<ReadAhead>,
    source: &SharedSource,
    book_id: &str,
    book_epoch: usize,
    index: usize,
) -> Result<Vec<u8>, String> {
    consume_matching(read_ahead, book_id, book_epoch, index).unwrap_or_else(|| {
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        let read_hint = source.page_read_hint(index);
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        let read_started = Instant::now();
        let read_result = source.read_page(index).map_err(|error| error.to_string());
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        record_direct_page_read(
            index,
            book_epoch,
            read_started.elapsed(),
            read_result.is_ok(),
            read_hint,
        );
        read_result
    })
}

fn source_bytes_cache_budget_from_env() -> Option<usize> {
    let gate = env::var(SOURCE_BYTES_CACHE_ENV).ok();
    let budget_mb = env::var(SOURCE_BYTES_CACHE_MB_ENV)
        .ok()
        .and_then(|value| value.parse::<usize>().ok());
    source_bytes_cache_budget(gate.as_deref(), budget_mb)
}

fn source_bytes_cache_budget(gate: Option<&str>, budget_mb: Option<usize>) -> Option<usize> {
    if !gate.is_some_and(enabled_value) {
        return None;
    }
    let budget_mb = budget_mb.unwrap_or(DEFAULT_SOURCE_BYTES_CACHE_MB);
    if budget_mb == 0 {
        return None;
    }
    Some(
        budget_mb
            .clamp(MIN_SOURCE_BYTES_CACHE_MB, MAX_SOURCE_BYTES_CACHE_MB)
            .saturating_mul(1024 * 1024),
    )
}

fn enabled_value(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on" | "auto"
    )
}

fn source_bytes_key(book_id: &str, index: usize) -> String {
    format!("{book_id}:{index}")
}

#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
fn record_direct_page_read(
    index: usize,
    book_epoch: usize,
    duration: Duration,
    success: bool,
    hint: Option<PageReadHint>,
) {
    let hint = hint.unwrap_or_else(PageReadHint::unknown);
    perf_trace::record_duration_if_at_least(
        "page_read",
        duration,
        Duration::from_millis(25),
        &[
            PerfField::Usize("page", index),
            PerfField::Usize("book_epoch", book_epoch),
            PerfField::Bool("success", success),
            PerfField::Bool("read_ahead", false),
            PerfField::Bool("decode_ahead", false),
            PerfField::Str("source_kind", hint.source_kind.as_str()),
            PerfField::Str("compression_method", hint.compression_method.as_str()),
            PerfField::Str("compression_state", hint.compression_state()),
            PerfField::Usize("compressed_size", size_hint_to_usize(hint.compressed_size)),
            PerfField::Usize(
                "uncompressed_size",
                size_hint_to_usize(hint.uncompressed_size),
            ),
            PerfField::Usize("compression_ratio_milli", hint.compression_ratio_milli()),
        ],
    );
}

#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
fn size_hint_to_usize(size: Option<u64>) -> usize {
    size.and_then(|size| usize::try_from(size).ok())
        .unwrap_or_default()
}

#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
fn record_source_bytes_cache(
    reason: &'static str,
    index: usize,
    book_epoch: usize,
    source_bytes: usize,
    cache_entries: usize,
    cache_bytes: usize,
    cache_budget_bytes: usize,
) {
    perf_trace::record_duration(
        "page_source_bytes_cache",
        Duration::ZERO,
        &[
            PerfField::Str("reason", reason),
            PerfField::Usize("page", index),
            PerfField::Usize("book_epoch", book_epoch),
            PerfField::Usize("source_bytes", source_bytes),
            PerfField::Usize("cache_entries", cache_entries),
            PerfField::Usize("cache_bytes", cache_bytes),
            PerfField::Usize("cache_budget_bytes", cache_budget_bytes),
        ],
    );
}

#[cfg(not(any(feature = "perf-dev", feature = "perf-diagnostics")))]
fn record_source_bytes_cache(
    _reason: &'static str,
    _index: usize,
    _book_epoch: usize,
    _source_bytes: usize,
    _cache_entries: usize,
    _cache_bytes: usize,
    _cache_budget_bytes: usize,
) {
}

#[cfg(test)]
mod tests {
    use super::{enabled_value, read_source_bytes, source_bytes_cache_budget, SourceBytesCache};
    use crate::core::source::{BookSource, SharedSource, SourceError};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[test]
    fn source_bytes_cache_reuses_inserted_page_bytes() {
        let mut cache = SourceBytesCache::new(32);
        let bytes = cache.insert("book", 3, 1, vec![1, 2, 3, 4]);

        assert_eq!(cache.get("book", 3, 1).as_deref(), Some(&[1, 2, 3, 4][..]));
        assert_eq!(cache.get("book", 4, 1), None);
        assert_eq!(bytes.as_ref(), &[1, 2, 3, 4]);
    }

    #[test]
    fn source_bytes_cache_prunes_to_budget() {
        let mut cache = SourceBytesCache::new(6);
        cache.insert("book", 1, 1, vec![1, 1, 1, 1]);
        cache.insert("book", 2, 1, vec![2, 2, 2, 2]);

        assert_eq!(cache.get("book", 1, 1), None);
        assert_eq!(cache.get("book", 2, 1).as_deref(), Some(&[2, 2, 2, 2][..]));
    }

    #[test]
    fn source_bytes_cache_skips_oversize_pages() {
        let mut cache = SourceBytesCache::new(3);
        let bytes = cache.insert("book", 1, 1, vec![1, 2, 3, 4]);

        assert_eq!(bytes.as_ref(), &[1, 2, 3, 4]);
        assert_eq!(cache.get("book", 1, 1), None);
    }

    #[test]
    fn read_source_bytes_uses_cache_before_source() {
        let source_impl = Arc::new(CountingSource {
            reads: AtomicUsize::new(0),
            path: PathBuf::from("counting-source"),
        });
        let source: SharedSource = source_impl.clone();
        let mut cache = SourceBytesCache::new(32);
        let mut read_ahead = None;

        let first = read_source_bytes(
            Some(&mut cache),
            &mut read_ahead,
            &source,
            "counting-source",
            1,
            2,
        )
        .unwrap();
        let second = read_source_bytes(
            Some(&mut cache),
            &mut read_ahead,
            &source,
            "counting-source",
            1,
            2,
        )
        .unwrap();

        assert_eq!(first.as_ref(), &[2, 3, 4]);
        assert_eq!(second.as_ref(), &[2, 3, 4]);
        assert_eq!(source_impl.reads.load(Ordering::Acquire), 1);
    }

    #[test]
    fn source_bytes_cache_env_gate_accepts_expected_enabled_values() {
        for value in ["1", "true", "yes", "on", "auto", " TRUE "] {
            assert!(enabled_value(value));
        }
        for value in ["0", "false", "off", "no", ""] {
            assert!(!enabled_value(value));
        }
    }

    #[test]
    fn source_bytes_cache_budget_requires_enabled_gate() {
        assert_eq!(source_bytes_cache_budget(None, Some(64)), None);
        assert_eq!(source_bytes_cache_budget(Some("0"), Some(64)), None);
        assert_eq!(
            source_bytes_cache_budget(Some("1"), Some(64)),
            Some(64 * 1024 * 1024)
        );
        assert_eq!(
            source_bytes_cache_budget(Some("1"), None),
            Some(64 * 1024 * 1024)
        );
    }

    struct CountingSource {
        reads: AtomicUsize,
        path: PathBuf,
    }

    impl BookSource for CountingSource {
        fn title(&self) -> &str {
            "counting"
        }

        fn source_path(&self) -> &Path {
            &self.path
        }

        fn book_id(&self) -> &str {
            "counting-source"
        }

        fn page_count(&self) -> usize {
            4
        }

        fn page_name(&self, index: usize) -> Option<&str> {
            match index {
                0 => Some("page-0000.png"),
                1 => Some("page-0001.png"),
                2 => Some("page-0002.png"),
                3 => Some("page-0003.png"),
                _ => None,
            }
        }

        fn read_page(&self, index: usize) -> Result<Vec<u8>, SourceError> {
            self.reads.fetch_add(1, Ordering::AcqRel);
            Ok(vec![index as u8, index as u8 + 1, index as u8 + 2])
        }
    }
}
