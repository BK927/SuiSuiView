//! Cross-process transactions for the split book-record files.
//!
//! A `BookRecord` contains several independently owned domains: automatic
//! reading position, manual page bookmarks, and the upscaler probe. Writers
//! must therefore re-read the latest file while holding the sidecar lock and
//! change only the domain they own. Serializing a cached whole record would
//! silently erase another app instance's newer fields.

use super::book_files;
use super::bookmarks::page_bookmark_order;
use super::{BookRecord, PersistedState, ReadingPosition, StateStore};
use crate::core::perf_trace::{self, PerfField};
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

const LOCK_WAIT_LIMIT: Duration = Duration::from_millis(25);
const LOCK_RETRY_DELAY: Duration = Duration::from_millis(1);

struct ExclusiveFileLock(File);

impl ExclusiveFileLock {
    fn acquire(path: &Path) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        let started = Instant::now();
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(Self(file)),
                Err(TryLockError::WouldBlock) if started.elapsed() < LOCK_WAIT_LIMIT => {
                    thread::sleep(LOCK_RETRY_DELAY);
                }
                Err(TryLockError::WouldBlock) => {
                    return Err(io::Error::new(
                        io::ErrorKind::WouldBlock,
                        "state file is busy in another app instance",
                    ));
                }
                Err(TryLockError::Error(error)) => return Err(error),
            }
        }
    }
}

impl Drop for ExclusiveFileLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

impl StateStore {
    pub(super) fn mutate_state_file<R>(
        &self,
        mutate: impl FnOnce(&mut PersistedState) -> (R, bool),
    ) -> io::Result<R> {
        let started = Instant::now();
        let result = (|| {
            let _lock = ExclusiveFileLock::acquire(&state_lock_path(&self.path))?;
            let mut latest = read_state_fresh(&self.path)?.unwrap_or_default();
            latest.settings.normalize_product_choices();
            let (value, changed) = mutate(&mut latest);
            if changed {
                latest.version = 4;
                let text = serde_json::to_string_pretty(&latest)?;
                book_files::write_atomic(&self.path, &text)?;
            }
            Ok(value)
        })();
        perf_trace::record_duration_if_at_least(
            "state_save",
            started.elapsed(),
            Duration::from_millis(20),
            &[PerfField::Bool("success", result.is_ok())],
        );
        result
    }

    pub(super) fn remove_legacy_book_records(&mut self, book_ids: &[String]) -> io::Result<()> {
        if book_ids.is_empty() {
            return Ok(());
        }
        self.mutate_state_file(|state| {
            for book_id in book_ids {
                state.books.remove(book_id);
            }
            ((), true)
        })?;
        for book_id in book_ids {
            self.state.books.remove(book_id);
        }
        Ok(())
    }

    /// Persist an automatic-reading-position snapshot without replacing manual
    /// bookmarks or the upscaler probe written by another app instance.
    pub(super) fn persist_reading_record(&mut self, update: &BookRecord) -> io::Result<()> {
        let _lock = ExclusiveFileLock::acquire(&books_lock_path(&self.books_dir))?;
        let path = book_record_path(&self.books_dir, &update.book_id);
        let latest = read_book_record_fresh(&path, &update.book_id)?;
        let merged = merge_reading_record(latest, update);
        write_book_record_atomic(&path, &merged)?;
        self.books
            .borrow_mut()
            .records
            .insert(merged.book_id.clone(), merged);
        Ok(())
    }

    /// Mutate a non-reading-position domain against the newest on-disk record.
    /// If an automatic position is buffered for this book, fold it in first so
    /// the one transaction can safely persist both independent updates.
    pub(super) fn mutate_book_record<R>(
        &mut self,
        book_id: &str,
        mutate: impl FnOnce(&mut BookRecord) -> (R, bool),
    ) -> io::Result<Option<R>> {
        let _lock = ExclusiveFileLock::acquire(&books_lock_path(&self.books_dir))?;
        let path = book_record_path(&self.books_dir, book_id);
        let latest = read_book_record_fresh(&path, book_id)?;
        let pending = self.pending_books.get(book_id).cloned();
        let pending_was_merged = pending.is_some();
        let record = match pending {
            Some(pending) => Some(merge_reading_record(latest, &pending)),
            None => latest,
        };
        let Some(mut record) = record else {
            return Ok(None);
        };

        let (result, changed) = mutate(&mut record);
        if changed || pending_was_merged {
            write_book_record_atomic(&path, &record)?;
        }
        self.books
            .borrow_mut()
            .records
            .insert(record.book_id.clone(), record);
        if pending_was_merged {
            self.pending_books.remove(book_id);
        }
        Ok(Some(result))
    }

