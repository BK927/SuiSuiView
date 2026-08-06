use super::{now_unix_seconds, FitMode, ReadingDirection, StateStore};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BookRecord {
    pub book_id: String,
    pub title: String,
    pub last_page: usize,
    #[serde(default)]
    pub last_page_name: Option<String>,
    pub total_pages: usize,
    pub known_paths: Vec<String>,
    pub reading_direction: ReadingDirection,
    pub fit_mode: FitMode,
    #[serde(default)]
    pub manual_zoom: Option<f32>,
    /// Opaque view-mode token (see `ViewMode::token` in the app layer). Core
    /// stores it verbatim; `None` on legacy records restores session behavior.
    #[serde(default)]
    pub view_mode: Option<String>,
    /// Anchor scroll offset for `view_mode == "vertical_strip"`; ignored in
    /// paged modes.
    #[serde(default)]
    pub strip_offset_frac: Option<f32>,
    /// Which index the Smart two-page pairing grid starts on (0 or 1); 0 on
    /// legacy records, which is the behavior they were saved with.
    #[serde(default)]
    pub smart_spread_phase: u8,
    #[serde(default)]
    pub path_positions: BTreeMap<String, ReadingPosition>,
    #[serde(default)]
    pub page_bookmarks: Vec<PageBookmark>,
    #[serde(default)]
    pub upscale_probe: Option<UpscaleProbeRecord>,
    pub updated_at: u64,
}

/// Persisted outcome of the per-book AUTO upscaler round-trip probe. `winner` stores a
/// [`WgpuUpscaleMethod::token`]; `version` guards against reusing a decision made by an
/// older probe algorithm.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpscaleProbeRecord {
    pub winner: String,
    pub ssim_anime4k: f32,
    pub ssim_fsr: f32,
    pub pages: u8,
    pub version: u32,
}

pub const UPSCALE_PROBE_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReadingPosition {
    pub last_page: usize,
    #[serde(default)]
    pub last_page_name: Option<String>,
    pub reading_direction: ReadingDirection,
    pub fit_mode: FitMode,
    #[serde(default)]
    pub manual_zoom: Option<f32>,
    #[serde(default)]
    pub view_mode: Option<String>,
    #[serde(default)]
    pub strip_offset_frac: Option<f32>,
    #[serde(default)]
    pub smart_spread_phase: u8,
    pub updated_at: u64,
}

impl ReadingPosition {
    pub(super) fn from_input(input: &BookRecordInput<'_>, now: u64) -> Self {
        Self {
            last_page: input.last_page.min(input.total_pages.saturating_sub(1)),
            last_page_name: input.last_page_name.map(ToOwned::to_owned),
            reading_direction: input.reading_direction,
            fit_mode: input.fit_mode,
            manual_zoom: input.manual_zoom,
            view_mode: input.view_mode.map(ToOwned::to_owned),
            strip_offset_frac: input.strip_offset_frac,
            smart_spread_phase: input.smart_spread_phase,
            updated_at: now,
        }
    }

    pub(super) fn from_record(record: &BookRecord) -> Self {
        Self {
            last_page: record.last_page,
            last_page_name: record.last_page_name.clone(),
            reading_direction: record.reading_direction,
            fit_mode: record.fit_mode,
            manual_zoom: record.manual_zoom,
            view_mode: record.view_mode.clone(),
            strip_offset_frac: record.strip_offset_frac,
            smart_spread_phase: record.smart_spread_phase,
            updated_at: record.updated_at,
        }
    }

    pub(super) fn matches_input(&self, input: &BookRecordInput<'_>) -> bool {
        self.last_page == input.last_page.min(input.total_pages.saturating_sub(1))
            && self.last_page_name.as_deref() == input.last_page_name
            && self.reading_direction == input.reading_direction
            && self.fit_mode == input.fit_mode
            && self.manual_zoom == input.manual_zoom
            && self.view_mode.as_deref() == input.view_mode
            && self.strip_offset_frac == input.strip_offset_frac
            && self.smart_spread_phase == input.smart_spread_phase
    }
}

