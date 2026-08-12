//! Background book-record discovery and short UI-side open finalization.
//!
//! Discovery scans without holding the cross-process books lock and returns a
//! revision-bound hint. The UI store later revalidates that hint under the lock
//! before it can migrate an identity or resolve automatic resume state.
//!
//! Adoption proof deliberately uses exact stored path text. Filesystem
//! canonicalization can change without a catalog write (for example when a
//! symlink is retargeted), so treating canonical aliases as proof would let an
//! otherwise current revision miss a newly ambiguous candidate.

use super::book_files;
use super::bookmarks::{page_bookmark_order, path_key};
use super::persistence::{
    apply_global_reading_position, book_record_path, books_lock_path, merge_reading_record,
    newest_position, read_book_record_fresh, write_book_record_atomic, ExclusiveFileLock,
};
use super::{
    BookRecord, BookRecordAdoption, FitMode, PageBookmark, PersistedState, ReadingDirection,
    StateStore,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

const DISCOVERY_RETRY_DELAY: Duration = Duration::from_millis(1);

/// State finalized by the UI store after it revalidates a background adoption
/// hint under the books lock.
///
/// `reading_position` is derived only from the automatic `path_positions`
/// domain. Manual page bookmarks stay behind the normal `StateStore` APIs.
#[derive(Debug, Clone)]
pub struct PreparedBookState {
    pub adoption: BookRecordAdoption,
    pub reading_position: Option<super::ReadingPosition>,
}

/// Best-effort read-only discovery produced away from the UI thread. The
/// catalog revision is rechecked before the UI store acts on the hint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BookRecordAdoptionHint {
    Candidate {
        book_id: String,
        preferred: bool,
        revision: u64,
    },
    DestinationExact {
        preferred_old_book_id: Option<String>,
        revision: u64,
    },
    NotFound {
        revision: u64,
    },
    Ambiguous {
        revision: u64,
    },
}

impl BookRecordAdoptionHint {
    fn revision(&self) -> u64 {
        match self {
            Self::Candidate { revision, .. }
            | Self::DestinationExact { revision, .. }
            | Self::NotFound { revision }
            | Self::Ambiguous { revision } => *revision,
        }
    }
}

#[derive(Debug)]
pub enum PrepareBookForOpenError {
    StaleHint,
    Io(io::Error),
}

impl std::fmt::Display for PrepareBookForOpenError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StaleHint => formatter.write_str("book record discovery changed while opening"),
            Self::Io(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for PrepareBookForOpenError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::StaleHint => None,
            Self::Io(error) => Some(error),
        }
    }
}

impl From<io::Error> for PrepareBookForOpenError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

pub(super) fn with_stable_book_catalog<R>(
    books_dir: &Path,
    access: impl FnOnce(u64) -> io::Result<R>,
) -> io::Result<R> {
    let _lock = ExclusiveFileLock::acquire(&books_lock_path(books_dir))?;
    let revision = stabilize_book_catalog_locked(books_dir)?;
    access(revision)
}

pub(super) fn run_book_catalog_mutation_locked<R>(
    books_dir: &Path,
    stable_revision: u64,
    mutate: impl FnOnce() -> io::Result<R>,
) -> io::Result<R> {
    let mutation_revision = begin_book_catalog_mutation(books_dir, stable_revision)?;
    let result = mutate();
    if let Err(original) = result.as_ref() {
        // A re-key can fail after its intent journal is durable. Complete that
        // transaction before declaring the catalog stable again; single-file
        // writers are atomic and recovery is a no-op for them.
        if let Err(recovery_error) = recover_book_record_migration(books_dir) {
            return Err(io::Error::new(
                original.kind(),
                format!(
                    "book catalog mutation failed ({original}); recovery also failed ({recovery_error})"
                ),
            ));
        }
    }
    let finish = finish_book_catalog_mutation(books_dir, mutation_revision);
    match (result, finish) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), _) => Err(error),
        (Ok(_), Err(error)) => Err(error),
    }
}