    /// Import the manual-bookmark domain from the legacy monolithic state
    /// without replacing a newer split record's automatic reading position.
    /// Re-running this after a failed `state.json` cleanup is idempotent.
    pub(super) fn merge_legacy_bookmark_record(&mut self, legacy: &BookRecord) -> io::Result<()> {
        let _lock = ExclusiveFileLock::acquire(&books_lock_path(&self.books_dir))?;
        let path = book_record_path(&self.books_dir, &legacy.book_id);
        let latest = read_book_record_fresh(&path, &legacy.book_id)?;
        let (merged, changed) = merge_legacy_bookmark_record(latest, legacy);
        if changed {
            write_book_record_atomic(&path, &merged)?;
        }
        self.books
            .borrow_mut()
            .records
            .insert(merged.book_id.clone(), merged);
        Ok(())
    }

    /// Re-key an edited folder record without opening a gap between reading the
    /// old record and deleting it. The destination must still be absent after
    /// the global books lock is acquired.
    pub(super) fn rekey_book_record_for_path(
        &mut self,
        old_book_id: &str,
        new_book_id: &str,
        expected_path: &str,
    ) -> io::Result<bool> {
        let _lock = ExclusiveFileLock::acquire(&books_lock_path(&self.books_dir))?;
        let old_path = book_record_path(&self.books_dir, old_book_id);
        let new_path = book_record_path(&self.books_dir, new_book_id);
        let Some(mut source) = read_book_record_fresh(&old_path, old_book_id)? else {
            return Ok(false);
        };
        if let Some(pending) = self.pending_books.get(old_book_id) {
            source = merge_reading_record(Some(source), pending);
        }
        if !source.known_paths.iter().any(|path| path == expected_path)
            && !source.path_positions.contains_key(expected_path)
        {
            return Ok(false);
        }
        if read_book_record_fresh(&new_path, new_book_id)?.is_some() {
            return Ok(false);
        }

        source.book_id = new_book_id.to_owned();
        write_book_record_atomic(&new_path, &source)?;

        // The destination write is the commit point. From here on the app must
        // resolve the new identity even if antivirus software or a transient
        // handle prevents the obsolete source file from being removed.
        let mut books = self.books.borrow_mut();
        books.records.remove(old_book_id);
        books.records.insert(new_book_id.to_owned(), source);
        drop(books);
        self.pending_books.remove(old_book_id);
        if fs::remove_file(&old_path).is_err() {
            let quarantine =
                old_path.with_extension(format!("stale-rekey-{}.tmp", std::process::id()));
            if fs::rename(&old_path, &quarantine).is_ok() {
                let _ = fs::remove_file(quarantine);
            }
        }
        Ok(true)
    }
}

fn merge_legacy_bookmark_record(
    latest: Option<BookRecord>,
    legacy: &BookRecord,
) -> (BookRecord, bool) {
    let Some(mut merged) = latest else {
        return (legacy.clone(), true);
    };
    let mut changed = false;
    for legacy_path in &legacy.known_paths {
        if !merged.known_paths.contains(legacy_path) {
            merged.known_paths.push(legacy_path.clone());
            changed = true;
        }
    }
    for legacy_bookmark in &legacy.page_bookmarks {
        match merged.page_bookmarks.iter_mut().find(|bookmark| {
            bookmark.source_path == legacy_bookmark.source_path
                && bookmark.page == legacy_bookmark.page
        }) {
            Some(bookmark) if legacy_bookmark.updated_at > bookmark.updated_at => {
                *bookmark = legacy_bookmark.clone();
                changed = true;
            }
            Some(_) => {}
            None => {
                merged.page_bookmarks.push(legacy_bookmark.clone());
                changed = true;
            }
        }
    }
    if merged.upscale_probe.is_none() && legacy.upscale_probe.is_some() {
        merged.upscale_probe.clone_from(&legacy.upscale_probe);
        changed = true;
    }
    if changed {
        merged.page_bookmarks.sort_by(page_bookmark_order);
        merged.updated_at = merged.updated_at.max(legacy.updated_at);
    }
    (merged, changed)
}

