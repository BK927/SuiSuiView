//! Durable catalog revision, migration journal, and redirect storage.

use super::super::book_files;
use super::super::persistence::{book_record_path, write_book_record_atomic};
use super::super::BookRecord;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

fn book_catalog_revision_path(books_dir: &Path) -> PathBuf {
    books_dir.join(".catalog-revision")
}

pub(super) fn read_book_catalog_revision(books_dir: &Path) -> io::Result<u64> {
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

pub(super) fn begin_book_catalog_mutation(
    books_dir: &Path,
    stable_revision: u64,
) -> io::Result<u64> {
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

pub(super) fn finish_book_catalog_mutation(
    books_dir: &Path,
    mutation_revision: u64,
) -> io::Result<()> {
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
pub(super) fn stabilize_book_catalog_locked(books_dir: &Path) -> io::Result<u64> {
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

pub(super) fn book_redirect_path(books_dir: &Path, book_id: &str) -> PathBuf {
    book_record_path(books_dir, book_id).with_extension("redirect")
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct BookMigrationJournal {
    pub(super) old_book_id: String,
    pub(super) new_book_id: String,
    pub(super) destination: BookRecord,
}

fn book_migration_journal_path(books_dir: &Path) -> PathBuf {
    books_dir.join(".identity-migration.json")
}

pub(super) fn write_book_migration_journal(
    books_dir: &Path,
    journal: &BookMigrationJournal,
) -> io::Result<()> {
    let text = serde_json::to_string(journal)?;
    book_files::write_atomic(&book_migration_journal_path(books_dir), &text)
}

pub(super) fn remove_book_migration_journal(books_dir: &Path) -> io::Result<()> {
    match fs::remove_file(book_migration_journal_path(books_dir)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

pub(super) fn recover_book_record_migration(books_dir: &Path) -> io::Result<()> {
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

pub(in crate::core::state) fn read_book_redirect(
    books_dir: &Path,
    book_id: &str,
) -> io::Result<Option<String>> {
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

pub(super) fn write_book_redirect(
    books_dir: &Path,
    old_book_id: &str,
    new_book_id: &str,
) -> io::Result<()> {
    book_files::write_atomic(&book_redirect_path(books_dir, old_book_id), new_book_id)
}

pub(super) fn restore_book_record(path: &Path, record: Option<&BookRecord>) -> io::Result<()> {
    match record {
        Some(record) => write_book_record_atomic(path, record),
        None => match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        },
    }
}