impl StateStore {
    /// Copy immutable app state and buffered automatic positions for a loader
    /// thread without carrying the UI store's parsed whole-library cache.
    pub fn fork_for_background(&self) -> Self {
        let state = PersistedState {
            version: self.state.version,
            settings: self.state.settings.clone(),
            window: self.state.window.clone(),
            fast_start_failure: self.state.fast_start_failure.clone(),
            // Legacy monolithic records are never consulted by background
            // adoption discovery. Avoid an O(library) deep clone on every open
            // when a previous legacy-import cleanup could not be persisted.
            books: BTreeMap::new(),
        };
        Self {
            path: self.path.clone(),
            books_dir: self.books_dir.clone(),
            state,
            pending_books: self.pending_books.clone(),
            state_dirty: self.state_dirty,
            books: Default::default(),
        }
    }

    /// Scan for a possible adoption without taking the books lock or changing
    /// persistent state. Call this on `fork_for_background()`.
    pub fn discover_book_record_adoption(
        &self,
        book_id: &str,
        preferred_old_book_id: Option<&str>,
        path: &Path,
    ) -> io::Result<BookRecordAdoptionHint> {
        self.discover_book_record_adoption_unlocked(book_id, preferred_old_book_id, path)
    }

    /// Revalidate a background hint and resolve the latest automatic resume in
    /// a short O(1) books-lock transaction on the UI store.
    pub fn prepare_book_for_open_from_hint(
        &mut self,
        book_id: &str,
        path: &Path,
        allow_identity_match: bool,
        hint: BookRecordAdoptionHint,
    ) -> Result<PreparedBookState, PrepareBookForOpenError> {
        self.prepare_book_for_open_from_hint_transaction(book_id, path, allow_identity_match, hint)
    }

    /// Move a single-path record onto `book_id` when the current filesystem
    /// location proves which record owns it.
    ///
    /// A book's identity is its content fingerprint, which survives moving and
    /// renaming — but not editing. Adding, removing, or re-saving a single image
    /// changes a folder's `book_id`, and the reading position and every bookmark
    /// become unreachable under an id nothing will ask for again. The app causes
    /// this itself: deleting one page from a folder book re-opens the same folder
    /// with a fresh fingerprint.
    ///
    /// `preferred_old_book_id` is the legacy fingerprint of the just-opened
    /// source. Looking it up first makes the common v1-to-v2 upgrade O(1), but
    /// the path must still match; a weak legacy fingerprint alone never proves
    /// that a moved folder is the same book.
    pub fn adopt_record_for_path(
        &mut self,
        book_id: &str,
        preferred_old_book_id: Option<&str>,
        path: &Path,
    ) -> io::Result<BookRecordAdoption> {
        // Compatibility path for non-loader callers. A stale catalog simply
        // retries discovery; loader integration can retry asynchronously.
        for _ in 0..3 {
            let background = self.fork_for_background();
            let hint =
                background.discover_book_record_adoption(book_id, preferred_old_book_id, path)?;
            match self.prepare_book_for_open_from_hint(book_id, path, false, hint) {
                Ok(prepared) => return Ok(prepared.adoption),
                Err(PrepareBookForOpenError::StaleHint) => continue,
                Err(PrepareBookForOpenError::Io(error)) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "book records kept changing during adoption",
        ))
    }

