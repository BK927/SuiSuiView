use super::super::{
    AppSettings, BookRecordInput, FitMode, PageBookmarkPathRebase, PersistedState,
    ReadingDirection, StateStore,
};
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn v2_state_loads_with_empty_page_bookmarks() {
    let json = r#"{
        "version": 2,
        "settings": {},
        "books": {
            "book-1": {
                "book_id": "book-1",
                "title": "Book One",
                "last_page": 3,
                "total_pages": 10,
                "known_paths": ["C:/books/book-1"],
                "reading_direction": "RightToLeft",
                "fit_mode": "FitPage",
                "updated_at": 100
            }
        }
    }"#;

    let state: PersistedState = serde_json::from_str(json).unwrap();
    let bookmark = state.books.get("book-1").unwrap();

    assert!(bookmark.page_bookmarks.is_empty());
    assert!(bookmark.path_positions.is_empty());
    assert!(bookmark.upscale_probe.is_none());
    assert!(!state.settings.show_status_bar);
    assert!(state.settings.resume_by_file_identity);
}

#[test]
fn upscale_probe_round_trips_and_defaults_when_absent() {
    use super::{BookRecord, UpscaleProbeRecord, UPSCALE_PROBE_VERSION};

    let base = r#"{"book_id":"b","title":"T","last_page":0,"total_pages":10,"known_paths":["p"],"reading_direction":"RightToLeft","fit_mode":"FitPage","updated_at":1"#;

    // Old records without the field still load, defaulting the probe to None.
    let without: BookRecord = serde_json::from_str(&(base.to_owned() + "}")).unwrap();
    assert!(without.upscale_probe.is_none());

    // A present probe survives a serialize/deserialize round-trip.
    let with_probe = base.to_owned()
        + r#","upscale_probe":{"winner":"wgsl_fsr1_easu_rcas","ssim_anime4k":0.91,"ssim_fsr":0.93,"pages":3,"version":1}}"#;
    let record: BookRecord = serde_json::from_str(&with_probe).unwrap();
    let reloaded: BookRecord =
        serde_json::from_str(&serde_json::to_string(&record).unwrap()).unwrap();
    assert_eq!(
        reloaded.upscale_probe,
        Some(UpscaleProbeRecord {
            winner: "wgsl_fsr1_easu_rcas".to_owned(),
            ssim_anime4k: 0.91,
            ssim_fsr: 0.93,
            pages: 3,
            version: UPSCALE_PROBE_VERSION,
        })
    );
}

#[test]
fn view_mode_and_strip_offset_round_trip_and_default_when_absent() {
    use super::BookRecord;

    let base = r#"{"book_id":"b","title":"T","last_page":0,"total_pages":10,"known_paths":["p"],"reading_direction":"RightToLeft","fit_mode":"FitPage","updated_at":1"#;

    // Legacy records without the fields still load, defaulting both to None.
    let without: BookRecord = serde_json::from_str(&(base.to_owned() + "}")).unwrap();
    assert!(without.view_mode.is_none());
    assert!(without.strip_offset_frac.is_none());

    // Present values survive a serialize/deserialize round-trip.
    let with_view = base.to_owned() + r#","view_mode":"vertical_strip","strip_offset_frac":0.375}"#;
    let record: BookRecord = serde_json::from_str(&with_view).unwrap();
    let reloaded: BookRecord =
        serde_json::from_str(&serde_json::to_string(&record).unwrap()).unwrap();
    assert_eq!(reloaded.view_mode.as_deref(), Some("vertical_strip"));
    assert_eq!(reloaded.strip_offset_frac, Some(0.375));
}

#[test]
fn page_bookmarks_add_and_remove() {
    let mut store = test_store("page-bookmarks");
    store
        .upsert_book_record(BookRecordInput {
            book_id: "book-1",
            title: "Book One",
            last_page: 0,
            last_page_name: None,
            total_pages: 20,
            path: Path::new("C:/books/book-1"),
            reading_direction: ReadingDirection::RightToLeft,
            fit_mode: FitMode::FitPage,
            manual_zoom: None,
            view_mode: None,
            strip_offset_frac: None,
            smart_spread_phase: 0,
        })
        .unwrap();

    let source_path = Path::new("C:/books/book-1");
    store
        .upsert_page_bookmark(
            "book-1",
            source_path,
            4,
            "Middle",
            Some("page-005.jpg".to_owned()),
        )
        .unwrap();
    store
        .upsert_page_bookmark(
            "book-1",
            source_path,
            1,
            "Start",
            Some("page-002.jpg".to_owned()),
        )
        .unwrap();

    let bookmarks = store.page_bookmarks("book-1");
    assert_eq!(bookmarks[0].page, 1);
    assert_eq!(bookmarks[1].page, 4);
    assert_eq!(bookmarks[1].page_name.as_deref(), Some("page-005.jpg"));
    assert_eq!(bookmarks[1].source_path, "C:/books/book-1");

    store
        .remove_page_bookmark("book-1", source_path, 4)
        .unwrap();
    assert!(!store.has_page_bookmark("book-1", source_path, 4));
    assert!(store.has_page_bookmark("book-1", source_path, 1));

    assert_eq!(
        store.clear_page_bookmarks("book-1", source_path).unwrap(),
        1
    );
    assert!(store.page_bookmarks("book-1").is_empty());
    assert_eq!(
        store.clear_page_bookmarks("book-1", source_path).unwrap(),
        0
    );
}

#[test]
fn page_bookmarks_are_scoped_by_source_path() {
    let mut store = test_store("page-bookmark-path-scope");
    store
        .upsert_book_record(BookRecordInput {
            book_id: "book-1",
            title: "Book One",
            last_page: 0,
            last_page_name: None,
            total_pages: 20,
            path: Path::new("C:/books/first/book.cbz"),
            reading_direction: ReadingDirection::RightToLeft,
            fit_mode: FitMode::FitPage,
            manual_zoom: None,
            view_mode: None,
            strip_offset_frac: None,
            smart_spread_phase: 0,
        })
        .unwrap();

    let first = Path::new("C:/books/first/book.cbz");
    let second = Path::new("D:/moved/book.cbz");
    store
        .upsert_page_bookmark("book-1", first, 4, "First", Some("004.jpg".to_owned()))
        .unwrap();
    store
        .upsert_page_bookmark("book-1", second, 4, "Second", Some("004.jpg".to_owned()))
        .unwrap();

    assert!(store.has_page_bookmark("book-1", first, 4));
    assert!(store.has_page_bookmark("book-1", second, 4));
    assert_eq!(store.page_bookmark_entries("book-1", first).len(), 1);
    assert_eq!(store.page_bookmark_entries("book-1", second).len(), 1);

    store.remove_page_bookmark("book-1", first, 4).unwrap();

    assert!(!store.has_page_bookmark("book-1", first, 4));
    assert!(store.has_page_bookmark("book-1", second, 4));
}

