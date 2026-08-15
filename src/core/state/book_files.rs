use super::{BookRecord, StateStore};
#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
use crate::core::perf_trace::{self, PerfField};
use directories::ProjectDirs;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
use std::time::Instant;

impl StateStore {
    pub(super) fn read_book_record(&self, book_id: &str) -> Option<BookRecord> {
        if book_redirect_exists(&self.books_dir, book_id) {
            self.books.borrow_mut().records.remove(book_id);
            return None;
        }
        if let Some(pending) = self.pending_books.get(book_id) {
            return Some(pending.clone());
        }
        if let Some(record) = self.books.borrow().records.get(book_id) {
            return Some(record.clone());
        }
        // A completed scan means every record is already here, so a miss is a
        // book that does not exist rather than one not read yet.
        if self.books.borrow().all_loaded {
            return None;
        }
        let text = fs::read_to_string(book_file_path(&self.books_dir, book_id)).ok()?;
        let record = serde_json::from_str::<BookRecord>(&text).ok()?;
        self.books
            .borrow_mut()
            .records
            .insert(record.book_id.clone(), record.clone());
        Some(record)
    }

    /// Parse every book record on disk once, then leave `records` complete so
    /// later whole-library questions stay in memory.
    fn ensure_all_book_records_loaded(&self) {
        if !self.books.borrow().all_loaded {
            #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
            let started = Instant::now();
            let mut books = self.books.borrow_mut();
            #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
            let mut scanned = 0usize;
            if let Ok(entries) = fs::read_dir(&self.books_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                        continue;
                    }
                    let Ok(text) = fs::read_to_string(&path) else {
                        continue;
                    };
                    if let Ok(record) = serde_json::from_str::<BookRecord>(&text) {
                        if book_redirect_exists(&self.books_dir, &record.book_id) {
                            continue;
                        }
                        books.records.insert(record.book_id.clone(), record);
                        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
                        {
                            scanned += 1;
                        }
                    }
                }
            }
            books.all_loaded = true;
            // Once per run, but it is the whole library off disk and it blocks
            // whichever frame first asks a question about every book.
            #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
            perf_trace::record_duration(
                "book_records_scan",
                started.elapsed(),
                &[PerfField::Usize("records", scanned)],
            );
        }
    }

    /// Visit every current book record in place, in unspecified order.
    ///
    /// [`Self::load_all_book_records`] deep-copies the whole library, which is
    /// the wrong primitive for a question that only needs to look at it. Use
    /// this for aggregates.
    pub(super) fn for_each_book_record(&self, mut visit: impl FnMut(&BookRecord)) {
        self.ensure_all_book_records_loaded();
        let books = self.books.borrow();
        for record in books.records.values() {
            // A pending edit supersedes the parsed copy; it is visited below.
            if self.pending_books.contains_key(&record.book_id) {
                continue;
            }
            if book_redirect_exists(&self.books_dir, &record.book_id) {
                continue;
            }
            visit(record);
        }
        for pending in self.pending_books.values() {
            if book_redirect_exists(&self.books_dir, &pending.book_id) {
                continue;
            }
            visit(pending);
        }
    }

    pub(super) fn load_all_book_records(&self) -> Vec<BookRecord> {
        self.ensure_all_book_records_loaded();
        let mut records: Vec<BookRecord> = self
            .books
            .borrow()
            .records
            .values()
            .filter(|record| !book_redirect_exists(&self.books_dir, &record.book_id))
            .cloned()
            .collect();
        for pending in self.pending_books.values() {
            if book_redirect_exists(&self.books_dir, &pending.book_id) {
                continue;
            }
            match records
                .iter_mut()
                .find(|record| record.book_id == pending.book_id)
            {
                Some(slot) => *slot = pending.clone(),
                None => records.push(pending.clone()),
            }
        }
        records
    }

    pub(super) fn flush_pending_books(&mut self) -> std::io::Result<()> {
        let mut first_error = None;
        let pending: Vec<_> = self.pending_books.values().cloned().collect();
        for record in pending {
            match self.persist_reading_record(&record) {
                Ok(()) => {
                    self.pending_books.remove(&record.book_id);
                }
                Err(error) if first_error.is_none() => first_error = Some(error),
                Err(_) => {}
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    pub(super) fn flush_pending_book(&mut self, book_id: &str) -> std::io::Result<()> {
        let Some(record) = self.pending_books.get(book_id).cloned() else {
            return Ok(());
        };
        self.persist_reading_record(&record)?;
        self.pending_books.remove(book_id);
        Ok(())
    }

    // One-time import from the old monolithic state.json. During beta the resume
    // history is disposable, so keep only the manual page bookmarks and discard
    // the reading positions; books without bookmarks are dropped entirely.
    pub(super) fn import_legacy_bookmarks(&mut self) {
        if self.state.books.is_empty() {
            return;
        }
        let book_ids: Vec<_> = self.state.books.keys().cloned().collect();
        let mut removable = Vec::new();
        for book_id in book_ids {
            let Some(record) = self.state.books.get(&book_id).cloned() else {
                continue;
            };
            if record.page_bookmarks.is_empty() {
                removable.push(book_id);
                continue;
            }
            let rescued = BookRecord {
                book_id: record.book_id,
                title: record.title,
                last_page: 0,
                last_page_name: None,
                total_pages: record.total_pages,
                known_paths: record.known_paths,
                reading_direction: record.reading_direction,
                fit_mode: record.fit_mode,
                manual_zoom: None,
                view_mode: None,
                strip_offset_frac: None,
                smart_spread_phase: 0,
                path_positions: BTreeMap::new(),
                page_bookmarks: record.page_bookmarks,
                upscale_probe: record.upscale_probe,
                updated_at: record.updated_at,
            };
            if self.merge_legacy_bookmark_record(&rescued).is_ok() {
                removable.push(book_id);
            }
        }
        // The split records are already durable. If cleanup fails, retain the
        // monolithic copies in memory and on disk so a later launch can retry.
        let _ = self.remove_legacy_book_records(&removable);
    }
}

pub(super) fn state_file_path() -> PathBuf {
    ProjectDirs::from("", "", "SuiSuiView")
        .map(|dirs| dirs.data_dir().join("state.json"))
        .unwrap_or_else(|| PathBuf::from("SuiSuiView-state.json"))
}

pub(super) fn books_dir_path() -> PathBuf {
    ProjectDirs::from("", "", "SuiSuiView")
        .map(|dirs| dirs.data_dir().join("books"))
        .unwrap_or_else(|| PathBuf::from("SuiSuiView-books"))
}

fn book_file_path(books_dir: &Path, book_id: &str) -> PathBuf {
    books_dir.join(format!("{}.json", sanitize_book_id(book_id)))
}

fn book_redirect_exists(books_dir: &Path, book_id: &str) -> bool {
    book_file_path(books_dir, book_id)
        .with_extension("redirect")
        .is_file()
}

// book_id is always "<kind>:<hex>"; ':' is invalid in Windows file names, so map
// any non-portable character to '_'. The kind prefix keeps ids collision-free.
fn sanitize_book_id(book_id: &str) -> String {
    book_id
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
                character
            } else {
                '_'
            }
        })
        .collect()
}