    pub(super) fn discover_book_record_adoption_unlocked(
        &self,
        book_id: &str,
        preferred_old_book_id: Option<&str>,
        path: &Path,
    ) -> io::Result<BookRecordAdoptionHint> {
        let wanted = path_key(path);
        let books_dir = &self.books_dir;
        for attempt in 0..6 {
            let revision = read_book_catalog_revision(books_dir)?;
            if revision % 2 != 0 {
                if attempt < 3 {
                    thread::sleep(DISCOVERY_RETRY_DELAY);
                    continue;
                }
                // An odd revision can be a live writer or a writer that
                // crashed. After a short wait, serialize with writers and
                // finish only the already-started catalog/journal recovery.
                // This never adopts the current loader's candidate.
                let _lock = ExclusiveFileLock::acquire(&books_lock_path(books_dir))?;
                stabilize_book_catalog_locked(books_dir)?;
                continue;
            }
            let hint = self.discover_book_record_adoption_at_revision(
                book_id,
                preferred_old_book_id,
                &wanted,
                revision,
            )?;
            if read_book_catalog_revision(books_dir)? == revision {
                return Ok(hint);
            }
        }
        Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "book records changed during background discovery",
        ))
    }

    fn discover_book_record_adoption_at_revision(
        &self,
        book_id: &str,
        preferred_old_book_id: Option<&str>,
        wanted: &str,
        revision: u64,
    ) -> io::Result<BookRecordAdoptionHint> {
        let destination = self.active_record_with_pending_fresh(book_id)?;
        let destination_has_exact_path = destination
            .as_ref()
            .is_some_and(|record| record_references_exact_path(record, wanted));
        let preferred = match preferred_old_book_id.filter(|old_id| *old_id != book_id) {
            Some(old_id) => self
                .active_record_with_pending_fresh(old_id)?
                .filter(|record| record_references_exact_path(record, wanted)),
            None => None,
        };

        if let Some(preferred) = preferred {
            let has_competing_candidate = self
                .load_active_records_with_pending_fresh()?
                .iter()
                .any(|record| {
                    record.book_id != book_id
                        && record.book_id != preferred.book_id
                        && record_references_exact_path(record, wanted)
                });
            return Ok(if has_competing_candidate {
                BookRecordAdoptionHint::Ambiguous { revision }
            } else {
                BookRecordAdoptionHint::Candidate {
                    book_id: preferred.book_id,
                    preferred: true,
                    revision,
                }
            });
        }
        if destination_has_exact_path {
            return Ok(BookRecordAdoptionHint::DestinationExact {
                preferred_old_book_id: preferred_old_book_id.map(ToOwned::to_owned),
                revision,
            });
        }

        let records = self.load_active_records_with_pending_fresh()?;
        let exact = records
            .iter()
            .filter(|record| {
                record.book_id != book_id && record_references_exact_path(record, wanted)
            })
            .collect::<Vec<_>>();
        Ok(match exact.as_slice() {
            [] => BookRecordAdoptionHint::NotFound { revision },
            [record] => BookRecordAdoptionHint::Candidate {
                book_id: record.book_id.clone(),
                preferred: false,
                revision,
            },
            _ => BookRecordAdoptionHint::Ambiguous { revision },
        })
    }

    pub(super) fn prepare_book_for_open_from_hint_transaction(
        &mut self,
        book_id: &str,
        path: &Path,
        allow_identity_match: bool,
        hint: BookRecordAdoptionHint,
    ) -> Result<PreparedBookState, PrepareBookForOpenError> {
        let books_dir = self.books_dir.clone();
        let _lock = ExclusiveFileLock::acquire(&books_lock_path(&books_dir))?;
        let revision = stabilize_book_catalog_locked(&books_dir)?;
        if hint.revision() != revision {
            return Err(PrepareBookForOpenError::StaleHint);
        }
        let wanted = path_key(path);
        let mut pending_candidates = Vec::new();
        for record in self.pending_books.values() {
            if record.book_id == book_id
                || !record_references_exact_path(record, &wanted)
                || read_book_redirect(&books_dir, &record.book_id)?.is_some()
            {
                continue;
            }
            pending_candidates.push(record.book_id.as_str());
        }
        let pending_hint_matches = match &hint {
            BookRecordAdoptionHint::Candidate {
                book_id: candidate, ..
            } => pending_candidates
                .iter()
                .all(|pending| *pending == candidate),
            BookRecordAdoptionHint::DestinationExact { .. } => true,
            BookRecordAdoptionHint::NotFound { .. } => pending_candidates.is_empty(),
            // An already-ambiguous hint cannot become unsafe through another
            // pending candidate because it never triggers a re-key.
            BookRecordAdoptionHint::Ambiguous { .. } => true,
        };
        if !pending_hint_matches {
            return Err(PrepareBookForOpenError::StaleHint);
        }
        // All hint kinds re-read the destination/candidate while holding the
        // lock. Revision equality protects the on-disk catalog; these checks
        // also cover the UI store's newer in-memory pending scope.
        let destination = self.active_record_with_pending_fresh(book_id)?;
        let hint_still_matches = match &hint {
            BookRecordAdoptionHint::Candidate {
                book_id: candidate,
                preferred,
                ..
            } => {
                let candidate_is_exact = self
                    .active_record_with_pending_fresh(candidate)?
                    .is_some_and(|record| record_references_exact_path(&record, &wanted));
                let destination_is_exact = destination
                    .as_ref()
                    .is_some_and(|record| record_references_exact_path(record, &wanted));
                candidate_is_exact && (*preferred || !destination_is_exact)
            }
            BookRecordAdoptionHint::DestinationExact {
                preferred_old_book_id,
                ..
            } => {
                let destination_is_exact = destination
                    .as_ref()
                    .is_some_and(|record| record_references_exact_path(record, &wanted));
                let preferred_became_candidate = match preferred_old_book_id {
                    Some(preferred) if preferred != book_id => self
                        .active_record_with_pending_fresh(preferred)?
                        .is_some_and(|record| record_references_exact_path(&record, &wanted)),
                    _ => false,
                };
                destination_is_exact && !preferred_became_candidate
            }
            BookRecordAdoptionHint::NotFound { .. } => destination
                .as_ref()
                .is_none_or(|record| !record_references_exact_path(record, &wanted)),
            BookRecordAdoptionHint::Ambiguous { .. } => true,
        };
        if !hint_still_matches {
            return Err(PrepareBookForOpenError::StaleHint);
        }
        let adoption = match hint {
            BookRecordAdoptionHint::Candidate {
                book_id: candidate, ..
            } => run_book_catalog_mutation_locked(&books_dir, revision, || {
                self.rekey_book_record_for_path_locked(&candidate, book_id, &wanted)
            })?,
            BookRecordAdoptionHint::DestinationExact { .. }
            | BookRecordAdoptionHint::NotFound { .. } => BookRecordAdoption::NotNeeded,
            BookRecordAdoptionHint::Ambiguous { .. } => BookRecordAdoption::Ambiguous,
        };

        let record = self.active_record_with_pending_fresh(book_id)?;
        let reading_position = record.as_ref().and_then(|record| {
            if allow_identity_match {
                record
                    .path_positions
                    .values()
                    .max_by_key(|position| position.updated_at)
                    .cloned()
            } else {
                record.path_positions.get(&wanted).cloned()
            }
        });
        let mut books = self.books.borrow_mut();
        match record.as_ref() {
            Some(record) => {
                books.records.insert(book_id.to_owned(), record.clone());
            }
            None => {
                books.records.remove(book_id);
            }
        }
        drop(books);
        if self.pending_books.contains_key(book_id) {
            // `read_book_record` intentionally prefers pending automatic state.
            // Refresh that snapshot with the latest manual/probe domains too,
            // otherwise it would hide the fresh transaction cache until flush.
            if let Some(record) = record.as_ref() {
                self.pending_books
                    .insert(book_id.to_owned(), record.clone());
            }
        }

        Ok(PreparedBookState {
            adoption,
            reading_position,
        })
    }

    fn active_record_with_pending_fresh(&self, book_id: &str) -> io::Result<Option<BookRecord>> {
        if read_book_redirect(&self.books_dir, book_id)?.is_some() {
            return Ok(None);
        }
        let latest = read_book_record_fresh(&book_record_path(&self.books_dir, book_id), book_id)?;
        Ok(match self.pending_books.get(book_id) {
            Some(pending) => Some(merge_reading_record(latest, pending)),
            None => latest,
        })
    }

    fn load_active_records_with_pending_fresh(&self) -> io::Result<Vec<BookRecord>> {
        let mut records = Vec::new();
        match fs::read_dir(&self.books_dir) {
            Ok(entries) => {
                for entry in entries {
                    let path = entry?.path();
                    if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
                        continue;
                    }
                    let Ok(text) = fs::read_to_string(&path) else {
                        continue;
                    };
                    let Ok(record) = serde_json::from_str::<BookRecord>(&text) else {
                        continue;
                    };
                    if read_book_redirect(&self.books_dir, &record.book_id)?.is_none() {
                        records.push(record);
                    }
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        for pending in self.pending_books.values() {
            if read_book_redirect(&self.books_dir, &pending.book_id)?.is_some() {
                continue;
            }
            match records
                .iter_mut()
                .find(|record| record.book_id == pending.book_id)
            {
                Some(record) => {
                    *record = merge_reading_record(Some(record.clone()), pending);
                }
                None => records.push(pending.clone()),
            }
        }
        Ok(records)
    }

    /// Re-key a path-proven, single-scope record without opening a gap between
    /// reading the source and committing the destination. Automatic resume and
    /// manual bookmarks are copied through separate helpers so neither domain
    /// can overwrite the other.
    fn rekey_book_record_for_path_locked(
        &mut self,
        old_book_id: &str,
        new_book_id: &str,
        expected_path: &str,
    ) -> io::Result<BookRecordAdoption> {
        if old_book_id == new_book_id {
            return Ok(BookRecordAdoption::NotNeeded);
        }
        if read_book_redirect(&self.books_dir, old_book_id)?.is_some() {
            return Ok(BookRecordAdoption::NotNeeded);
        }
        let old_path = book_record_path(&self.books_dir, old_book_id);
        let new_path = book_record_path(&self.books_dir, new_book_id);
        let new_redirect_path = book_redirect_path(&self.books_dir, new_book_id);
        let mut source = read_book_record_fresh(&old_path, old_book_id)?;
        if let Some(pending) = self.pending_books.get(old_book_id) {
            source = Some(merge_reading_record(source, pending));
        }
        let Some(source) = source else {
            return Ok(BookRecordAdoption::NotNeeded);
        };
        if !record_references_exact_path(&source, expected_path) {
            return Ok(BookRecordAdoption::NotNeeded);
        }
        // A single content identity can have several copies. Moving the entire
        // record after proving only one path would steal the other copies'
        // automatic positions and manual bookmarks, so fail closed here.
        if record_has_other_path_scope(&source, expected_path) {
            return Ok(BookRecordAdoption::Conflict);
        }

        let mut destination = read_book_record_fresh(&new_path, new_book_id)?;
        let destination_before = destination.clone();
        if let Some(pending) = self.pending_books.get(new_book_id) {
            destination = Some(merge_reading_record(destination, pending));
        }
        let destination_had_scope = destination
            .as_ref()
            .is_some_and(|record| record_references_exact_path(record, expected_path));
        if destination_had_scope
            && !manual_scope_already_present(
                destination.as_ref().expect("scope requires a record"),
                &source,
                expected_path,
            )
        {
            return Ok(BookRecordAdoption::Conflict);
        }

        let mut destination = destination.unwrap_or_else(|| {
            let mut record = source.clone();
            record.book_id = new_book_id.to_owned();
            record.known_paths.clear();
            record.path_positions.clear();
            record.page_bookmarks.clear();
            record.upscale_probe = None;
            reset_global_reading_position(&mut record);
            record
        });
        migrate_automatic_scope(
            &source,
            &mut destination,
            expected_path,
            destination_had_scope,
        );
        if !destination_had_scope {
            migrate_manual_scope(&source, &mut destination, expected_path);
        }
        destination.updated_at = destination.updated_at.max(source.updated_at);
        let journal = BookMigrationJournal {
            old_book_id: old_book_id.to_owned(),
            new_book_id: new_book_id.to_owned(),
            destination: destination.clone(),
        };
        write_book_migration_journal(&self.books_dir, &journal)?;
        write_book_record_atomic(&new_path, &destination)?;
        let destination_redirect = match fs::read_to_string(&new_redirect_path) {
            Ok(target) => Some(target),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => {
                let _ = restore_book_record(&new_path, destination_before.as_ref());
                return Err(error);
            }
        };
        if destination_redirect.is_some() {
            if let Err(error) = fs::remove_file(&new_redirect_path) {
                let _ = restore_book_record(&new_path, destination_before.as_ref());
                return Err(error);
            }
        }

        // A process that opened the old identity before this transaction can
        // still hold a deferred automatic position or accept a manual bookmark.
        // Leave a durable redirect marker before removing the JSON so those
        // stale writers drop their update instead of recreating the old record.
        if let Err(error) = write_book_redirect(&self.books_dir, old_book_id, new_book_id) {
            let rollback = restore_book_record(&new_path, destination_before.as_ref());
            if let Some(target) = destination_redirect.as_deref() {
                let _ = book_files::write_atomic(&new_redirect_path, target);
            }
            return match rollback {
                Ok(()) => Err(error),
                Err(rollback_error) => Err(io::Error::new(
                    error.kind(),
                    format!(
                        "could not retire old book identity ({error}); destination rollback also failed ({rollback_error})"
                    ),
                )),
            };
        }

        // The destination write is the commit point. From here on the app must
        // resolve the new identity even if antivirus software or a transient
        // handle prevents the obsolete source file from being removed.
        let mut books = self.books.borrow_mut();
        books.records.remove(old_book_id);
        books.records.insert(new_book_id.to_owned(), destination);
        drop(books);
        self.pending_books.remove(old_book_id);
        self.pending_books.remove(new_book_id);
        if fs::remove_file(&old_path).is_err() {
            let quarantine =
                old_path.with_extension(format!("stale-rekey-{}.tmp", std::process::id()));
            if fs::rename(&old_path, &quarantine).is_ok() {
                let _ = fs::remove_file(quarantine);
            }
        }
        remove_book_migration_journal(&self.books_dir)?;
        Ok(BookRecordAdoption::Adopted)
    }
}

fn record_references_exact_path(record: &BookRecord, expected_path: &str) -> bool {
    record.known_paths.iter().any(|path| path == expected_path)
        || record.path_positions.contains_key(expected_path)
        || record
            .page_bookmarks
            .iter()
            .any(|bookmark| bookmark.source_path == expected_path)
}

/// Raw stored paths that can make a record enter or leave adoption candidate
/// discovery. Page numbers, bookmark labels, probe data, and other metadata do
/// not affect this fingerprint, so ordinary page-turn saves avoid revision I/O.
fn record_path_scope_fingerprint(record: &BookRecord) -> Vec<&str> {
    let mut paths = record
        .known_paths
        .iter()
        .map(String::as_str)
        .chain(record.path_positions.keys().map(String::as_str))
        .chain(
            record
                .page_bookmarks
                .iter()
                .map(|bookmark| bookmark.source_path.as_str())
                .filter(|path| !path.is_empty()),
        )
        .collect::<Vec<_>>();
    paths.sort_unstable();
    paths.dedup();
    paths
}

pub(super) fn book_record_path_scope_changed(
    before: Option<&BookRecord>,
    after: Option<&BookRecord>,
) -> bool {
    let before = before
        .map(record_path_scope_fingerprint)
        .unwrap_or_default();
    let after = after.map(record_path_scope_fingerprint).unwrap_or_default();
    before != after
}

fn record_has_other_path_scope(record: &BookRecord, expected_path: &str) -> bool {
    record.known_paths.iter().any(|path| path != expected_path)
        || record
            .path_positions
            .keys()
            .any(|path| path != expected_path)
        || record.page_bookmarks.iter().any(|bookmark| {
            bookmark.source_path.is_empty() || bookmark.source_path != expected_path
        })
}

fn migrate_automatic_scope(
    source: &BookRecord,
    destination: &mut BookRecord,
    expected_path: &str,
    destination_had_scope: bool,
) {
    let source_position = source
        .path_positions
        .iter()
        .filter(|(path, _)| *path == expected_path)
        .map(|(_, position)| position)
        .max_by_key(|position| position.updated_at)
        .cloned();
    let destination_position = destination
        .path_positions
        .iter()
        .filter(|(path, _)| *path == expected_path)
        .map(|(_, position)| position)
        .max_by_key(|position| position.updated_at)
        .cloned();
    let selected = match (source_position, destination_position) {
        (Some(source), Some(destination)) if destination.updated_at > source.updated_at => {
            Some(destination)
        }
        (Some(source), _) => Some(source),
        (None, destination) => destination,
    };

    destination
        .path_positions
        .retain(|path, _| path != expected_path);
    if let Some(position) = selected {
        destination
            .path_positions
            .insert(expected_path.to_owned(), position);
    }

    let source_was_recent = source.known_paths.iter().any(|path| path == expected_path);
    if source_was_recent && !destination_had_scope {
        destination.known_paths.push(expected_path.to_owned());
        if destination.known_paths.len() > 8 {
            let extra = destination.known_paths.len() - 8;
            destination.known_paths.drain(0..extra);
        }
    }
    if let Some(position) = newest_position(destination) {
        apply_global_reading_position(destination, &position);
    }
}

fn reset_global_reading_position(record: &mut BookRecord) {
    record.last_page = 0;
    record.last_page_name = None;
    record.reading_direction = ReadingDirection::default();
    record.fit_mode = FitMode::default();
    record.manual_zoom = None;
    record.view_mode = None;
    record.strip_offset_frac = None;
    record.smart_spread_phase = 0;
}

fn migrate_manual_scope(source: &BookRecord, destination: &mut BookRecord, expected_path: &str) {
    destination
        .page_bookmarks
        .extend(source.page_bookmarks.iter().filter_map(|bookmark| {
            (bookmark.source_path == expected_path).then(|| {
                let mut bookmark = bookmark.clone();
                bookmark.source_path = expected_path.to_owned();
                bookmark
            })
        }));
    destination.page_bookmarks.sort_by(page_bookmark_order);
}

fn manual_scope_already_present(
    destination: &BookRecord,
    source: &BookRecord,
    expected_path: &str,
) -> bool {
    source
        .page_bookmarks
        .iter()
        .filter(|bookmark| bookmark.source_path == expected_path)
        .all(|source_bookmark| {
            destination
                .page_bookmarks
                .iter()
                .any(|destination_bookmark| {
                    destination_bookmark.source_path == expected_path
                        && same_manual_bookmark_except_path(destination_bookmark, source_bookmark)
                })
        })
}

fn same_manual_bookmark_except_path(left: &PageBookmark, right: &PageBookmark) -> bool {
    left.page == right.page
        && left.title == right.title
        && left.page_name == right.page_name
        && left.pinned == right.pinned
        && left.created_at == right.created_at
        && left.updated_at == right.updated_at
}

fn book_catalog_revision_path(books_dir: &Path) -> PathBuf {
    books_dir.join(".catalog-revision")
}

fn read_book_catalog_revision(books_dir: &Path) -> io::Result<u64> {
    match fs::read_to_string(book_catalog_revision_path(books_dir)) {
        Ok(text) => text.trim().parse::<u64>().map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid book catalog revision: {error}"),
            )
        }),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(0),
        Err(error) => Err(error),
    }
}

