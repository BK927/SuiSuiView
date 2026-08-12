//! Cross-process transactions for the split book-record files.
//!
//! A `BookRecord` contains several independently owned domains: automatic
//! reading position, manual page bookmarks, and the upscaler probe. Writers
//! must therefore re-read the latest file while holding the sidecar lock and
//! change only the domain they own. Serializing a cached whole record would
//! silently erase another app instance's newer fields.

use super::bookmarks::page_bookmark_order;
use super::{BookRecord, ReadingPosition, StateStore};
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

#[cfg(test)]
mod tests {
    use super::{merge_legacy_bookmark_record, merge_reading_record};
    use crate::core::state::{
        BookRecord, FitMode, PageBookmark, ReadingDirection, ReadingPosition,
    };
    use std::collections::BTreeMap;

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