fn merge_reading_record(latest: Option<BookRecord>, update: &BookRecord) -> BookRecord {
    let Some(mut merged) = latest else {
        return update.clone();
    };

    let latest_position_time = newest_position(&merged).map(|position| position.updated_at);
    let update_position_time = newest_position(update).map(|position| position.updated_at);

    merged.title = update.title.clone();
    merged.total_pages = update.total_pages;
    for path in &update.known_paths {
        if !merged.known_paths.contains(path) {
            merged.known_paths.push(path.clone());
        }
    }
    if merged.known_paths.len() > 8 {
        let extra = merged.known_paths.len() - 8;
        merged.known_paths.drain(0..extra);
    }
    for (path, position) in &update.path_positions {
        let replace = merged
            .path_positions
            .get(path)
            .is_none_or(|current| position.updated_at >= current.updated_at);
        if replace {
            merged.path_positions.insert(path.clone(), position.clone());
        }
    }

    if update_position_time >= latest_position_time {
        apply_global_reading_record(&mut merged, update);
    } else if let Some(position) = newest_position(&merged) {
        apply_global_reading_position(&mut merged, &position);
    } else if update.updated_at >= merged.updated_at {
        apply_global_reading_record(&mut merged, update);
    }
    merged.updated_at = merged.updated_at.max(update.updated_at);
    merged
}

fn newest_position(record: &BookRecord) -> Option<ReadingPosition> {
    record
        .path_positions
        .values()
        .max_by_key(|position| position.updated_at)
        .cloned()
}

fn apply_global_reading_position(record: &mut BookRecord, position: &ReadingPosition) {
    record.last_page = position.last_page.min(record.total_pages.saturating_sub(1));
    record.last_page_name = position.last_page_name.clone();
    record.reading_direction = position.reading_direction;
    record.fit_mode = position.fit_mode;
    record.manual_zoom = position.manual_zoom;
    record.view_mode = position.view_mode.clone();
    record.strip_offset_frac = position.strip_offset_frac;
    record.smart_spread_phase = position.smart_spread_phase;
}

fn apply_global_reading_record(record: &mut BookRecord, update: &BookRecord) {
    record.last_page = update.last_page.min(record.total_pages.saturating_sub(1));
    record.last_page_name = update.last_page_name.clone();
    record.reading_direction = update.reading_direction;
    record.fit_mode = update.fit_mode;
    record.manual_zoom = update.manual_zoom;
    record.view_mode = update.view_mode.clone();
    record.strip_offset_frac = update.strip_offset_frac;
    record.smart_spread_phase = update.smart_spread_phase;
}

fn read_book_record_fresh(path: &Path, expected_book_id: &str) -> io::Result<Option<BookRecord>> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let record: BookRecord = serde_json::from_str(&text).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid book record JSON: {error}"),
        )
    })?;
    if record.book_id != expected_book_id {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "book record identity does not match its file name",
        ));
    }
    Ok(Some(record))
}

fn books_lock_path(books_dir: &Path) -> PathBuf {
    books_dir.join(".write.lock")
}

fn state_lock_path(state_path: &Path) -> PathBuf {
    let file_name = state_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("state.json");
    state_path.with_file_name(format!("{file_name}.write.lock"))
}

fn book_record_path(books_dir: &Path, book_id: &str) -> PathBuf {
    let portable_id: String = book_id
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
                character
            } else {
                '_'
            }
        })
        .collect();
    books_dir.join(format!("{portable_id}.json"))
}

fn write_book_record_atomic(path: &Path, record: &BookRecord) -> io::Result<()> {
    let text = serde_json::to_string_pretty(record)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("book");
    let temporary = path.with_file_name(format!("{file_name}.{}.tmp", std::process::id()));
    fs::write(&temporary, text)?;
    fs::rename(temporary, path)
}