fn write_book_catalog_revision(books_dir: &Path, revision: u64) -> io::Result<()> {
    book_files::write_atomic(
        &book_catalog_revision_path(books_dir),
        &revision.to_string(),
    )
}

fn next_book_catalog_revision(revision: u64) -> io::Result<u64> {
    revision
        .checked_add(1)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "book catalog revision overflow"))
}

fn begin_book_catalog_mutation(books_dir: &Path, stable_revision: u64) -> io::Result<u64> {
    if stable_revision % 2 != 0 || read_book_catalog_revision(books_dir)? != stable_revision {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "book catalog is not at the expected stable revision",
        ));
    }
    let mutation_revision = next_book_catalog_revision(stable_revision)?;
    write_book_catalog_revision(books_dir, mutation_revision)?;
    Ok(mutation_revision)
}

fn finish_book_catalog_mutation(books_dir: &Path, mutation_revision: u64) -> io::Result<()> {
    if mutation_revision % 2 == 0 || read_book_catalog_revision(books_dir)? != mutation_revision {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "book catalog mutation revision changed unexpectedly",
        ));
    }
    write_book_catalog_revision(books_dir, next_book_catalog_revision(mutation_revision)?)
}

/// Repair a catalog left in an odd (in-progress) revision by a crashed writer,
/// or one with a durable migration journal created by an older build/test.
fn stabilize_book_catalog_locked(books_dir: &Path) -> io::Result<u64> {
    let revision = read_book_catalog_revision(books_dir)?;
    let has_journal = book_migration_journal_path(books_dir).is_file();
    if revision % 2 == 0 && !has_journal {
        return Ok(revision);
    }
    let mutation_revision = if revision % 2 == 0 {
        begin_book_catalog_mutation(books_dir, revision)?
    } else {
        revision
    };
    recover_book_record_migration(books_dir)?;
    finish_book_catalog_mutation(books_dir, mutation_revision)?;
    read_book_catalog_revision(books_dir)
}