#[test]
fn reading_position_can_use_identity_or_exact_path() {
    let mut store = test_store("reading-position-policy");
    store
        .upsert_book_record(BookRecordInput {
            book_id: "book-1",
            title: "Book One",
            last_page: 2,
            last_page_name: Some("002.jpg"),
            total_pages: 20,
            path: Path::new("C:/books/book.cbz"),
            reading_direction: ReadingDirection::RightToLeft,
            fit_mode: FitMode::FitPage,
            manual_zoom: None,
            view_mode: None,
            strip_offset_frac: None,
            smart_spread_phase: 0,
        })
        .unwrap();
    store
        .upsert_book_record(BookRecordInput {
            book_id: "book-1",
            title: "Book One",
            last_page: 7,
            last_page_name: Some("007.jpg"),
            total_pages: 20,
            path: Path::new("D:/moved/book.cbz"),
            reading_direction: ReadingDirection::LeftToRight,
            fit_mode: FitMode::Manual,
            manual_zoom: Some(1.5),
            view_mode: None,
            strip_offset_frac: None,
            smart_spread_phase: 0,
        })
        .unwrap();

    let original = store
        .reading_position("book-1", Path::new("C:/books/book.cbz"), false)
        .unwrap();
    let identity = store
        .reading_position("book-1", Path::new("C:/books/book.cbz"), true)
        .unwrap();

    assert_eq!(original.last_page, 2);
    assert_eq!(original.last_page_name.as_deref(), Some("002.jpg"));
    assert_eq!(identity.last_page, 7);
    assert_eq!(identity.last_page_name.as_deref(), Some("007.jpg"));
    assert_eq!(identity.reading_direction, ReadingDirection::LeftToRight);
    assert_eq!(identity.manual_zoom, Some(1.5));
}

#[test]
fn all_page_bookmarks_and_clear_all_keep_book_records() {
    let mut store = test_store("all-page-bookmarks");
    for (book_id, path) in [
        ("book-1", "C:/books/book-1"),
        ("book-2", "C:/books/book-2.cbz"),
    ] {
        store
            .upsert_book_record(BookRecordInput {
                book_id,
                title: book_id,
                last_page: 0,
                last_page_name: None,
                total_pages: 20,
                path: Path::new(path),
                reading_direction: ReadingDirection::RightToLeft,
                fit_mode: FitMode::FitPage,
                manual_zoom: None,
                view_mode: None,
                strip_offset_frac: None,
                smart_spread_phase: 0,
            })
            .unwrap();
    }
    store
        .upsert_page_bookmark(
            "book-1",
            Path::new("C:/books/book-1"),
            0,
            "Cover",
            Some("cover.png".to_owned()),
        )
        .unwrap();
    store
        .upsert_page_bookmark(
            "book-2",
            Path::new("C:/books/book-2.cbz"),
            3,
            "Page",
            Some("chapter/page.jpg".to_owned()),
        )
        .unwrap();

    let entries = store.all_page_bookmarks();
    assert_eq!(entries.len(), 2);
    assert!(entries.iter().any(|entry| entry.book_id == "book-1"));
    assert_eq!(store.clear_all_page_bookmarks().unwrap(), 2);
    assert!(store.all_page_bookmarks().is_empty());
    assert!(store.book_record("book-1").is_some());
    assert_eq!(store.clear_all_page_bookmarks().unwrap(), 0);
}

#[test]
fn page_bookmarks_without_source_path_are_hidden() {
    let json = r#"{
        "version": 4,
        "settings": {},
        "books": {
            "book-1": {
                "book_id": "book-1",
                "title": "Book One",
                "last_page": 0,
                "total_pages": 10,
                "known_paths": ["C:/books/book-1"],
                "reading_direction": "RightToLeft",
                "fit_mode": "FitPage",
                "page_bookmarks": [{
                    "page": 2,
                    "title": "legacy",
                    "page_name": "002.jpg",
                    "pinned": false,
                    "created_at": 1,
                    "updated_at": 1
                }],
                "updated_at": 100
            }
        }
    }"#;

    let state: PersistedState = serde_json::from_str(json).unwrap();
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let base = std::env::temp_dir()
        .join("suisuiview-tests")
        .join(format!("hidden-page-bookmarks-{stamp}"));
    let mut store = StateStore {
        path: base.join("state.json"),
        books_dir: base.join("books"),
        state,
        pending_books: Default::default(),
        state_dirty: false,
        books: Default::default(),
    };
    store.import_legacy_bookmarks();

    assert_eq!(store.page_bookmarks("book-1").len(), 1);
    assert!(store.all_page_bookmarks().is_empty());
    assert_eq!(store.all_page_bookmark_count(), 0);
}

#[test]
fn settings_default_hides_status_bar() {
    assert!(!AppSettings::default().show_status_bar);
}

#[test]
fn settings_default_pins_top_bar() {
    assert!(AppSettings::default().top_bar_pinned);
}

#[test]
fn settings_default_resumes_by_file_identity() {
    assert!(AppSettings::default().resume_by_file_identity);
}

#[test]
fn settings_default_keeps_transition_off_for_parity() {
    assert!(!AppSettings::default().transition_effect);
    assert_eq!(
        AppSettings::default().page_transition_style,
        super::super::PageTransitionStyle::SlideFade
    );
}

#[test]
fn book_records_persist_across_store_instances() {
    let base = unique_base("persist-across");
    let mut store = store_at(&base);
    store
        .upsert_book_record(BookRecordInput {
            book_id: "book-1",
            title: "Book One",
            last_page: 5,
            last_page_name: Some("006.webp"),
            total_pages: 20,
            path: Path::new("C:/books/book-1.zip"),
            reading_direction: ReadingDirection::RightToLeft,
            fit_mode: FitMode::FitPage,
            manual_zoom: None,
            view_mode: None,
            strip_offset_frac: None,
            smart_spread_phase: 0,
        })
        .unwrap();

    let reopened = store_at(&base);
    let position = reopened
        .reading_position("book-1", Path::new("C:/books/book-1.zip"), true)
        .expect("record persisted to its own file");
    assert_eq!(position.last_page, 5);
    assert_eq!(position.last_page_name.as_deref(), Some("006.webp"));
}

#[test]
fn cached_book_records_still_reflect_writes_and_deletions() {
    // The store keeps parsed records in memory so the bookmark popover does not
    // re-read every book file each frame. The cache must never answer with what
    // the file no longer says.
    let base = unique_base("record-cache");
    let mut store = store_at(&base);
    for (book_id, path) in [
        ("book-1", "C:/books/book-1.zip"),
        ("book-2", "C:/books/book-2.cbz"),
    ] {
        store
            .upsert_book_record(BookRecordInput {
                book_id,
                title: book_id,
                last_page: 0,
                last_page_name: None,
                total_pages: 20,
                path: Path::new(path),
                reading_direction: ReadingDirection::RightToLeft,
                fit_mode: FitMode::FitPage,
                manual_zoom: None,
                view_mode: None,
                strip_offset_frac: None,
                smart_spread_phase: 0,
            })
            .unwrap();
    }
    store
        .upsert_page_bookmark(
            "book-1",
            Path::new("C:/books/book-1.zip"),
            0,
            "Cover",
            Some("cover.png".to_owned()),
        )
        .unwrap();
    // Populate the whole-library cache, then keep mutating behind it.
    assert_eq!(store.all_page_bookmarks().len(), 1);

    store
        .upsert_page_bookmark("book-2", Path::new("C:/books/book-2.cbz"), 7, "Later", None)
        .unwrap();
    assert_eq!(
        store.all_page_bookmarks().len(),
        2,
        "a book written after the scan must still show up"
    );
    assert!(store.has_page_bookmark("book-2", Path::new("C:/books/book-2.cbz"), 7));

    store
        .remove_page_bookmark("book-1", Path::new("C:/books/book-1.zip"), 0)
        .unwrap();
    assert_eq!(store.all_page_bookmarks().len(), 1);
    assert!(!store.has_page_bookmark("book-1", Path::new("C:/books/book-1.zip"), 0));

    // The cache is not a substitute for the file: a fresh store sees the same.
    let reopened = store_at(&base);
    assert_eq!(reopened.all_page_bookmarks().len(), 1);
}

