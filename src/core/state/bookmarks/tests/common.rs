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
    assert!(reopened
        .reading_position("with-bookmark", Path::new("C:/books/one.zip"), true,)
        .is_none());
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
