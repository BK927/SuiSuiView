use super::{next_source_instance_id, PageId, PageIdInterner};
use crate::core::source::{BookSource, FolderSource, SourceError};
use std::fs;
use std::path::PathBuf;

#[test]
fn same_name_interns_to_stable_id() {
    let interner = PageIdInterner::new();
    let first = interner.intern("cover.jpg");
    let again = interner.intern("cover.jpg");
    assert_eq!(first, again);
}

#[test]
fn distinct_names_get_distinct_ids() {
    let interner = PageIdInterner::new();
    let a = interner.intern("a.jpg");
    let b = interner.intern("b.jpg");
    assert_ne!(a, b);
}

#[test]
fn vanished_then_returned_name_keeps_old_id() {
    let interner = PageIdInterner::new();
    let a = interner.intern("a.jpg");
    let _b = interner.intern("b.jpg");
    // "a.jpg" vanished from a refreshed snapshot, then returned later.
    let a_again = interner.intern("a.jpg");
    assert_eq!(a, a_again);
}

#[test]
fn ids_are_monotonic() {
    let interner = PageIdInterner::new();
    assert_eq!(interner.intern("a.jpg"), PageId(0));
    assert_eq!(interner.intern("b.jpg"), PageId(1));
    assert_eq!(interner.intern("c.jpg"), PageId(2));
}

#[test]
fn source_instance_ids_are_strictly_increasing_and_nonzero() {
    let first = next_source_instance_id();
    let second = next_source_instance_id();
    assert!(first > 0);
    assert!(second > first);
}

#[test]
fn refresh_reassigns_survivors_and_drops_deleted_id() {
    let dir = temp_test_dir("refresh-survivors");
    for name in ["p1.jpg", "p2.jpg", "p3.jpg"] {
        fs::write(dir.join(name), b"x").unwrap();
    }

    let source = FolderSource::open(&dir).unwrap();
    let id0 = source.page_id(0).unwrap();
    let id1 = source.page_id(1).unwrap();
    let id2 = source.page_id(2).unwrap();
    let original_book_id = source.book_id().to_owned();
    let original_instance = source.source_instance_id();

    // Delete the middle page and refresh.
    fs::remove_file(dir.join("p2.jpg")).unwrap();
    let refreshed = source.refresh_snapshot().unwrap().unwrap();

    // Surviving names keep their ids; deleted id is unmapped.
    assert_eq!(refreshed.page_id(0), Some(id0));
    assert_eq!(refreshed.page_id(1), Some(id2));
    assert_eq!(refreshed.page_index_for_id(id0), Some(0));
    assert_eq!(refreshed.page_index_for_id(id2), Some(1));
    assert_eq!(refreshed.page_index_for_id(id1), None);

    // Order stays natural.
    assert_eq!(refreshed.page_name(0), Some("p1.jpg"));
    assert_eq!(refreshed.page_name(1), Some("p3.jpg"));

    // book_id is frozen; instance id changed.
    assert_eq!(refreshed.book_id(), original_book_id);
    assert_ne!(refreshed.source_instance_id(), original_instance);

    // Non-identity mapping: at least one page's id differs from its index,
    // proving we are not silently falling back to the trait defaults.
    assert_ne!(refreshed.page_id(1), Some(PageId(1)));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn vanished_then_recreated_file_keeps_page_id() {
    let dir = temp_test_dir("refresh-vanish-return");
    for name in ["p1.jpg", "p2.jpg", "p3.jpg"] {
        fs::write(dir.join(name), b"x").unwrap();
    }

    let source = FolderSource::open(&dir).unwrap();
    let original = source.page_id(1).unwrap();

    fs::remove_file(dir.join("p2.jpg")).unwrap();
    let refreshed = source.refresh_snapshot().unwrap().unwrap();
    assert_eq!(refreshed.page_index_for_id(original), None);

    fs::write(dir.join("p2.jpg"), b"x").unwrap();
    let restored = refreshed.refresh_snapshot().unwrap().unwrap();

    // The recreated file recovers its original id.
    assert_eq!(restored.page_id(1), Some(original));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn added_file_gets_new_monotonic_id() {
    let dir = temp_test_dir("refresh-added");
    for name in ["p1.jpg", "p2.jpg"] {
        fs::write(dir.join(name), b"x").unwrap();
    }

    let source = FolderSource::open(&dir).unwrap();
    let max_before = (0..source.page_count())
        .filter_map(|index| source.page_id(index))
        .max()
        .unwrap();

    fs::write(dir.join("p3.jpg"), b"x").unwrap();
    let refreshed = source.refresh_snapshot().unwrap().unwrap();

    let new_id = refreshed.page_id(2).unwrap();
    assert!(new_id > max_before);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn refresh_reruns_the_matching_collector() {
    let dir = temp_test_dir("refresh-collector");
    fs::write(dir.join("p1.jpg"), b"x").unwrap();
    let nested = dir.join("nested");
    fs::create_dir_all(&nested).unwrap();

    let recursive = FolderSource::open(&dir).unwrap();
    let direct = FolderSource::open_direct(&dir).unwrap();

    // Add a nested image after opening both sources.
    fs::write(nested.join("p2.jpg"), b"x").unwrap();

    let recursive_refreshed = recursive.refresh_snapshot().unwrap().unwrap();
    let direct_refreshed = direct.refresh_snapshot().unwrap().unwrap();

    // The recursive collector picks up the nested image; the flat one does not.
    assert_eq!(recursive_refreshed.page_count(), 2);
    assert_eq!(direct_refreshed.page_count(), 1);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn independent_sources_get_distinct_instance_ids() {
    let dir = temp_test_dir("distinct-instances");
    fs::write(dir.join("p1.jpg"), b"x").unwrap();

    let first = FolderSource::open(&dir).unwrap();
    let second = FolderSource::open(&dir).unwrap();

    assert_ne!(first.source_instance_id(), second.source_instance_id());

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn refresh_on_empty_folder_reports_no_pages() {
    let dir = temp_test_dir("refresh-empty");
    fs::write(dir.join("p1.jpg"), b"x").unwrap();
    let source = FolderSource::open(&dir).unwrap();

    fs::remove_file(dir.join("p1.jpg")).unwrap();
    let result = source.refresh_snapshot().unwrap();
    assert!(matches!(result, Err(SourceError::NoPages(_))));

    let _ = fs::remove_dir_all(&dir);
}

fn temp_test_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "suisuiview-{label}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}