#[test]
fn immediate_write_keeps_another_books_deferred_update_pending() {
    let base = unique_base("switch-flush");
    let mut store = store_at(&base);
    let changed = store.upsert_book_record_deferred(BookRecordInput {
        book_id: "book-1",
        title: "Book One",
        last_page: 3,
        last_page_name: None,
        total_pages: 10,
        path: Path::new("C:/books/one.zip"),
        reading_direction: ReadingDirection::RightToLeft,
        fit_mode: FitMode::FitPage,
        manual_zoom: None,
        view_mode: None,
        strip_offset_frac: None,
        smart_spread_phase: 0,
    });
    assert!(changed);

    // Saving book-2 must not make book-1's failure part of book-2's result.
    store
        .upsert_book_record(BookRecordInput {
            book_id: "book-2",
            title: "Book Two",
            last_page: 1,
            last_page_name: None,
            total_pages: 10,
            path: Path::new("C:/books/two.zip"),
            reading_direction: ReadingDirection::RightToLeft,
            fit_mode: FitMode::FitPage,
            manual_zoom: None,
            view_mode: None,
            strip_offset_frac: None,
            smart_spread_phase: 0,
        })
        .unwrap();

    let reopened = store_at(&base);
    assert!(reopened.book_record("book-1").is_none());
    assert_eq!(reopened.book_record("book-2").unwrap().last_page, 1);

    // The independent deferred write remains available to the normal timer or
    // shutdown flush instead of being discarded when another book is saved.
    store.flush().unwrap();
    let reopened = store_at(&base);
    assert_eq!(reopened.book_record("book-1").unwrap().last_page, 3);
    assert_eq!(reopened.book_record("book-2").unwrap().last_page, 1);
}

#[test]
fn recent_books_filters_pathless_resume_records_before_applying_the_limit() {
    let base = unique_base("recent-pathless-limit");
    let mut store = store_at(&base);
    for index in 0..2 {
        let book_id = format!("kept-{index}");
        let path = format!("C:/books/kept-{index}.cbz");
        store
            .upsert_book_record(BookRecordInput {
                book_id: &book_id,
                title: "Kept recent location",
                last_page: 0,
                last_page_name: None,
                total_pages: 1,
                path: Path::new(&path),
                reading_direction: ReadingDirection::RightToLeft,
                fit_mode: FitMode::FitPage,
                manual_zoom: None,
                view_mode: None,
                strip_offset_frac: None,
                smart_spread_phase: 0,
            })
            .unwrap();
    }

    store.state.settings.remember_recent_locations = false;
    for index in 0..10 {
        let book_id = format!("resume-only-{index}");
        let path = format!("C:/books/resume-only-{index}.cbz");
        store
            .upsert_book_record(BookRecordInput {
                book_id: &book_id,
                title: "Automatic resume only",
                last_page: 1,
                last_page_name: Some("002.jpg"),
                total_pages: 2,
                path: Path::new(&path),
                reading_direction: ReadingDirection::RightToLeft,
                fit_mode: FitMode::FitPage,
                manual_zoom: None,
                view_mode: None,
                strip_offset_frac: None,
                smart_spread_phase: 0,
            })
            .unwrap();
    }

    let recent = store.recent_books(8);
    assert_eq!(recent.len(), 2);
    assert!(recent
        .iter()
        .all(|record| record.book_id.starts_with("kept-") && !record.known_paths.is_empty()));
}

#[test]
fn archive_page_name_cleanup_finds_resume_paths_when_recent_locations_are_disabled() {
    let base = unique_base("archive-name-cleanup-path-position");
    let mut store = store_at(&base);
    store.state.settings.remember_recent_locations = false;
    store
        .upsert_book_record(BookRecordInput {
            book_id: "archive-resume-only",
            title: "Archive resume only",
            last_page: 2,
            last_page_name: Some("003.jpg"),
            total_pages: 4,
            path: Path::new("C:/books/resume-only.cbz"),
            reading_direction: ReadingDirection::RightToLeft,
            fit_mode: FitMode::FitPage,
            manual_zoom: None,
            view_mode: None,
            strip_offset_frac: None,
            smart_spread_phase: 0,
        })
        .unwrap();

    let before = store.book_record("archive-resume-only").unwrap();
    assert!(before.known_paths.is_empty());
    assert_eq!(store.clear_archive_page_names().unwrap(), 2);

    let after = store.book_record("archive-resume-only").unwrap();
    assert_eq!(after.last_page, 2);
    assert!(after.last_page_name.is_none());
    assert!(after
        .path_positions
        .values()
        .all(|position| position.last_page_name.is_none()));
}

#[test]
fn legacy_import_keeps_bookmarks_and_drops_resume() {
    let json = r#"{
        "version": 4,
        "settings": {},
        "books": {
            "with-bookmark": {
                "book_id": "with-bookmark",
                "title": "Bookmarked",
                "last_page": 7,
                "total_pages": 20,
                "known_paths": ["C:/books/one.zip"],
                "reading_direction": "RightToLeft",
                "fit_mode": "FitPage",
                "page_bookmarks": [{
                    "page": 5,
                    "source_path": "C:/books/one.zip",
                    "title": "mark",
                    "page_name": "006.webp",
                    "pinned": false,
                    "created_at": 1,
                    "updated_at": 1
                }],
                "updated_at": 100
            },
            "resume-only": {
                "book_id": "resume-only",
                "title": "Resume Only",
                "last_page": 9,
                "total_pages": 20,
                "known_paths": ["C:/books/two.zip"],
                "reading_direction": "RightToLeft",
                "fit_mode": "FitPage",
                "updated_at": 90
            }
        }
    }"#;
    let state: PersistedState = serde_json::from_str(json).unwrap();
    let base = unique_base("legacy-import");
    let mut store = StateStore {
        path: base.join("state.json"),
        books_dir: base.join("books"),
        state,
        pending_books: Default::default(),
        state_dirty: false,
        books: Default::default(),
    };
    store.import_legacy_bookmarks();

    let reopened = store_at(&base);
    // Bookmarked book: the manual bookmark survives, resume position is reset.
    let record = reopened
        .book_record("with-bookmark")
        .expect("bookmarked book is kept");
    assert_eq!(record.page_bookmarks.len(), 1);
    assert_eq!(record.last_page, 0);
    assert!(record.path_positions.is_empty());
    // Resume-only book (no manual bookmark) is discarded entirely.
    assert!(reopened.book_record("resume-only").is_none());
}

