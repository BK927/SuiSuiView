//! `StateStore` behavior: load, in-memory mutation, and the save/flush path.
//!
//! The struct and the persisted types live in the parent module; the two other
//! `impl StateStore` blocks are `book_files.rs` (file I/O) and `bookmarks.rs`.

use super::book_files;
use super::bookmarks::path_key;
use super::now_unix_nanos;
use super::{
    AppSettings, BookRecord, BookRecordInput, FastStartFailureNotice, PersistedState,
    ReadingPosition, StateStore, WindowPlacement,
};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

impl StateStore {
    pub fn load() -> Self {
        let path = book_files::state_file_path();
        let books_dir = book_files::books_dir_path();
        let mut state = fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<PersistedState>(&text).ok())
            .unwrap_or_default();
        state.settings.normalize_product_choices();
        if looks_like_maximized_rect_artifact(&state.window) {
            state.window = WindowPlacement::default();
        }

        let mut store = Self {
            path,
            books_dir,
            state,
            pending_books: BTreeMap::new(),
            state_dirty: false,
            books: Default::default(),
        };
        store.import_legacy_bookmarks();
        store
    }

    pub fn book_record(&self, book_id: &str) -> Option<BookRecord> {
        self.read_book_record(book_id)
    }

    /// Re-key an existing record onto `book_id` when this exact path is one it
    /// already knows. Returns whether anything was adopted.
    ///
    /// A book's identity is its content fingerprint, which survives moving and
    /// renaming — but not editing. Adding, removing, or re-saving a single image
    /// changes a folder's `book_id`, and the reading position and every bookmark
    /// become unreachable under an id nothing will ask for again. The app causes
    /// this itself: deleting one page from a folder book re-opens the same folder
    /// with a fresh fingerprint.
    ///
    /// The path is the second identity axis, so it covers exactly the case the
    /// fingerprint cannot. Adoption requires an exact path match, so the only way
    /// to inherit the wrong record is to replace a book with a different one at
    /// the same path — rarer, and milder, than losing the record outright.
    pub fn adopt_record_for_path(&mut self, book_id: &str, path: &Path) -> std::io::Result<bool> {
        if self.read_book_record(book_id).is_some() {
            return Ok(false);
        }
        let wanted = path_key(path);
        // `known_paths` follows the recent-locations setting, but `path_positions`
        // is always written for per-path resume, so the fallback keeps working
        // with the recent list turned off.
        let Some(record) = self
            .load_all_book_records()
            .into_iter()
            .filter(|record| {
                record.known_paths.iter().any(|known| known == &wanted)
                    || record.path_positions.contains_key(&wanted)
            })
            .max_by_key(|record| record.updated_at)
        else {
            return Ok(false);
        };

        let previous_book_id = record.book_id;
        if previous_book_id == book_id {
            return Ok(false);
        }
        self.rekey_book_record_for_path(&previous_book_id, book_id, &wanted)
    }

    pub fn reading_position(
        &self,
        book_id: &str,
        path: &Path,
        allow_identity_match: bool,
    ) -> Option<ReadingPosition> {
        let record = self.read_book_record(book_id)?;
        if allow_identity_match {
            return Some(ReadingPosition::from_record(&record));
        }
        record.path_positions.get(path_key(path).as_str()).cloned()
    }

    pub fn settings(&self) -> &AppSettings {
        &self.state.settings
    }

    pub fn fast_start_failure_notice(&self) -> Option<&FastStartFailureNotice> {
        self.state.fast_start_failure.as_ref()
    }

    pub fn update_settings(&mut self, mut settings: AppSettings) {
        settings.normalize_product_choices();
        self.state.settings = settings;
        self.state.version = 4;
        let _ = self.save();
    }

    pub fn record_fast_start_failure(&mut self, notice: FastStartFailureNotice) {
        self.state.fast_start_failure = Some(notice);
        self.state.version = 4;
        let _ = self.save();
    }

    pub fn mark_fast_start_failure_notice_shown(&mut self) {
        let Some(notice) = self.state.fast_start_failure.as_mut() else {
            return;
        };
        if notice.shown {
            return;
        }
        notice.shown = true;
        self.state.version = 4;
        let _ = self.save();
    }

    pub fn clear_fast_start_failure_notice(&mut self) {
        if self.state.fast_start_failure.take().is_some() {
            self.state.version = 4;
            let _ = self.save();
        }
    }

    pub fn window_placement(&self) -> &WindowPlacement {
        &self.state.window
    }

    pub fn update_window_placement_deferred(&mut self, placement: WindowPlacement) -> bool {
        if self.state.window == placement {
            return false;
        }
        self.state.window = placement;
        self.state.version = 4;
        self.state_dirty = true;
        true
    }

    pub fn upsert_book_record(&mut self, input: BookRecordInput<'_>) -> std::io::Result<()> {
        let (record, _changed) = self.compute_record_update(input, true);
        let book_id = record.book_id.clone();
        self.pending_books.insert(book_id.clone(), record);
        self.flush_pending_book(&book_id)
    }

    pub fn upsert_book_record_deferred(&mut self, input: BookRecordInput<'_>) -> bool {
        let (record, changed) = self.compute_record_update(input, false);
        if changed {
            self.pending_books.insert(record.book_id.clone(), record);
        }
        changed
    }

    pub fn clear_archive_page_names(&mut self) -> std::io::Result<usize> {
        self.flush_pending_books()?;
        let mut cleared = 0;
        for record in self.load_all_book_records() {
            if !looks_like_archive_book(&record) {
                continue;
            }
            if let Some(count) = self.mutate_book_record(&record.book_id, |record| {
                let mut count = usize::from(record.last_page_name.take().is_some());
                for position in record.path_positions.values_mut() {
                    count += usize::from(position.last_page_name.take().is_some());
                }
                if count > 0 {
                    record.updated_at = now_unix_nanos();
                }
                (count, count > 0)
            })? {
                cleared += count;
            }
        }
        Ok(cleared)
    }

    fn compute_record_update(&self, input: BookRecordInput<'_>, touch: bool) -> (BookRecord, bool) {
        let path_text = input.path.to_string_lossy().to_string();
        let now = now_unix_nanos();
        let existing = self.read_book_record(input.book_id);
        let is_new = existing.is_none();
        let mut record = existing.unwrap_or_else(|| BookRecord {
            book_id: input.book_id.to_owned(),
            title: input.title.to_owned(),
            last_page: 0,
            last_page_name: None,
            total_pages: input.total_pages,
            known_paths: Vec::new(),
            reading_direction: input.reading_direction,
            fit_mode: input.fit_mode,
            manual_zoom: None,
            view_mode: None,
            strip_offset_frac: None,
            smart_spread_phase: 0,
            path_positions: BTreeMap::new(),
            page_bookmarks: Vec::new(),
            upscale_probe: None,
            updated_at: now,
        });

        let title = input.title.to_owned();
        let last_page = input.last_page.min(input.total_pages.saturating_sub(1));
        let last_page_name = input.last_page_name.map(ToOwned::to_owned);
        let path_position_changed = record
            .path_positions
            .get(path_text.as_str())
            .is_none_or(|position| !position.matches_input(&input));
        let mut changed = is_new
            || record.title != title
            || record.last_page != last_page
            || record.last_page_name != last_page_name
            || record.total_pages != input.total_pages
            || record.reading_direction != input.reading_direction
            || record.fit_mode != input.fit_mode
            || record.manual_zoom != input.manual_zoom
            || record.view_mode.as_deref() != input.view_mode
            || record.strip_offset_frac != input.strip_offset_frac
            || record.smart_spread_phase != input.smart_spread_phase
            || path_position_changed;

        record.title = title;
        record.last_page = last_page;
        record.last_page_name = last_page_name;
        record.total_pages = input.total_pages;
        record.reading_direction = input.reading_direction;
        record.fit_mode = input.fit_mode;
        record.manual_zoom = input.manual_zoom;
        record.view_mode = input.view_mode.map(ToOwned::to_owned);
        record.strip_offset_frac = input.strip_offset_frac;
        record.smart_spread_phase = input.smart_spread_phase;
        if path_position_changed || touch {
            record
                .path_positions
                .insert(path_text.clone(), ReadingPosition::from_input(&input, now));
        }

        // "Remember recent locations" used to hide the list while still writing
        // every path to disk, so unchecking it stopped nothing and re-checking it
        // brought the whole history back. Honour it at the point of recording.
        if self.state.settings.remember_recent_locations
            && !record.known_paths.iter().any(|known| known == &path_text)
        {
            record.known_paths.push(path_text);
            changed = true;
        }
        if record.known_paths.len() > 8 {
            let extra = record.known_paths.len() - 8;
            record.known_paths.drain(0..extra);
            changed = true;
        }

        if changed || touch {
            record.updated_at = now;
        }
        (record, changed || touch)
    }

    pub fn save(&mut self) -> std::io::Result<()> {
        self.write_state_file()
    }

    pub fn flush(&mut self) -> std::io::Result<()> {
        let mut result = self.flush_pending_books();
        if self.state_dirty {
            let state_result = self.write_state_file();
            if result.is_ok() {
                result = state_result;
            }
        }
        result
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// One-time heal for window placement corrupted by the pre-fix maximize bug,
/// where the *maximized* rect (the monitor work area plus the invisible resize
/// borders) was persisted as the normal geometry with `maximized == false`. That
/// artifact is unmistakable: a non-maximized placement whose physical origin is
/// negative on both axes (the resize-border overhang a maximized window sits at)
/// paired with an implausibly tall restore height. A genuine normal window can
/// never produce that combination, so reset it to the default rather than
/// restoring the window off-screen at monitor size. The real fix (native
/// GetWindowPlacement + visibility gating) prevents new corruption; this only
/// cleans the existing one. Intentionally narrow — it targets the observed
/// corruption, not a general "too big" heuristic.
fn looks_like_maximized_rect_artifact(placement: &WindowPlacement) -> bool {
    if placement.maximized {
        return false;
    }
    // `normal_rect_px` did not exist before the fix that made placement a pure
    // GetWindowPlacement round-trip, so its presence proves this state was written
    // by a build that can no longer produce the corruption. Without this the
    // heuristic is not one-time at all: a tall window on a monitor arranged left
    // of and above the primary has a legitimately negative origin on both axes,
    // and gets reset to the default size on every single launch.
    if placement.normal_rect_px.is_some() {
        return false;
    }
    let Some([x, y]) = placement.outer_position_px else {
        return false;
    };
    let Some([_, height]) = placement.inner_size else {
        return false;
    };
    x < 0 && y < 0 && height > 1800.0
}

fn looks_like_archive_book(record: &BookRecord) -> bool {
    record.known_paths.iter().any(|path| {
        Path::new(path)
            .extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| matches!(extension.to_ascii_lowercase().as_str(), "zip" | "cbz"))
            .unwrap_or(false)
    })
}