fn book_redirect_path(books_dir: &Path, book_id: &str) -> PathBuf {
    book_record_path(books_dir, book_id).with_extension("redirect")
}

#[derive(Debug, Serialize, Deserialize)]
struct BookMigrationJournal {
    old_book_id: String,
    new_book_id: String,
    destination: BookRecord,
}

fn book_migration_journal_path(books_dir: &Path) -> PathBuf {
    books_dir.join(".identity-migration.json")
}

fn write_book_migration_journal(
    books_dir: &Path,
    journal: &BookMigrationJournal,
) -> io::Result<()> {
    let text = serde_json::to_string(journal)?;
    book_files::write_atomic(&book_migration_journal_path(books_dir), &text)
}

fn remove_book_migration_journal(books_dir: &Path) -> io::Result<()> {
    match fs::remove_file(book_migration_journal_path(books_dir)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn recover_book_record_migration(books_dir: &Path) -> io::Result<()> {
    let journal_path = book_migration_journal_path(books_dir);
    let text = match fs::read_to_string(&journal_path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let journal: BookMigrationJournal = serde_json::from_str(&text).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid book identity migration journal: {error}"),
        )
    })?;
    if journal.destination.book_id != journal.new_book_id {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "book identity migration journal destination does not match its target",
        ));
    }
    let destination = book_record_path(books_dir, &journal.new_book_id);
    write_book_record_atomic(&destination, &journal.destination)?;
    let destination_redirect = book_redirect_path(books_dir, &journal.new_book_id);
    match fs::remove_file(destination_redirect) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    write_book_redirect(books_dir, &journal.old_book_id, &journal.new_book_id)?;
    let source = book_record_path(books_dir, &journal.old_book_id);
    match fs::remove_file(source) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(_) => {
            // The redirect already makes a leftover JSON invisible and blocks
            // stale writers. A later recovery pass can retry cleanup.
        }
    }
    remove_book_migration_journal(books_dir)
}

pub(super) fn read_book_redirect(books_dir: &Path, book_id: &str) -> io::Result<Option<String>> {
    let path = book_redirect_path(books_dir, book_id);
    match fs::read_to_string(path) {
        Ok(target) if !target.trim().is_empty() => Ok(Some(target)),
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "book identity redirect is empty",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn write_book_redirect(books_dir: &Path, old_book_id: &str, new_book_id: &str) -> io::Result<()> {
    book_files::write_atomic(&book_redirect_path(books_dir, old_book_id), new_book_id)
}

fn restore_book_record(path: &Path, record: Option<&BookRecord>) -> io::Result<()> {
    match record {
        Some(record) => write_book_record_atomic(path, record),
        None => match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        },
    }
}

#[cfg(test)]
mod tests;