#[test]
fn failed_legacy_bookmark_import_keeps_the_original_for_retry() {
    let json = r#"{
        "version": 4,
        "settings": {},
        "books": {
            "legacy": {
                "book_id": "legacy",
                "title": "Legacy",
                "last_page": 3,
                "total_pages": 8,
                "known_paths": ["C:/books/legacy.cbz"],
                "reading_direction": "RightToLeft",
                "fit_mode": "FitPage",
                "page_bookmarks": [{
                    "page": 3,
                    "source_path": "C:/books/legacy.cbz",
                    "title": "Keep me",
                    "pinned": false,
                    "created_at": 1,
                    "updated_at": 1
                }],
                "updated_at": 1
            }
        }
    }"#;
    let base = unique_base("legacy-import-retry");
    let mut store = StateStore {
        path: base.join("state.json"),
        books_dir: base.join("books"),
        state: serde_json::from_str(json).unwrap(),
        pending_books: Default::default(),
        state_dirty: false,
        books: Default::default(),
    };
    fs::create_dir_all(base.join("books")).unwrap();
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(base.join("books").join(".write.lock"))
        .unwrap();
    lock.lock().unwrap();

    store.import_legacy_bookmarks();

    assert!(store.state.books.contains_key("legacy"));
    assert!(store.book_record("legacy").is_none());

    lock.unlock().unwrap();
    drop(lock);
    store.import_legacy_bookmarks();
    assert!(!store.state.books.contains_key("legacy"));
    assert_eq!(store.book_record("legacy").unwrap().page_bookmarks.len(), 1);
}

#[test]
fn smoke_real_folder_book_resume_round_trip() {
    use crate::core::source::open_source_from_path;

    let base = unique_base("smoke-folder");
    let comic = base.join("comic");
    write_test_png(&comic.join("001.png"), 32, 48);
    write_test_png(&comic.join("002.png"), 40, 40);
    write_test_png(&comic.join("003.png"), 24, 60);

    let (source, _forced) = open_source_from_path(&comic).expect("folder opens");
    assert_eq!(source.page_count(), 3);
    let book_id = source.book_id().to_owned();

    // Same content opened again yields the same identity (crux of the resume bug).
    let (again, _) = open_source_from_path(&comic).expect("folder reopens");
    assert_eq!(again.book_id(), book_id);

    // Save a reading position, then restore it from a fresh store instance.
    let store_dir = base.join("state");
    let mut store = store_at(&store_dir);
    store
        .upsert_book_record(BookRecordInput {
            book_id: &book_id,
            title: source.title(),
            last_page: 2,
            last_page_name: source.page_name(2),
            total_pages: source.page_count(),
            path: source.source_path(),
            reading_direction: ReadingDirection::RightToLeft,
            fit_mode: FitMode::FitPage,
            manual_zoom: None,
            view_mode: None,
            strip_offset_frac: None,
            smart_spread_phase: 0,
        })
        .unwrap();
    let reopened = store_at(&store_dir);
    assert_eq!(
        reopened
            .reading_position(&book_id, source.source_path(), true)
            .expect("resume restored")
            .last_page,
        2
    );

    // Changing the folder contents changes the identity (fresh start, no stale resume).
    write_test_png(&comic.join("004.png"), 30, 30);
    let (edited, _) = open_source_from_path(&comic).expect("folder reopens after edit");
    assert_ne!(edited.book_id(), book_id);
}

#[test]
fn smoke_real_zip_book_resume_round_trip() {
    use crate::core::source::open_source_from_path;

    let base = unique_base("smoke-zip");
    let archive = base.join("book.zip");
    write_test_zip(&archive, &["01.png", "02.png", "03.png"]);

    let (source, _forced) = open_source_from_path(&archive).expect("zip opens");
    assert_eq!(source.page_count(), 3);
    let book_id = source.book_id().to_owned();
    assert!(book_id.starts_with("zip:"));

    let store_dir = base.join("state");
    let mut store = store_at(&store_dir);
    store
        .upsert_book_record(BookRecordInput {
            book_id: &book_id,
            title: source.title(),
            last_page: 1,
            last_page_name: source.page_name(1),
            total_pages: source.page_count(),
            path: source.source_path(),
            reading_direction: ReadingDirection::RightToLeft,
            fit_mode: FitMode::FitPage,
            manual_zoom: None,
            view_mode: None,
            strip_offset_frac: None,
            smart_spread_phase: 0,
        })
        .unwrap();

    // Reopening the same archive from a fresh store restores the page (by name).
    let reopened = store_at(&store_dir);
    let position = reopened
        .reading_position(&book_id, source.source_path(), true)
        .expect("resume restored");
    assert_eq!(position.last_page, 1);
    assert_eq!(position.last_page_name.as_deref(), Some("02.png"));
}

fn write_test_png(path: &Path, width: u32, height: u32) {
    use image::{codecs::png::PngEncoder, ColorType, ImageEncoder};
    let pixels = vec![0u8; width as usize * height as usize * 3];
    let mut bytes = Vec::new();
    PngEncoder::new(&mut bytes)
        .write_image(&pixels, width, height, ColorType::Rgb8.into())
        .expect("encode test png");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

fn write_test_zip(path: &Path, page_names: &[&str]) {
    use image::{codecs::png::PngEncoder, ColorType, ImageEncoder};
    use std::io::Write;
    use zip::write::SimpleFileOptions;
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let file = std::fs::File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let options = SimpleFileOptions::default();
    for (index, name) in page_names.iter().enumerate() {
        let width = 24 + index as u32 * 4;
        let pixels = vec![0u8; width as usize * 32 * 3];
        let mut bytes = Vec::new();
        PngEncoder::new(&mut bytes)
            .write_image(&pixels, width, 32, ColorType::Rgb8.into())
            .expect("encode test png");
        zip.start_file(*name, options).expect("zip start_file");
        zip.write_all(&bytes).expect("zip write");
    }
    zip.finish().expect("zip finish");
}

/// A books directory no other test can be looking at. `cargo test` runs the lib
/// and bin targets as separate processes at the same time, and both compile
/// `core`, so the same test name runs twice concurrently. The Windows system
/// clock advances in ~15 ms steps, so a timestamp alone can collide — and tests
/// that scan the whole directory (`load_all_book_records`) then see each other's
/// records. Process id plus a per-process counter makes the name unique.
fn unique_base(name: &str) -> std::path::PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let seq = NEXT.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir()
        .join("suisuiview-tests")
        .join(format!("{name}-{stamp}-{}-{seq}", std::process::id()))
}

fn store_at(base: &Path) -> StateStore {
    StateStore {
        path: base.join("state.json"),
        books_dir: base.join("books"),
        state: PersistedState::default(),
        pending_books: Default::default(),
        state_dirty: false,
        books: Default::default(),
    }
}

fn test_store(name: &str) -> StateStore {
    store_at(&unique_base(name))
}

