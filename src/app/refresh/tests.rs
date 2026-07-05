use super::{folder_page_file_vanished, remap_current_page, ticket_matches, RefreshTicket};
use crate::core::source::{BookSource, FolderSource};
use crate::core::worker::NavigationDirection;
use std::fs;
use std::path::{Path, PathBuf};

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

fn folder_with(dir: &Path, names: &[&str]) -> FolderSource {
    for name in names {
        fs::write(dir.join(name), b"x").unwrap();
    }
    FolderSource::open(dir).unwrap()
}

#[test]
fn remap_keeps_surviving_page_at_shifted_index() {
    let dir = temp_test_dir("remap-survives");
    let old = folder_with(&dir, &["p1.jpg", "p2.jpg", "p3.jpg", "p4.jpg", "p5.jpg"]);

    // Current is p3 (index 2); delete a page before it so p3 shifts down by one.
    fs::remove_file(dir.join("p1.jpg")).unwrap();
    let new = old.refresh_snapshot().unwrap().unwrap();

    let remapped = remap_current_page(&old, new.as_ref(), 2, NavigationDirection::Forward);
    assert_eq!(new.page_name(remapped), Some("p3.jpg"));
    assert_eq!(remapped, 1);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn remap_current_deleted_forward_picks_next_neighbor() {
    let dir = temp_test_dir("remap-fwd");
    let old = folder_with(&dir, &["p1.jpg", "p2.jpg", "p3.jpg", "p4.jpg", "p5.jpg"]);

    fs::remove_file(dir.join("p3.jpg")).unwrap();
    let new = old.refresh_snapshot().unwrap().unwrap();

    let remapped = remap_current_page(&old, new.as_ref(), 2, NavigationDirection::Forward);
    assert_eq!(new.page_name(remapped), Some("p4.jpg"));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn remap_current_deleted_backward_picks_previous_neighbor() {
    let dir = temp_test_dir("remap-bwd");
    let old = folder_with(&dir, &["p1.jpg", "p2.jpg", "p3.jpg", "p4.jpg", "p5.jpg"]);

    fs::remove_file(dir.join("p3.jpg")).unwrap();
    let new = old.refresh_snapshot().unwrap().unwrap();

    let remapped = remap_current_page(&old, new.as_ref(), 2, NavigationDirection::Backward);
    assert_eq!(new.page_name(remapped), Some("p2.jpg"));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn remap_falls_to_earlier_side_when_everything_after_current_deleted() {
    let dir = temp_test_dir("remap-tail-gone");
    let old = folder_with(&dir, &["p1.jpg", "p2.jpg", "p3.jpg", "p4.jpg", "p5.jpg"]);

    for name in ["p3.jpg", "p4.jpg", "p5.jpg"] {
        fs::remove_file(dir.join(name)).unwrap();
    }
    let new = old.refresh_snapshot().unwrap().unwrap();

    // Forward from p3 finds nothing ahead, so it falls back to the earlier side.
    let remapped = remap_current_page(&old, new.as_ref(), 2, NavigationDirection::Forward);
    assert_eq!(new.page_name(remapped), Some("p2.jpg"));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn remap_returns_zero_when_all_pages_replaced() {
    let dir = temp_test_dir("remap-all-replaced");
    let old = folder_with(&dir, &["p1.jpg", "p2.jpg", "p3.jpg"]);

    for name in ["p1.jpg", "p2.jpg", "p3.jpg"] {
        fs::remove_file(dir.join(name)).unwrap();
    }
    for name in ["q1.jpg", "q2.jpg", "q3.jpg"] {
        fs::write(dir.join(name), b"x").unwrap();
    }
    let new = old.refresh_snapshot().unwrap().unwrap();

    let remapped = remap_current_page(&old, new.as_ref(), 1, NavigationDirection::Forward);
    assert_eq!(remapped, 0);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn refresh_then_remap_reflects_new_count_and_stable_ids() {
    let dir = temp_test_dir("refresh-integration");
    let old = folder_with(&dir, &["p1.jpg", "p2.jpg", "p3.jpg", "p4.jpg", "p5.jpg"]);
    let id_p2 = old.page_id(1).unwrap();
    let id_p4 = old.page_id(3).unwrap();

    fs::remove_file(dir.join("p3.jpg")).unwrap();
    let new = old.refresh_snapshot().unwrap().unwrap();

    // Count drops by one; surviving ids keep their identity at shifted indices.
    assert_eq!(new.page_count(), 4);
    assert_eq!(new.page_index_for_id(id_p2), Some(1));
    assert_eq!(new.page_index_for_id(id_p4), Some(2));

    // Current was p3 (deleted); Forward remap lands on p4.
    let remapped = remap_current_page(&old, new.as_ref(), 2, NavigationDirection::Forward);
    assert_eq!(new.page_index_for_id(id_p4), Some(remapped));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn page_file_vanished_only_when_file_is_gone() {
    let dir = temp_test_dir("probe-vanished");
    let source = folder_with(&dir, &["p1.jpg", "p2.jpg"]);

    // Both files exist: a decode failure here is corruption, not a vanished file.
    assert!(!folder_page_file_vanished(&source, 0));
    assert!(!folder_page_file_vanished(&source, 1));

    // Delete the on-disk file for index 1 (snapshot still lists it).
    fs::remove_file(dir.join("p2.jpg")).unwrap();
    assert!(folder_page_file_vanished(&source, 1));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn ticket_matches_requires_book_path_and_instance() {
    let ticket = RefreshTicket {
        book_id: "folder:abc".to_owned(),
        opened_path: PathBuf::from("/books/one"),
        source_instance_id: 7,
    };

    assert!(ticket_matches(
        Some("folder:abc"),
        Some(Path::new("/books/one")),
        Some(7),
        &ticket,
    ));
    // Book swapped underneath.
    assert!(!ticket_matches(
        Some("folder:other"),
        Some(Path::new("/books/one")),
        Some(7),
        &ticket,
    ));
    // A newer snapshot already replaced this instance.
    assert!(!ticket_matches(
        Some("folder:abc"),
        Some(Path::new("/books/one")),
        Some(8),
        &ticket,
    ));
    // No book open.
    assert!(!ticket_matches(None, None, None, &ticket));
}
