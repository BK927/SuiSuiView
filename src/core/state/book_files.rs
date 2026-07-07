use super::{BookRecord, StateStore};
use crate::core::perf_trace::{self, PerfField};
use directories::ProjectDirs;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

impl StateStore {
    pub(super) fn read_book_record(&self, book_id: &str) -> Option<BookRecord> {
        if let Some(pending) = &self.pending_book {
            if pending.book_id == book_id {
                return Some(pending.clone());
            }
        }
        let text = fs::read_to_string(book_file_path(&self.books_dir, book_id)).ok()?;
        serde_json::from_str::<BookRecord>(&text).ok()
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

    pub(super) fn write_book_record(&mut self, record: &BookRecord) -> std::io::Result<()> {
        if self
            .pending_book
            .as_ref()
            .is_some_and(|pending| pending.book_id == record.book_id)
        {
            self.pending_book = None;
        }
        let text = serde_json::to_string_pretty(record)?;
        write_atomic(&book_file_path(&self.books_dir, &record.book_id), &text)
    }

    pub(super) fn load_all_book_records(&self) -> Vec<BookRecord> {
        let mut records: Vec<BookRecord> = Vec::new();
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
                    records.push(record);
                }
            }
        }
        if let Some(pending) = &self.pending_book {
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

    pub(super) fn flush_pending_book(&mut self) {
        if let Some(record) = self.pending_book.take() {
            let _ = self.write_book_record(&record);
        }
    }

    // The single pending buffer only ever holds the current book; if a write for
    // a different book arrives, persist the buffered one first so it is not lost.
    pub(super) fn flush_pending_book_if_other(&mut self, book_id: &str) {
        if self
            .pending_book
            .as_ref()
            .is_some_and(|pending| pending.book_id != book_id)
        {
            self.flush_pending_book();
        }
    }

    // One-time import from the old monolithic state.json. During beta the resume
    // history is disposable, so keep only the manual page bookmarks and discard
    // the reading positions; books without bookmarks are dropped entirely.
    pub(super) fn import_legacy_bookmarks(&mut self) {
        if self.state.books.is_empty() {
            return;
        }
        let books = std::mem::take(&mut self.state.books);
        for record in books.into_values() {
            if record.page_bookmarks.is_empty() {
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
                path_positions: BTreeMap::new(),
                page_bookmarks: record.page_bookmarks,
                upscale_probe: record.upscale_probe,
                updated_at: record.updated_at,
            };
            let _ = self.write_book_record(&rescued);
        }
        let _ = self.write_state_file();
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