#[test]
fn stale_resume_flush_preserves_another_instance_manual_bookmark() {
    let base = unique_base("cross-instance-resume-bookmark");
    let path = Path::new("C:/books/shared.cbz");
    let mut first = store_at(&base);
    first
        .upsert_book_record(BookRecordInput {
            book_id: "zip:shared",
            title: "Shared",
            last_page: 1,
            last_page_name: Some("002.jpg"),
            total_pages: 12,
            path,
            reading_direction: ReadingDirection::RightToLeft,
            fit_mode: FitMode::FitPage,
            manual_zoom: None,
            view_mode: Some("single"),
            strip_offset_frac: None,
            smart_spread_phase: 0,
        })
        .unwrap();

    let mut second = store_at(&base);
    assert!(first.book_record("zip:shared").is_some());
    assert!(second.book_record("zip:shared").is_some());
    second
        .upsert_page_bookmark("zip:shared", path, 4, "Manual mark", Some("005.jpg".into()))
        .unwrap();

    assert!(first.upsert_book_record_deferred(BookRecordInput {
        book_id: "zip:shared",
        title: "Shared",
        last_page: 7,
        last_page_name: Some("008.jpg"),
        total_pages: 12,
        path,
        reading_direction: ReadingDirection::RightToLeft,
        fit_mode: FitMode::FitWidth,
        manual_zoom: None,
        view_mode: Some("single"),
        strip_offset_frac: None,
        smart_spread_phase: 0,
    }));
    first.flush().expect("automatic position flushes");

    let reopened = store_at(&base);
    let record = reopened.book_record("zip:shared").expect("shared record");
    assert_eq!(record.last_page, 7);
    assert_eq!(record.fit_mode, FitMode::FitWidth);
    assert_eq!(record.page_bookmarks.len(), 1);
    assert_eq!(record.page_bookmarks[0].title, "Manual mark");
}

#[test]
fn stale_manual_bookmark_writes_merge_instead_of_replacing_each_other() {
    let base = unique_base("cross-instance-manual-bookmarks");
    let path = Path::new("C:/books/shared.cbz");
    let mut first = store_at(&base);
    first
        .upsert_book_record(BookRecordInput {
            book_id: "zip:shared",
            title: "Shared",
            last_page: 0,
            last_page_name: Some("001.jpg"),
            total_pages: 12,
            path,
            reading_direction: ReadingDirection::RightToLeft,
            fit_mode: FitMode::FitPage,
            manual_zoom: None,
            view_mode: None,
            strip_offset_frac: None,
            smart_spread_phase: 0,
        })
        .unwrap();
    let mut second = store_at(&base);
    assert!(first.book_record("zip:shared").is_some());
    assert!(second.book_record("zip:shared").is_some());

    second
        .upsert_page_bookmark("zip:shared", path, 2, "Second", Some("003.jpg".into()))
        .unwrap();
    first
        .upsert_page_bookmark("zip:shared", path, 6, "First", Some("007.jpg".into()))
        .unwrap();

    let reopened = store_at(&base);
    let titles: Vec<_> = reopened
        .page_bookmarks("zip:shared")
        .into_iter()
        .map(|bookmark| bookmark.title)
        .collect();
    assert_eq!(titles.len(), 2);
    assert!(titles.contains(&"First".to_owned()));
    assert!(titles.contains(&"Second".to_owned()));
}

#[test]
fn busy_store_keeps_immediate_and_deferred_positions_for_retry() {
    let base = unique_base("busy-store-pending-map");
    let first_path = Path::new("C:/books/first.cbz");
    let second_path = Path::new("C:/books/second.cbz");
    let mut store = store_at(&base);
    store
        .upsert_book_record(BookRecordInput {
            book_id: "zip:first",
            title: "First",
            last_page: 0,
            last_page_name: Some("001.jpg"),
            total_pages: 10,
            path: first_path,
            reading_direction: ReadingDirection::RightToLeft,
            fit_mode: FitMode::FitPage,
            manual_zoom: None,
            view_mode: None,
            strip_offset_frac: None,
            smart_spread_phase: 0,
        })
        .unwrap();

    let lock_path = base.join("books").join(".write.lock");
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)
        .unwrap();
    lock.lock().unwrap();

    let error = store
        .upsert_book_record(BookRecordInput {
            book_id: "zip:first",
            title: "First",
            last_page: 6,
            last_page_name: Some("007.jpg"),
            total_pages: 10,
            path: first_path,
            reading_direction: ReadingDirection::RightToLeft,
            fit_mode: FitMode::FitWidth,
            manual_zoom: None,
            view_mode: Some("single"),
            strip_offset_frac: None,
            smart_spread_phase: 0,
        })
        .expect_err("held sidecar lock must bound the wait and return Busy");
    assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
    assert!(store.upsert_book_record_deferred(BookRecordInput {
        book_id: "zip:second",
        title: "Second",
        last_page: 3,
        last_page_name: Some("004.jpg"),
        total_pages: 10,
        path: second_path,
        reading_direction: ReadingDirection::LeftToRight,
        fit_mode: FitMode::FitPage,
        manual_zoom: None,
        view_mode: Some("double_ltr"),
        strip_offset_frac: None,
        smart_spread_phase: 0,
    }));
    assert_eq!(store.pending_books.len(), 2);

    lock.unlock().unwrap();
    drop(lock);
    store.flush().unwrap();

    let reopened = store_at(&base);
    assert_eq!(reopened.book_record("zip:first").unwrap().last_page, 6);
    assert_eq!(reopened.book_record("zip:second").unwrap().last_page, 3);
}

#[test]
fn failed_pending_record_does_not_block_another_books_immediate_save() {
    let base = unique_base("failed-pending-isolated");
    let books = base.join("books");
    fs::create_dir_all(&books).unwrap();
    let corrupt_path = books.join("zip_blocked.json");
    fs::write(&corrupt_path, b"{ not valid json").unwrap();
    let mut store = store_at(&base);

    assert!(store.upsert_book_record_deferred(BookRecordInput {
        book_id: "zip:blocked",
        title: "Blocked",
        last_page: 4,
        last_page_name: Some("005.jpg"),
        total_pages: 10,
        path: Path::new("C:/books/blocked.cbz"),
        reading_direction: ReadingDirection::RightToLeft,
        fit_mode: FitMode::FitPage,
        manual_zoom: None,
        view_mode: None,
        strip_offset_frac: None,
        smart_spread_phase: 0,
    }));

    store
        .upsert_book_record(BookRecordInput {
            book_id: "zip:healthy",
            title: "Healthy",
            last_page: 7,
            last_page_name: Some("008.jpg"),
            total_pages: 12,
            path: Path::new("C:/books/healthy.cbz"),
            reading_direction: ReadingDirection::LeftToRight,
            fit_mode: FitMode::FitWidth,
            manual_zoom: None,
            view_mode: Some("single"),
            strip_offset_frac: None,
            smart_spread_phase: 0,
        })
        .expect("the healthy book saves independently");

    assert!(store.pending_books.contains_key("zip:blocked"));
    assert!(!store.pending_books.contains_key("zip:healthy"));
    let reopened = store_at(&base);
    assert_eq!(reopened.book_record("zip:healthy").unwrap().last_page, 7);
    assert_eq!(fs::read(corrupt_path).unwrap(), b"{ not valid json");
}

#[test]
fn corrupt_book_record_is_not_overwritten_by_a_manual_bookmark_mutation() {
    let base = unique_base("corrupt-book-mutation");
    let books = base.join("books");
    fs::create_dir_all(&books).unwrap();
    let record_path = books.join("zip_corrupt.json");
    let original = b"{ this is not valid json";
    fs::write(&record_path, original).unwrap();
    let mut store = store_at(&base);

    assert!(store
        .upsert_page_bookmark(
            "zip:corrupt",
            Path::new("C:/books/corrupt.cbz"),
            2,
            "Must not write",
            Some("003.jpg".into()),
        )
        .is_err());

    assert_eq!(fs::read(record_path).unwrap(), original);
}