#[cfg(test)]
mod window_heal_tests {
    use super::{looks_like_maximized_rect_artifact, WindowPlacement};

    #[test]
    fn heals_saved_maximized_rect_artifact() {
        // Exact corrupt values observed in the field: the maximized rect of a
        // left portrait monitor saved as normal geometry with maximized=false.
        let placement = WindowPlacement {
            inner_size: Some([1152.0, 1977.0]),
            outer_position: Some([-1159.0, -7.0]),
            outer_position_px: Some([-1449, -9]),
            normal_rect_px: None,
            maximized: false,
        };
        assert!(looks_like_maximized_rect_artifact(&placement));
    }

    #[test]
    fn keeps_sane_placement() {
        let placement = WindowPlacement {
            inner_size: Some([1280.0, 820.0]),
            outer_position: Some([100.0, 100.0]),
            outer_position_px: Some([100, 100]),
            normal_rect_px: None,
            maximized: false,
        };
        assert!(!looks_like_maximized_rect_artifact(&placement));
    }

    #[test]
    fn keeps_a_tall_left_and_above_placement_written_by_a_fixed_build() {
        // Same shape as the artifact, but carrying `normal_rect_px` — only a build
        // past the placement fix writes that, so this is a real window the user
        // put on a portrait monitor arranged left of and above the primary.
        let placement = WindowPlacement {
            inner_size: Some([1152.0, 1900.0]),
            outer_position: Some([-1159.0, -7.0]),
            outer_position_px: Some([-1449, -9]),
            normal_rect_px: Some([-1449, -9, -297, 1891]),
            maximized: false,
        };
        assert!(!looks_like_maximized_rect_artifact(&placement));
    }

    #[test]
    fn ignores_genuinely_maximized_placement() {
        // A real maximized window carries maximized=true and must not be reset.
        let placement = WindowPlacement {
            inner_size: Some([1152.0, 1977.0]),
            outer_position: Some([-1159.0, -7.0]),
            outer_position_px: Some([-1449, -9]),
            normal_rect_px: None,
            maximized: true,
        };
        assert!(!looks_like_maximized_rect_artifact(&placement));
    }
}
