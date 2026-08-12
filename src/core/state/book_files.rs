use super::{BookRecord, StateStore};
use crate::core::perf_trace::{self, PerfField};
use directories::ProjectDirs;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

impl StateStore {
    pub(super) fn read_book_record(&self, book_id: &str) -> Option<BookRecord> {
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

    pub(super) fn write_state_file(&mut self) -> std::io::Result<()> {
        let started = Instant::now();
        let result = (|| {
            let text = serde_json::to_string_pretty(&self.state)?;
            write_atomic(&self.path, &text)
        })();
        if result.is_ok() {
            self.state_dirty = false;
        }
        perf_trace::record_duration_if_at_least(
            "state_save",
            started.elapsed(),
            Duration::from_millis(20),
            &[PerfField::Bool("success", result.is_ok())],
        );
        result
    }

    pub(super) fn load_all_book_records(&self) -> Vec<BookRecord> {
        if !self.books.borrow().all_loaded {
            let mut books = self.books.borrow_mut();
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
                        books.records.insert(record.book_id.clone(), record);
                    }
                }
            }
            books.all_loaded = true;
        }
        let mut records: Vec<BookRecord> = self.books.borrow().records.values().cloned().collect();
        for pending in self.pending_books.values() {
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
        let mut state_changed = false;
        for book_id in book_ids {
            let Some(record) = self.state.books.get(&book_id).cloned() else {
                continue;
            };
            if record.page_bookmarks.is_empty() {
                self.state.books.remove(&book_id);
                state_changed = true;
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
                self.state.books.remove(&book_id);
                state_changed = true;
            }
        }
        if state_changed {
            self.state_dirty = true;
            let _ = self.write_state_file();
        }
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

fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
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