#[test]
fn remap_page_bookmarks_follows_names_and_drops_vanished_pages() {
    let mut store = test_store("page-bookmark-remap");
    store
        .upsert_book_record(BookRecordInput {
            book_id: "book-1",
            title: "Book One",
            last_page: 3,
            last_page_name: Some("004.jpg"),
            total_pages: 4,
            path: Path::new("C:/books/book-1"),
            reading_direction: ReadingDirection::LeftToRight,
            fit_mode: FitMode::FitPage,
            manual_zoom: None,
            view_mode: None,
            strip_offset_frac: None,
            smart_spread_phase: 0,
        })
        .unwrap();

    let source_path = Path::new("C:/books/book-1");
    store
        .upsert_page_bookmark("book-1", source_path, 1, "Second", Some("002.jpg".into()))
        .unwrap();
    store
        .upsert_page_bookmark("book-1", source_path, 2, "Third", Some("003.jpg".into()))
        .unwrap();
    store
        .upsert_page_bookmark("book-1", source_path, 3, "Fourth", Some("004.jpg".into()))
        .unwrap();
    // A legacy bookmark with no remembered name cannot be re-resolved.
    store
        .upsert_page_bookmark("book-1", source_path, 0, "First", None)
        .unwrap();

    // "002.jpg" was deleted from the folder, so everything after it shifts down.
    let automatic_before = store.book_record("book-1").unwrap();
    assert!(store
        .remap_page_bookmarks("book-1", source_path, |page_name| match page_name {
            "003.jpg" => Some(1),
            "004.jpg" => Some(2),
            _ => None,
        })
        .unwrap());

    let automatic_after = store.book_record("book-1").unwrap();
    assert_eq!(automatic_after.last_page, automatic_before.last_page);
    assert_eq!(
        automatic_after.last_page_name,
        automatic_before.last_page_name
    );
    assert_eq!(
        automatic_after.path_positions,
        automatic_before.path_positions
    );
    assert!(!store
        .remap_page_bookmarks("book-1", source_path, |page_name| match page_name {
            "003.jpg" => Some(1),
            "004.jpg" => Some(2),
            _ => None,
        })
        .unwrap());

    let bookmarks = store.page_bookmarks("book-1");
    let by_title: Vec<_> = bookmarks
        .iter()
        .map(|bookmark| (bookmark.title.as_str(), bookmark.page))
        .collect();
    assert_eq!(by_title, vec![("First", 0), ("Fourth", 2), ("Third", 1)]);
}

#[test]
fn remap_page_bookmarks_leaves_other_source_paths_alone() {
    let mut store = test_store("page-bookmark-remap-scope");
    store
        .upsert_book_record(BookRecordInput {
            book_id: "book-1",
            title: "Book One",
            last_page: 0,
            last_page_name: None,
            total_pages: 4,
            path: Path::new("C:/books/book-1"),
            reading_direction: ReadingDirection::LeftToRight,
            fit_mode: FitMode::FitPage,
            manual_zoom: None,
            view_mode: None,
            strip_offset_frac: None,
            smart_spread_phase: 0,
        })
        .unwrap();

    let refreshed = Path::new("C:/books/book-1");
    let other = Path::new("C:/books/elsewhere");
    store
        .upsert_page_bookmark("book-1", refreshed, 2, "Here", Some("003.jpg".into()))
        .unwrap();
    store
        .upsert_page_bookmark("book-1", other, 2, "Elsewhere", Some("003.jpg".into()))
        .unwrap();

    store
        .remap_page_bookmarks("book-1", refreshed, |_| None)
        .unwrap();

    let remaining: Vec<_> = store
        .page_bookmarks("book-1")
        .iter()
        .map(|bookmark| bookmark.title.clone())
        .collect();
    assert_eq!(remaining, vec!["Elsewhere".to_owned()]);
}

#[test]
fn adopt_record_for_path_rekeys_an_edited_folder_and_keeps_bookmarks() {
    let mut store = test_store("adopt-record-path");
    let folder = Path::new("C:/books/edited-folder");
    store
        .upsert_book_record(BookRecordInput {
            book_id: "folder:before",
            title: "Edited Folder",
            last_page: 12,
            last_page_name: Some("013.jpg"),
            total_pages: 40,
            path: folder,
            reading_direction: ReadingDirection::LeftToRight,
            fit_mode: FitMode::FitWidth,
            manual_zoom: None,
            view_mode: None,
            strip_offset_frac: None,
            smart_spread_phase: 0,
        })
        .unwrap();
    store
        .upsert_page_bookmark("folder:before", folder, 12, "Scene", Some("013.jpg".into()))
        .unwrap();

    // One image was added, so the content fingerprint is a different book id.
    assert!(store.adopt_record_for_path("folder:after", folder).unwrap());

    let adopted = store.book_record("folder:after").expect("re-keyed record");
    assert_eq!(adopted.last_page, 12);
    assert_eq!(adopted.last_page_name.as_deref(), Some("013.jpg"));
    assert_eq!(adopted.page_bookmarks.len(), 1);
    assert_eq!(adopted.page_bookmarks[0].title, "Scene");
    // The old id must not linger, or `page_bookmarks` would list the book twice.
    assert!(store.book_record("folder:before").is_none());
}

#[test]
fn adopt_record_reloads_the_source_before_rekeying() {
    let base = unique_base("adopt-record-fresh-source");
    let folder = Path::new("C:/books/edited-folder");
    let mut first = store_at(&base);
    first
        .upsert_book_record(BookRecordInput {
            book_id: "folder:before",
            title: "Edited Folder",
            last_page: 2,
            last_page_name: Some("003.jpg"),
            total_pages: 8,
            path: folder,
            reading_direction: ReadingDirection::LeftToRight,
            fit_mode: FitMode::FitPage,
            manual_zoom: None,
            view_mode: None,
            strip_offset_frac: None,
            smart_spread_phase: 0,
        })
        .unwrap();
    assert!(first.book_record("folder:before").is_some());

    let mut second = store_at(&base);
    second
        .upsert_page_bookmark(
            "folder:before",
            folder,
            5,
            "Other window",
            Some("006.jpg".into()),
        )
        .unwrap();

    assert!(first.adopt_record_for_path("folder:after", folder).unwrap());
    let reopened = store_at(&base);
    let adopted = reopened.book_record("folder:after").unwrap();
    assert_eq!(adopted.page_bookmarks.len(), 1);
    assert_eq!(adopted.page_bookmarks[0].title, "Other window");
    assert!(reopened.book_record("folder:before").is_none());
}