fn read_state_fresh(path: &Path) -> io::Result<Option<PersistedState>> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    serde_json::from_str(&text).map(Some).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid state JSON: {error}"),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::{merge_legacy_bookmark_record, merge_reading_record, state_lock_path};
    use crate::core::state::{
        AppSettings, BookRecord, FastStartFailureNotice, FitMode, PageBookmark, PersistedState,
        ReadingDirection, ReadingPosition, RendererMode, StateStore, WindowPlacement,
    };
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn reading_merge_keeps_manual_bookmarks_and_uses_newest_position() {
        let mut latest = record_at(7, 20);
        latest.page_bookmarks.push(PageBookmark {
            page: 3,
            source_path: "book.cbz".into(),
            title: "Manual mark".into(),
            page_name: Some("004.jpg".into()),
            pinned: false,
            created_at: 19,
            updated_at: 20,
        });
        let update = record_at(2, 10);

        let merged = merge_reading_record(Some(latest), &update);

        assert_eq!(merged.last_page, 7);
        assert_eq!(merged.page_bookmarks.len(), 1);
        assert_eq!(merged.page_bookmarks[0].title, "Manual mark");
    }

    #[test]
    fn equally_recent_reading_positions_use_the_last_serialized_update() {
        let latest = record_at(7, 20);
        let update = record_at(2, 20);

        let merged = merge_reading_record(Some(latest), &update);

        assert_eq!(merged.last_page, 2);
        assert_eq!(merged.last_page_name.as_deref(), Some("002.jpg"));
    }

    #[test]
    fn legacy_bookmarks_merge_without_replacing_the_newer_reading_position() {
        let mut latest = record_at(7, 30);
        latest.page_bookmarks.push(PageBookmark {
            page: 2,
            source_path: "book.cbz".into(),
            title: "Newer manual mark".into(),
            page_name: Some("003.jpg".into()),
            pinned: false,
            created_at: 20,
            updated_at: 30,
        });
        let mut legacy = record_at(1, 10);
        legacy.page_bookmarks.extend([
            PageBookmark {
                page: 2,
                source_path: "book.cbz".into(),
                title: "Older duplicate".into(),
                page_name: Some("003.jpg".into()),
                pinned: false,
                created_at: 1,
                updated_at: 10,
            },
            PageBookmark {
                page: 4,
                source_path: "book.cbz".into(),
                title: "Legacy only".into(),
                page_name: Some("005.jpg".into()),
                pinned: false,
                created_at: 1,
                updated_at: 10,
            },
        ]);

        let (merged, changed) = merge_legacy_bookmark_record(Some(latest), &legacy);

        assert!(changed);
        assert_eq!(merged.last_page, 7);
        assert_eq!(merged.page_bookmarks.len(), 2);
        assert!(merged
            .page_bookmarks
            .iter()
            .any(|bookmark| bookmark.title == "Newer manual mark"));
        assert!(merged
            .page_bookmarks
            .iter()
            .any(|bookmark| bookmark.title == "Legacy only"));
    }

    #[test]
    fn stale_window_flush_preserves_settings_written_by_another_store() {
        let base = unique_base("state-window-settings");
        let mut window_store = store_at(&base);
        let mut settings_store = store_at(&base);
        let placement = WindowPlacement {
            inner_size: Some([1111.0, 777.0]),
            outer_position: Some([20.0, 30.0]),
            outer_position_px: Some([20, 30]),
            normal_rect_px: Some([20, 30, 1131, 807]),
            maximized: false,
        };
        assert!(window_store.update_window_placement_deferred(placement.clone()));
        let mut settings = settings_store.settings().clone();
        settings.show_status_bar = true;
        settings_store.update_settings(settings).unwrap();

        window_store.flush().unwrap();

        let reopened = store_at(&base);
        assert!(reopened.settings().show_status_bar);
        assert_eq!(reopened.window_placement(), &placement);
    }

    #[test]
    fn settings_three_way_merge_preserves_independent_changes_and_rejects_conflicts() {
        let base = unique_base("state-settings-merge");
        let mut first = store_at(&base);
        first.update_settings(AppSettings::default()).unwrap();
        let mut second = store_at(&base);

        let mut first_settings = first.settings().clone();
        first_settings.show_status_bar = true;
        first.update_settings(first_settings).unwrap();

        let mut second_settings = second.settings().clone();
        second_settings.show_filename_overlay = true;
        let merged = second.update_settings(second_settings).unwrap();
        assert!(merged.show_status_bar);
        assert!(merged.show_filename_overlay);

        let mut third = store_at(&base);
        let mut fourth = store_at(&base);
        let mut third_settings = third.settings().clone();
        third_settings.pixel_grid_min_zoom_pct = 900;
        third.update_settings(third_settings).unwrap();
        let mut fourth_settings = fourth.settings().clone();
        fourth_settings.pixel_grid_min_zoom_pct = 1000;
        let error = fourth
            .update_settings(fourth_settings)
            .expect_err("the same setting changed differently");
        assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
        assert_eq!(fourth.settings().pixel_grid_min_zoom_pct, 900);
        assert_eq!(store_at(&base).settings().pixel_grid_min_zoom_pct, 900);
    }

    #[test]
    fn stale_notice_dismiss_does_not_hide_a_new_failure() {
        let base = unique_base("state-notice-identity");
        let mut first = store_at(&base);
        first
            .record_fast_start_failure(notice("first", false))
            .unwrap();
        let mut stale = store_at(&base);
        first
            .record_fast_start_failure(notice("second", false))
            .unwrap();

        assert!(!stale.mark_fast_start_failure_notice_shown().unwrap());

        let reopened = store_at(&base);
        let latest = reopened.fast_start_failure_notice().unwrap();
        assert_eq!(latest.stage, "second");
        assert!(!latest.shown);
    }

    #[test]
    fn single_setting_update_preserves_notice_and_legacy_books() {
        let base = unique_base("state-domain-preservation");
        let mut state = PersistedState::default();
        state.fast_start_failure = Some(notice("existing", false));
        state.books.insert("zip:legacy".into(), record_at(3, 10));
        fs::create_dir_all(&base).unwrap();
        fs::write(
            base.join("state.json"),
            serde_json::to_string_pretty(&state).unwrap(),
        )
        .unwrap();
        let mut store = store_at(&base);

        store.update_top_bar_pinned(false).unwrap();

        let persisted: PersistedState =
            serde_json::from_str(&fs::read_to_string(base.join("state.json")).unwrap()).unwrap();
        assert!(!persisted.settings.top_bar_pinned);
        assert_eq!(persisted.fast_start_failure.unwrap().stage, "existing");
        assert!(persisted.books.contains_key("zip:legacy"));
    }

    #[test]
    fn corrupt_state_is_not_overwritten_by_a_settings_update() {
        let base = unique_base("state-corrupt");
        fs::create_dir_all(&base).unwrap();
        let state_path = base.join("state.json");
        let original = b"{ corrupt state";
        fs::write(&state_path, original).unwrap();
        let mut store = store_at_with_state(&base, PersistedState::default());

        let error = store
            .update_top_bar_pinned(false)
            .expect_err("malformed state must not be replaced");

        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(fs::read(state_path).unwrap(), original);
    }

    #[test]
    fn gpu_demotion_commits_renderer_and_notice_together_or_not_at_all() {
        let base = unique_base("state-gpu-demotion");
        let mut store = store_at(&base);
        let mut settings = store.settings().clone();
        settings.renderer_mode = RendererMode::Wgpu;
        store.update_settings(settings).unwrap();
        let before = fs::read(base.join("state.json")).unwrap();
        let lock_path = state_lock_path(&base.join("state.json"));
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_path)
            .unwrap();
        lock.lock().unwrap();

        let error = store
            .record_fast_start_failure_and_disable_gpu(notice("wgpu_prewarm", false))
            .expect_err("held state lock must reject the whole transaction");
        assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
        assert_eq!(fs::read(base.join("state.json")).unwrap(), before);

        lock.unlock().unwrap();
        drop(lock);
        store
            .record_fast_start_failure_and_disable_gpu(notice("wgpu_prewarm", false))
            .unwrap();
        let reopened = store_at(&base);
        assert_eq!(
            reopened.settings().renderer_mode,
            RendererMode::LowMemoryGlow
        );
        assert_eq!(
            reopened.fast_start_failure_notice().unwrap().stage,
            "wgpu_prewarm"
        );
    }

    fn notice(stage: &str, shown: bool) -> FastStartFailureNotice {
        FastStartFailureNotice {
            id: format!("id-{stage}"),
            generated_at: format!("time-{stage}"),
            stage: stage.to_owned(),
            error: format!("error-{stage}"),
            shown,
            ..Default::default()
        }
    }

    fn store_at(base: &Path) -> StateStore {
        let state = fs::read_to_string(base.join("state.json"))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        store_at_with_state(base, state)
    }

    fn store_at_with_state(base: &Path, state: PersistedState) -> StateStore {
        StateStore {
            path: base.join("state.json"),
            books_dir: base.join("books"),
            state,
            pending_books: BTreeMap::new(),
            state_dirty: false,
            books: Default::default(),
        }
    }

    fn unique_base(label: &str) -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join("suisuiview-tests").join(format!(
            "{label}-{stamp}-{}",
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn record_at(page: usize, updated_at: u64) -> BookRecord {
        let position = ReadingPosition {
            last_page: page,
            last_page_name: Some(format!("{page:03}.jpg")),
            reading_direction: ReadingDirection::LeftToRight,
            fit_mode: FitMode::FitPage,
            manual_zoom: None,
            view_mode: None,
            strip_offset_frac: None,
            smart_spread_phase: 0,
            updated_at,
        };
        BookRecord {
            book_id: "zip:test".into(),
            title: "Test".into(),
            last_page: page,
            last_page_name: position.last_page_name.clone(),
            total_pages: 10,
            known_paths: vec!["book.cbz".into()],
            reading_direction: ReadingDirection::LeftToRight,
            fit_mode: FitMode::FitPage,
            manual_zoom: None,
            view_mode: None,
            strip_offset_frac: None,
            smart_spread_phase: 0,
            path_positions: BTreeMap::from([("book.cbz".into(), position)]),
            page_bookmarks: Vec::new(),
            upscale_probe: None,
            updated_at,
        }
    }
}