pub(super) fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("state");
    let tmp = path.with_file_name(format!("{file_name}.{}.tmp", std::process::id()));
    fs::write(&tmp, text)?;
    fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::state::{FitMode, PageBookmark, PersistedState, ReadingDirection};
    use std::hint::black_box;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{Instant, SystemTime, UNIX_EPOCH};

    const SYNTHETIC_RECORDS: usize = 1_000;
    const BENCHMARK_SAMPLES: usize = 21;

    #[test]
    #[ignore = "release-only synthetic performance probe; run explicitly with --ignored"]
    fn synthetic_book_record_catalog_benchmark() {
        let base = unique_benchmark_base();
        let books_dir = base.join("books");
        fs::create_dir_all(&books_dir).expect("create synthetic books directory");

        for index in 0..SYNTHETIC_RECORDS {
            let record = synthetic_record(index);
            let text = serde_json::to_string(&record).expect("serialize synthetic book record");
            fs::write(book_file_path(&books_dir, &record.book_id), text)
                .expect("write synthetic book record");
        }

        let mut samples_us = Vec::with_capacity(BENCHMARK_SAMPLES);
        for _ in 0..BENCHMARK_SAMPLES {
            let store = benchmark_store(&base);
            let started = Instant::now();
            let records = black_box(store.load_all_book_records());
            samples_us.push(started.elapsed().as_micros());
            assert_eq!(records.len(), SYNTHETIC_RECORDS);
        }
        samples_us.sort_unstable();

        let median_us = samples_us[samples_us.len() / 2];
        let p95_index = (samples_us.len() * 95 / 100).min(samples_us.len() - 1);
        println!(
            "{}",
            serde_json::json!({
                "benchmark": "synthetic_book_record_catalog",
                "records": SYNTHETIC_RECORDS,
                "samples": BENCHMARK_SAMPLES,
                "median_us": median_us,
                "p95_us": samples_us[p95_index],
                "max_us": samples_us[samples_us.len() - 1],
            })
        );

        let _ = fs::remove_dir_all(base);
    }

    /// The catalog benchmark above measures the one-time scan. This measures the
    /// shape the bookmark popover actually produces: the same whole-library
    /// question asked again on an already-warm store, once per frame for as long
    /// as the popover is open.
    #[test]
    #[ignore = "release-only synthetic performance probe; run explicitly with --ignored"]
    fn synthetic_warm_bookmark_count_benchmark() {
        let base = unique_benchmark_base();
        let books_dir = base.join("books");
        fs::create_dir_all(&books_dir).expect("create synthetic books directory");

        for index in 0..SYNTHETIC_RECORDS {
            let mut record = synthetic_record(index);
            record.page_bookmarks = vec![synthetic_bookmark(index)];
            let text = serde_json::to_string(&record).expect("serialize synthetic book record");
            fs::write(book_file_path(&books_dir, &record.book_id), text)
                .expect("write synthetic book record");
        }

        let store = benchmark_store(&base);
        // The first frame pays the scan; every later frame is what we measure.
        assert_eq!(store.all_page_bookmark_count(), SYNTHETIC_RECORDS);

        let mut samples_us = Vec::with_capacity(BENCHMARK_SAMPLES);
        for _ in 0..BENCHMARK_SAMPLES {
            let started = Instant::now();
            let count = black_box(store.all_page_bookmark_count());
            samples_us.push(started.elapsed().as_micros());
            assert_eq!(count, SYNTHETIC_RECORDS);
        }
        samples_us.sort_unstable();

        let p95_index = (samples_us.len() * 95 / 100).min(samples_us.len() - 1);
        println!(
            "{}",
            serde_json::json!({
                "benchmark": "synthetic_warm_bookmark_count",
                "records": SYNTHETIC_RECORDS,
                "samples": BENCHMARK_SAMPLES,
                "median_us": samples_us[samples_us.len() / 2],
                "p95_us": samples_us[p95_index],
                "max_us": samples_us[samples_us.len() - 1],
            })
        );

        let _ = fs::remove_dir_all(base);
    }

    fn synthetic_bookmark(index: usize) -> PageBookmark {
        PageBookmark {
            page: index % 200,
            source_path: format!("C:/synthetic/books/{index:08x}.cbz"),
            title: format!("Synthetic bookmark {index}"),
            page_name: Some(format!("chapter/page-{index:04}.jpg")),
            pinned: false,
            created_at: index as u64,
            updated_at: index as u64,
        }
    }

    fn benchmark_store(base: &Path) -> StateStore {
        StateStore {
            path: base.join("state.json"),
            books_dir: base.join("books"),
            state: PersistedState::default(),
            pending_books: Default::default(),
            state_dirty: false,
            books: Default::default(),
        }
    }

    fn synthetic_record(index: usize) -> BookRecord {
        BookRecord {
            book_id: format!("synthetic:{index:08x}"),
            title: format!("Synthetic Book {index}"),
            last_page: index % 200,
            last_page_name: None,
            total_pages: 200,
            known_paths: vec![format!("C:/synthetic/books/{index:08x}.cbz")],
            reading_direction: ReadingDirection::RightToLeft,
            fit_mode: FitMode::FitPage,
            manual_zoom: None,
            view_mode: None,
            strip_offset_frac: None,
            smart_spread_phase: 0,
            path_positions: BTreeMap::new(),
            page_bookmarks: Vec::new(),
            upscale_probe: None,
            updated_at: index as u64,
        }
    }

    fn unique_benchmark_base() -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock after epoch")
            .as_nanos();
        let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join("suisuiview-tests").join(format!(
            "book-catalog-bench-{stamp}-{}-{sequence}",
            std::process::id()
        ))
    }
}