#[test]
fn adopt_record_surfaces_rekey_errors_without_creating_a_destination() {
    let base = unique_base("adopt-record-error");
    let folder = Path::new("C:/books/edited-folder");
    let mut store = store_at(&base);
    store
        .upsert_book_record(BookRecordInput {
            book_id: "folder:before",
            title: "Edited Folder",
            last_page: 2,
            last_page_name: Some("003.jpg"),
            total_pages: 8,
            path: folder,
            reading_direction: ReadingDirection::LeftToRight,
            fit_mode: FitMode::FitPage,
            manual_zoom: None,
            view_mode: None,
            strip_offset_frac: None,
            smart_spread_phase: 0,
        })
        .unwrap();
    assert!(store.book_record("folder:before").is_some());

    let old_record_path = base.join("books").join("folder_before.json");
    let corrupt = b"{ truncated";
    fs::write(&old_record_path, corrupt).unwrap();

    let error = store
        .adopt_record_for_path("folder:after", folder)
        .expect_err("a malformed source record must stop the re-key");
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    assert!(store.book_record("folder:after").is_none());
    assert_eq!(fs::read(old_record_path).unwrap(), corrupt);
}

#[test]
fn adopt_record_does_not_replace_a_destination_created_by_another_instance() {
    let base = unique_base("adopt-record-destination-race");
    let old_path = Path::new("C:/books/edited-folder");
    let destination_path = Path::new("D:/books/destination");
    let mut first = store_at(&base);
    first
        .upsert_book_record(BookRecordInput {
            book_id: "folder:before",
            title: "Old",
            last_page: 2,
            last_page_name: None,
            total_pages: 8,
            path: old_path,
            reading_direction: ReadingDirection::LeftToRight,
            fit_mode: FitMode::FitPage,
            manual_zoom: None,
            view_mode: None,
            strip_offset_frac: None,
            smart_spread_phase: 0,
        })
        .unwrap();
    assert!(first.book_record("folder:after").is_none());

    let mut second = store_at(&base);
    second
        .upsert_book_record(BookRecordInput {
            book_id: "folder:after",
            title: "Destination",
            last_page: 6,
            last_page_name: None,
            total_pages: 12,
            path: destination_path,
            reading_direction: ReadingDirection::RightToLeft,
            fit_mode: FitMode::FitWidth,
            manual_zoom: None,
            view_mode: None,
            strip_offset_frac: None,
            smart_spread_phase: 0,
        })
        .unwrap();

    assert!(!first
        .adopt_record_for_path("folder:after", old_path)
        .unwrap());
    let reopened = store_at(&base);
    assert_eq!(
        reopened.book_record("folder:after").unwrap().title,
        "Destination"
    );
    assert!(reopened.book_record("folder:before").is_some());
}

#[test]
fn adopt_record_for_path_does_nothing_when_the_id_already_resolves() {
    let mut store = test_store("adopt-record-existing");
    let folder = Path::new("C:/books/intact-folder");
    store
        .upsert_book_record(BookRecordInput {
            book_id: "folder:intact",
            title: "Intact",
            last_page: 3,
            last_page_name: None,
            total_pages: 10,
            path: folder,
            reading_direction: ReadingDirection::LeftToRight,
            fit_mode: FitMode::FitPage,
            manual_zoom: None,
            view_mode: None,
            strip_offset_frac: None,
            smart_spread_phase: 0,
        })
        .unwrap();

    assert!(!store
        .adopt_record_for_path("folder:intact", folder)
        .unwrap());
    assert_eq!(store.book_record("folder:intact").unwrap().last_page, 3);
}

#[test]
fn adopt_record_for_path_ignores_records_from_other_paths() {
    let mut store = test_store("adopt-record-other-path");
    store
        .upsert_book_record(BookRecordInput {
            book_id: "folder:elsewhere",
            title: "Elsewhere",
            last_page: 7,
            last_page_name: None,
            total_pages: 20,
            path: Path::new("C:/books/elsewhere"),
            reading_direction: ReadingDirection::LeftToRight,
            fit_mode: FitMode::FitPage,
            manual_zoom: None,
            view_mode: None,
            strip_offset_frac: None,
            smart_spread_phase: 0,
        })
        .unwrap();

    assert!(!store
        .adopt_record_for_path("folder:new", Path::new("C:/books/unrelated"))
        .unwrap());
    assert!(store.book_record("folder:new").is_none());
    assert!(store.book_record("folder:elsewhere").is_some());
}

#[test]
fn moved_archive_rebases_only_manual_bookmark_paths() {
    let base = unique_base("archive-bookmark-rebase");
    fs::create_dir_all(&base).unwrap();
    let old_path = base.join("old-name.cbz");
    let new_path = base.join("new-name.cbz");
    fs::write(&old_path, b"synthetic archive identity").unwrap();
    let mut store = store_at(&base);
    seed_archive_record(&mut store, "zip:rebase", &old_path, 7, "008.jpg");
    store
        .upsert_page_bookmark(
            "zip:rebase",
            &old_path,
            2,
            "Scene two",
            Some("003.jpg".into()),
        )
        .unwrap();
    store
        .upsert_page_bookmark(
            "zip:rebase",
            &old_path,
            5,
            "Scene five",
            Some("006.jpg".into()),
        )
        .unwrap();
    let before = store.book_record("zip:rebase").unwrap();

    fs::rename(&old_path, &new_path).unwrap();
    assert_eq!(
        store
            .rebase_moved_archive_page_bookmarks("zip:rebase", &new_path)
            .unwrap(),
        PageBookmarkPathRebase::Rebased(2)
    );

    let after = store.book_record("zip:rebase").unwrap();
    assert_eq!(after.last_page, before.last_page);
    assert_eq!(after.last_page_name, before.last_page_name);
    assert_eq!(after.path_positions, before.path_positions);
    assert!(after
        .page_bookmarks
        .iter()
        .all(|bookmark| bookmark.source_path == new_path.to_string_lossy()));
    let mut pages_and_names: Vec<_> = after
        .page_bookmarks
        .iter()
        .map(|bookmark| {
            (
                bookmark.page,
                bookmark.title.as_str(),
                bookmark.page_name.as_deref(),
                bookmark.created_at,
            )
        })
        .collect();
    let mut expected: Vec<_> = before
        .page_bookmarks
        .iter()
        .map(|bookmark| {
            (
                bookmark.page,
                bookmark.title.as_str(),
                bookmark.page_name.as_deref(),
                bookmark.created_at,
            )
        })
        .collect();
    pages_and_names.sort_by_key(|bookmark| bookmark.0);
    expected.sort_by_key(|bookmark| bookmark.0);
    assert_eq!(pages_and_names, expected);
}

#[test]
fn copied_archive_keeps_manual_bookmarks_scoped_to_the_original() {
    let base = unique_base("archive-bookmark-copy");
    fs::create_dir_all(&base).unwrap();
    let old_path = base.join("original.cbz");
    let new_path = base.join("copy.cbz");
    fs::write(&old_path, b"synthetic archive identity").unwrap();
    fs::copy(&old_path, &new_path).unwrap();
    let mut store = store_at(&base);
    seed_archive_record(&mut store, "zip:copy", &old_path, 3, "004.jpg");
    store
        .upsert_page_bookmark("zip:copy", &old_path, 1, "Original", Some("002.jpg".into()))
        .unwrap();

    assert_eq!(
        store
            .rebase_moved_archive_page_bookmarks("zip:copy", &new_path)
            .unwrap(),
        PageBookmarkPathRebase::NotNeeded
    );
    assert_eq!(
        store.page_bookmarks("zip:copy")[0].source_path,
        old_path.to_string_lossy()
    );
}

