use super::{now_unix_nanos, FitMode, ReadingDirection, StateStore};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageBookmarkChange {
    Added,
    Removed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageBookmarkPathRebase {
    Rebased(usize),
    NotNeeded,
    Ambiguous,
    Conflict,
}

pub(super) fn path_key(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

impl StateStore {
    pub fn recent_books(&self, limit: usize) -> Vec<BookRecord> {
        let mut records = self.load_all_book_records();
        // Automatic resume records continue to exist while recent locations
        // are disabled, but they are not recent-menu entries. Filter them
        // before applying the limit so pathless records cannot crowd out older
        // locations the user deliberately kept.
        records.retain(|record| {
            record
                .known_paths
                .last()
                .is_some_and(|path| !path.is_empty())
        });
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
    ) -> std::io::Result<()> {
        let now = now_unix_nanos();
        let title = title.into();
        let source_path = path_key(source_path);
        let result = self.mutate_book_record(book_id, move |record| {
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
            ((), true)
        })?;
        result.ok_or_else(missing_book_record_error)
    }

    pub fn toggle_page_bookmark(
        &mut self,
        book_id: &str,
        source_path: &Path,
        page: usize,
        title: impl Into<String>,
        page_name: Option<String>,
    ) -> std::io::Result<PageBookmarkChange> {
        let now = now_unix_nanos();
        let title = title.into();
        let source_path = path_key(source_path);
        let result =
            self.mutate_book_record(book_id, move |record| {
                if let Some(index) = record.page_bookmarks.iter().position(|bookmark| {
                    bookmark.source_path == source_path && bookmark.page == page
                }) {
                    record.page_bookmarks.remove(index);
                    record.updated_at = now;
                    return (PageBookmarkChange::Removed, true);
                }
                record.page_bookmarks.push(PageBookmark {
                    page,
                    source_path,
                    title,
                    page_name,
                    pinned: false,
                    created_at: now,
                    updated_at: now,
                });
                record.page_bookmarks.sort_by(page_bookmark_order);
                record.updated_at = now;
                (PageBookmarkChange::Added, true)
            })?;
        result.ok_or_else(missing_book_record_error)
    }

    /// Move the manual-bookmark scope from one vanished archive location to the
    /// newly opened location without touching automatic reading history.
    ///
    /// A live, distinct old path means the archive was copied, not moved. More
    /// than one vanished old path is ambiguous, and an already-bookmarked
    /// destination is a conflict; all three cases deliberately remain unchanged.
    /// Candidate discovery and mutation run in `mutate_book_record`, so they see
    /// the newest cross-process record under the books lock.
    pub fn rebase_moved_archive_page_bookmarks(
        &mut self,
        book_id: &str,
        new_path: &Path,
    ) -> std::io::Result<PageBookmarkPathRebase> {
        let metadata = fs::metadata(new_path)?;
        if !metadata.is_file() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "archive bookmark destination is not a file",
            ));
        }
        let new_canonical = fs::canonicalize(new_path)?;
        let new_path = path_key(new_path);
        let now = now_unix_nanos();
        let result = self.mutate_book_record(book_id, move |record| {
            let source_paths: BTreeSet<String> = record
                .page_bookmarks
                .iter()
                .map(|bookmark| bookmark.source_path.clone())
                .filter(|path| !path.is_empty() && path != &new_path)
                .collect();
            let mut candidates = Vec::new();
            for path in source_paths {
                match bookmark_path_rebase_candidate(&path, &new_canonical) {
                    Some(true) => candidates.push(path),
                    Some(false) => {}
                    None => return (PageBookmarkPathRebase::Ambiguous, false),
                }
            }

            let old_path = match candidates.as_slice() {
                [] => return (PageBookmarkPathRebase::NotNeeded, false),
                [old_path] => old_path,
                _ => return (PageBookmarkPathRebase::Ambiguous, false),
            };
            if record
                .page_bookmarks
                .iter()
                .any(|bookmark| bookmark.source_path == new_path)
            {
                return (PageBookmarkPathRebase::Conflict, false);
            }

            let mut rebased = 0;
            for bookmark in &mut record.page_bookmarks {
                if bookmark.source_path == *old_path {
                    bookmark.source_path.clone_from(&new_path);
                    bookmark.updated_at = now;
                    rebased += 1;
                }
            }
            debug_assert!(rebased > 0);
            record.page_bookmarks.sort_by(page_bookmark_order);
            record.updated_at = now;
            (PageBookmarkPathRebase::Rebased(rebased), true)
        })?;
        Ok(result.unwrap_or(PageBookmarkPathRebase::NotNeeded))
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
    ) -> std::io::Result<bool> {
        let source_path = path_key(source_path);
        let result = self.mutate_book_record(book_id, move |record| {
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
            if changed {
                record.page_bookmarks.sort_by(page_bookmark_order);
                record.updated_at = now_unix_nanos();
            }
            (changed, changed)
        })?;
        Ok(result.unwrap_or(false))
    }

    /// Attach the AUTO upscaler probe outcome to an existing book record. Mirrors the
    /// bookmark read-modify-write path so a buffered pending record stays consistent; a
    /// no-op if the record does not exist yet (callers persist it first) or is unchanged.
    pub fn set_book_upscale_probe(
        &mut self,
        book_id: &str,
        probe: UpscaleProbeRecord,
    ) -> std::io::Result<bool> {
        let result = self.mutate_book_record(book_id, move |record| {
            if record.upscale_probe.as_ref() == Some(&probe) {
                return (false, false);
            }
            record.upscale_probe = Some(probe);
            record.updated_at = now_unix_nanos();
            (true, true)
        })?;
        Ok(result.unwrap_or(false))
    }

    pub fn remove_page_bookmark(
        &mut self,
        book_id: &str,
        source_path: &Path,
        page: usize,
    ) -> std::io::Result<bool> {
        let source_path = path_key(source_path);
        let result = self.mutate_book_record(book_id, move |record| {
            let previous_len = record.page_bookmarks.len();
            record.page_bookmarks.retain(|page_bookmark| {
                page_bookmark.source_path != source_path || page_bookmark.page != page
            });
            let changed = record.page_bookmarks.len() != previous_len;
            if changed {
                record.updated_at = now_unix_nanos();
            }
            (changed, changed)
        })?;
        Ok(result.unwrap_or(false))
    }

    pub fn clear_page_bookmarks(
        &mut self,
        book_id: &str,
        source_path: &Path,
    ) -> std::io::Result<usize> {
        let source_path = path_key(source_path);
        let result = self.mutate_book_record(book_id, move |record| {
            let previous_len = record.page_bookmarks.len();
            record
                .page_bookmarks
                .retain(|page_bookmark| page_bookmark.source_path != source_path);
            let removed = previous_len - record.page_bookmarks.len();
            if removed > 0 {
                record.updated_at = now_unix_nanos();
            }
            (removed, removed > 0)
        })?;
        Ok(result.unwrap_or(0))
    }

    pub fn clear_all_page_bookmarks(&mut self) -> std::io::Result<usize> {
        self.flush_pending_books()?;
        let mut removed = 0;
        let now = now_unix_nanos();
        for record in self.load_all_book_records() {
            if let Some(count) = self.mutate_book_record(&record.book_id, |record| {
                let previous_len = record.page_bookmarks.len();
                record
                    .page_bookmarks
                    .retain(|page_bookmark| page_bookmark.source_path.is_empty());
                let count = previous_len - record.page_bookmarks.len();
                if count > 0 {
                    record.updated_at = now;
                }
                (count, count > 0)
            })? {
                removed += count;
            }
        }
        Ok(removed)
    }
}

fn missing_book_record_error() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "book record does not exist yet",
    )
}

/// `Some(true)` is the same file (for example Windows case aliases) or a
/// vanished path; `Some(false)` is a live, distinct copy. An uninspectable path
/// is `None` so callers conservatively leave every bookmark untouched.
fn bookmark_path_rebase_candidate(old_path: &str, new_canonical: &Path) -> Option<bool> {
    match fs::canonicalize(old_path) {
        Ok(old_canonical) => Some(old_canonical == new_canonical),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(true),
        Err(_) => None,
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

pub(super) fn page_bookmark_order(left: &PageBookmark, right: &PageBookmark) -> std::cmp::Ordering {
    right
        .pinned
        .cmp(&left.pinned)
        .then_with(|| right.updated_at.cmp(&left.updated_at))
        .then_with(|| left.page.cmp(&right.page))
}

#[cfg(test)]
mod tests;