pub struct BookRecordInput<'a> {
    pub book_id: &'a str,
    pub title: &'a str,
    pub last_page: usize,
    pub last_page_name: Option<&'a str>,
    pub total_pages: usize,
    pub path: &'a Path,
    pub reading_direction: ReadingDirection,
    pub fit_mode: FitMode,
    pub manual_zoom: Option<f32>,
    pub view_mode: Option<&'a str>,
    pub strip_offset_frac: Option<f32>,
    pub smart_spread_phase: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageBookmark {
    pub page: usize,
    #[serde(default)]
    pub source_path: String,
    pub title: String,
    #[serde(default)]
    pub page_name: Option<String>,
    pub pinned: bool,
    pub created_at: u64,
    pub updated_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageBookmarkEntry {
    pub book_id: String,
    pub book_title: String,
    pub known_path: Option<String>,
    pub bookmark: PageBookmark,
}

pub(super) fn path_key(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

impl StateStore {
    pub fn recent_books(&self, limit: usize) -> Vec<BookRecord> {
        let mut records = self.load_all_book_records();
        records.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
        records.truncate(limit);
        records
    }

    pub fn page_bookmarks(&self, book_id: &str) -> Vec<PageBookmark> {
        self.read_book_record(book_id)
            .map(|record| record.page_bookmarks)
            .unwrap_or_default()
    }

    pub fn all_page_bookmarks(&self) -> Vec<PageBookmarkEntry> {
        self.load_all_book_records()
            .iter()
            .flat_map(page_bookmark_entries_for_book)
            .collect()
    }

    pub fn page_bookmark_entries(
        &self,
        book_id: &str,
        source_path: &Path,
    ) -> Vec<PageBookmarkEntry> {
        self.read_book_record(book_id)
            .map(|record| page_bookmark_entries_for_path(&record, path_key(source_path).as_str()))
            .unwrap_or_default()
    }

    pub fn all_page_bookmark_count(&self) -> usize {
        self.load_all_book_records()
            .iter()
            .map(|record| {
                record
                    .page_bookmarks
                    .iter()
                    .filter(|bookmark| !bookmark.source_path.is_empty())
                    .count()
            })
            .sum()
    }

    pub fn has_page_bookmark(&self, book_id: &str, source_path: &Path, page: usize) -> bool {
        let source_path = path_key(source_path);
        self.read_book_record(book_id).is_some_and(|record| {
            record
                .page_bookmarks
                .iter()
                .any(|bookmark| bookmark.source_path == source_path && bookmark.page == page)
        })
    }

    pub fn upsert_page_bookmark(
        &mut self,
        book_id: &str,
        source_path: &Path,
        page: usize,
        title: impl Into<String>,
        page_name: Option<String>,
    ) {
        let now = now_unix_seconds();
        let title = title.into();
        let source_path = path_key(source_path);
        let Some(mut record) = self.read_book_record(book_id) else {
            return;
        };

        if let Some(existing) = record
            .page_bookmarks
            .iter_mut()
            .find(|bookmark| bookmark.source_path == source_path && bookmark.page == page)
        {
            existing.title = title;
            existing.page_name = page_name;
            existing.updated_at = now;
        } else {
            record.page_bookmarks.push(PageBookmark {
                page,
                source_path,
                title,
                page_name,
                pinned: false,
                created_at: now,
                updated_at: now,
            });
        }
        record.page_bookmarks.sort_by(page_bookmark_order);
        record.updated_at = now;
        let _ = self.write_book_record(&record);
    }

    /// Re-point this book's bookmarks at their pages after the page set changed
    /// underneath them (a folder snapshot refresh). `resolve` maps a remembered
    /// `page_name` to its index in the new snapshot; `None` drops the bookmark,
    /// because the file it marked is gone. Bookmarks for other source paths, and
    /// legacy bookmarks with no `page_name`, are left exactly as they are — there
    /// is nothing to re-resolve them by.
    ///
    /// Without this the current page is remapped by identity while the bookmarks
    /// keep pointing at stale indices: every bookmark past a deleted file lands
    /// one image off, and toggling one there deletes a bookmark the reader never
    /// made.
    pub fn remap_page_bookmarks(
        &mut self,
        book_id: &str,
        source_path: &Path,
        resolve: impl Fn(&str) -> Option<usize>,
    ) {
        let source_path = path_key(source_path);
        let Some(mut record) = self.read_book_record(book_id) else {
            return;
        };

        let mut changed = false;
        record.page_bookmarks.retain_mut(|bookmark| {
            if bookmark.source_path != source_path {
                return true;
            }
            let Some(page_name) = bookmark.page_name.as_deref() else {
                return true;
            };
            match resolve(page_name) {
                Some(page) => {
                    if bookmark.page != page {
                        bookmark.page = page;
                        changed = true;
                    }
                    true
                }
                None => {
                    changed = true;
                    false
                }
            }
        });
        if !changed {
            return;
        }
        record.page_bookmarks.sort_by(page_bookmark_order);
        record.updated_at = now_unix_seconds();
        let _ = self.write_book_record(&record);
    }

    /// Attach the AUTO upscaler probe outcome to an existing book record. Mirrors the
    /// bookmark read-modify-write path so a buffered pending record stays consistent; a
    /// no-op if the record does not exist yet (callers persist it first) or is unchanged.
    pub fn set_book_upscale_probe(&mut self, book_id: &str, probe: UpscaleProbeRecord) {
        let Some(mut record) = self.read_book_record(book_id) else {
            return;
        };
        if record.upscale_probe.as_ref() == Some(&probe) {
            return;
        }
        record.upscale_probe = Some(probe);
        record.updated_at = now_unix_seconds();
        let _ = self.write_book_record(&record);
    }

    pub fn remove_page_bookmark(&mut self, book_id: &str, source_path: &Path, page: usize) {
        let Some(mut record) = self.read_book_record(book_id) else {
            return;
        };
        let source_path = path_key(source_path);
        let previous_len = record.page_bookmarks.len();
        record.page_bookmarks.retain(|page_bookmark| {
            page_bookmark.source_path != source_path || page_bookmark.page != page
        });
        if record.page_bookmarks.len() == previous_len {
            return;
        }
        record.updated_at = now_unix_seconds();
        let _ = self.write_book_record(&record);
    }

    pub fn clear_page_bookmarks(&mut self, book_id: &str, source_path: &Path) -> usize {
        let Some(mut record) = self.read_book_record(book_id) else {
            return 0;
        };
        let source_path = path_key(source_path);
        let previous_len = record.page_bookmarks.len();
        record
            .page_bookmarks
            .retain(|page_bookmark| page_bookmark.source_path != source_path);
        let removed = previous_len - record.page_bookmarks.len();
        if removed == 0 {
            return 0;
        }
        record.updated_at = now_unix_seconds();
        let _ = self.write_book_record(&record);
        removed
    }

    pub fn clear_all_page_bookmarks(&mut self) -> usize {
        self.flush_pending_book();
        let mut removed = 0;
        let now = now_unix_seconds();
        for mut record in self.load_all_book_records() {
            let previous_len = record.page_bookmarks.len();
            record
                .page_bookmarks
                .retain(|page_bookmark| page_bookmark.source_path.is_empty());
            let count = previous_len - record.page_bookmarks.len();
            if count == 0 {
                continue;
            }
            removed += count;
            record.updated_at = now;
            let _ = self.write_book_record(&record);
        }
        removed
    }
}

fn page_bookmark_entries_for_book(book: &BookRecord) -> Vec<PageBookmarkEntry> {
    book.page_bookmarks
        .iter()
        .filter(|bookmark| !bookmark.source_path.is_empty())
        .map(|bookmark| PageBookmarkEntry {
            book_id: book.book_id.clone(),
            book_title: book.title.clone(),
            known_path: Some(bookmark.source_path.clone()),
            bookmark: bookmark.clone(),
        })
        .collect()
}

fn page_bookmark_entries_for_path(book: &BookRecord, source_path: &str) -> Vec<PageBookmarkEntry> {
    book.page_bookmarks
        .iter()
        .filter(|bookmark| bookmark.source_path == source_path)
        .map(|bookmark| PageBookmarkEntry {
            book_id: book.book_id.clone(),
            book_title: book.title.clone(),
            known_path: Some(bookmark.source_path.clone()),
            bookmark: bookmark.clone(),
        })
        .collect()
}

fn page_bookmark_order(left: &PageBookmark, right: &PageBookmark) -> std::cmp::Ordering {
    right
        .pinned
        .cmp(&left.pinned)
        .then_with(|| right.updated_at.cmp(&left.updated_at))
        .then_with(|| left.page.cmp(&right.page))
}

#[cfg(test)]
mod tests;