#[test]
fn archive_rebase_leaves_multiple_missing_sources_ambiguous() {
    let base = unique_base("archive-bookmark-ambiguous");
    fs::create_dir_all(&base).unwrap();
    let new_path = base.join("current.cbz");
    let first_missing = base.join("first-missing.cbz");
    let second_missing = base.join("second-missing.cbz");
    fs::write(&new_path, b"synthetic archive identity").unwrap();
    let mut store = store_at(&base);
    seed_archive_record(&mut store, "zip:ambiguous", &new_path, 2, "003.jpg");
    store
        .upsert_page_bookmark(
            "zip:ambiguous",
            &first_missing,
            0,
            "First",
            Some("001.jpg".into()),
        )
        .unwrap();
    store
        .upsert_page_bookmark(
            "zip:ambiguous",
            &second_missing,
            1,
            "Second",
            Some("002.jpg".into()),
        )
        .unwrap();

    assert_eq!(
        store
            .rebase_moved_archive_page_bookmarks("zip:ambiguous", &new_path)
            .unwrap(),
        PageBookmarkPathRebase::Ambiguous
    );
    let paths: Vec<_> = store
        .page_bookmarks("zip:ambiguous")
        .iter()
        .map(|bookmark| bookmark.source_path.clone())
        .collect();
    assert!(paths.contains(&first_missing.to_string_lossy().into_owned()));
    assert!(paths.contains(&second_missing.to_string_lossy().into_owned()));
}

#[test]
fn archive_rebase_does_not_merge_into_an_occupied_destination() {
    let base = unique_base("archive-bookmark-conflict");
    fs::create_dir_all(&base).unwrap();
    let old_path = base.join("missing.cbz");
    let new_path = base.join("current.cbz");
    fs::write(&new_path, b"synthetic archive identity").unwrap();
    let mut store = store_at(&base);
    seed_archive_record(&mut store, "zip:conflict", &new_path, 4, "005.jpg");
    store
        .upsert_page_bookmark("zip:conflict", &old_path, 1, "Old", Some("002.jpg".into()))
        .unwrap();
    store
        .upsert_page_bookmark("zip:conflict", &new_path, 3, "New", Some("004.jpg".into()))
        .unwrap();

    assert_eq!(
        store
            .rebase_moved_archive_page_bookmarks("zip:conflict", &new_path)
            .unwrap(),
        PageBookmarkPathRebase::Conflict
    );
    let paths: Vec<_> = store
        .page_bookmarks("zip:conflict")
        .iter()
        .map(|bookmark| bookmark.source_path.clone())
        .collect();
    assert!(paths.contains(&old_path.to_string_lossy().into_owned()));
    assert!(paths.contains(&new_path.to_string_lossy().into_owned()));
}

#[test]
fn archive_rebase_reads_the_latest_cross_process_record() {
    let base = unique_base("archive-bookmark-rebase-race");
    fs::create_dir_all(&base).unwrap();
    let old_path = base.join("old.cbz");
    let new_path = base.join("new.cbz");
    fs::write(&old_path, b"synthetic archive identity").unwrap();
    let mut first = store_at(&base);
    seed_archive_record(&mut first, "zip:race", &old_path, 1, "002.jpg");
    first
        .upsert_page_bookmark("zip:race", &old_path, 0, "First", Some("001.jpg".into()))
        .unwrap();
    let _stale_cache = first.book_record("zip:race").unwrap();
    fs::rename(&old_path, &new_path).unwrap();

    let mut second = store_at(&base);
    seed_archive_record(&mut second, "zip:race", &new_path, 8, "009.jpg");
    second
        .upsert_page_bookmark("zip:race", &old_path, 4, "Second", Some("005.jpg".into()))
        .unwrap();
    let expected_automatic = store_at(&base).book_record("zip:race").unwrap();

    assert_eq!(
        first
            .rebase_moved_archive_page_bookmarks("zip:race", &new_path)
            .unwrap(),
        PageBookmarkPathRebase::Rebased(2)
    );
    let persisted = store_at(&base).book_record("zip:race").unwrap();
    assert_eq!(persisted.last_page, expected_automatic.last_page);
    assert_eq!(persisted.last_page_name, expected_automatic.last_page_name);
    assert_eq!(persisted.path_positions, expected_automatic.path_positions);
    assert_eq!(persisted.page_bookmarks.len(), 2);
    assert!(persisted
        .page_bookmarks
        .iter()
        .all(|bookmark| bookmark.source_path == new_path.to_string_lossy()));
}

#[test]
fn archive_rebase_refuses_to_overwrite_a_corrupt_record() {
    let base = unique_base("archive-bookmark-rebase-corrupt");
    fs::create_dir_all(&base).unwrap();
    let old_path = base.join("old.cbz");
    let new_path = base.join("new.cbz");
    fs::write(&new_path, b"synthetic archive identity").unwrap();
    let mut store = store_at(&base);
    seed_archive_record(&mut store, "zip:corrupt", &old_path, 2, "003.jpg");
    store
        .upsert_page_bookmark(
            "zip:corrupt",
            &old_path,
            1,
            "Bookmark",
            Some("002.jpg".into()),
        )
        .unwrap();
    let record_path = base.join("books").join("zip_corrupt.json");
    let corrupt = b"{ truncated";
    fs::write(&record_path, corrupt).unwrap();

    let error = store
        .rebase_moved_archive_page_bookmarks("zip:corrupt", &new_path)
        .expect_err("corrupt JSON must stop a bookmark-path rebase");
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    assert_eq!(fs::read(record_path).unwrap(), corrupt);
}

#[cfg(windows)]
#[test]
fn archive_rebase_accepts_a_windows_case_only_path_change() {
    let base = unique_base("archive-bookmark-case-rebase");
    fs::create_dir_all(&base).unwrap();
    let recorded_path = base.join("Book.CBZ");
    let opened_path = base.join("book.cbz");
    fs::write(&recorded_path, b"synthetic archive identity").unwrap();
    let mut store = store_at(&base);
    seed_archive_record(&mut store, "zip:case", &recorded_path, 0, "001.jpg");
    store
        .upsert_page_bookmark(
            "zip:case",
            &recorded_path,
            0,
            "Case",
            Some("001.jpg".into()),
        )
        .unwrap();

    assert_eq!(
        store
            .rebase_moved_archive_page_bookmarks("zip:case", &opened_path)
            .unwrap(),
        PageBookmarkPathRebase::Rebased(1)
    );
    assert_eq!(
        store.page_bookmarks("zip:case")[0].source_path,
        opened_path.to_string_lossy()
    );
}

fn seed_archive_record(
    store: &mut StateStore,
    book_id: &str,
    path: &Path,
    last_page: usize,
    last_page_name: &str,
) {
    store
        .upsert_book_record(BookRecordInput {
            book_id,
            title: "Synthetic archive",
            last_page,
            last_page_name: Some(last_page_name),
            total_pages: 12,
            path,
            reading_direction: ReadingDirection::RightToLeft,
            fit_mode: FitMode::FitWidth,
            manual_zoom: Some(1.25),
            view_mode: Some("vertical_strip"),
            strip_offset_frac: Some(0.4),
            smart_spread_phase: 1,
        })
        .unwrap();
}
