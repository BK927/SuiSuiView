use super::super::{
    AppSettings, BookRecordInput, FitMode, PersistedState, ReadingDirection, StateStore,
};
use std::path::Path;
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
    store.upsert_book_record(BookRecordInput {
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
    });

    let source_path = Path::new("C:/books/book-1");
    store.upsert_page_bookmark(
        "book-1",
        source_path,
        4,
        "Middle",
        Some("page-005.jpg".to_owned()),
    );
    store.upsert_page_bookmark(
        "book-1",
        source_path,
        1,
        "Start",
        Some("page-002.jpg".to_owned()),
    );

    let bookmarks = store.page_bookmarks("book-1");
    assert_eq!(bookmarks[0].page, 1);
    assert_eq!(bookmarks[1].page, 4);
    assert_eq!(bookmarks[1].page_name.as_deref(), Some("page-005.jpg"));
    assert_eq!(bookmarks[1].source_path, "C:/books/book-1");

    store.remove_page_bookmark("book-1", source_path, 4);
    assert!(!store.has_page_bookmark("book-1", source_path, 4));
    assert!(store.has_page_bookmark("book-1", source_path, 1));

    assert_eq!(store.clear_page_bookmarks("book-1", source_path), 1);
    assert!(store.page_bookmarks("book-1").is_empty());
    assert_eq!(store.clear_page_bookmarks("book-1", source_path), 0);
}

#[test]
fn page_bookmarks_are_scoped_by_source_path() {
    let mut store = test_store("page-bookmark-path-scope");
    store.upsert_book_record(BookRecordInput {
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
    });

    let first = Path::new("C:/books/first/book.cbz");
    let second = Path::new("D:/moved/book.cbz");
    store.upsert_page_bookmark("book-1", first, 4, "First", Some("004.jpg".to_owned()));
    store.upsert_page_bookmark("book-1", second, 4, "Second", Some("004.jpg".to_owned()));

    assert!(store.has_page_bookmark("book-1", first, 4));
    assert!(store.has_page_bookmark("book-1", second, 4));
    assert_eq!(store.page_bookmark_entries("book-1", first).len(), 1);
    assert_eq!(store.page_bookmark_entries("book-1", second).len(), 1);

    store.remove_page_bookmark("book-1", first, 4);

    assert!(!store.has_page_bookmark("book-1", first, 4));
    assert!(store.has_page_bookmark("book-1", second, 4));
}

#[test]
fn reading_position_can_use_identity_or_exact_path() {
    let mut store = test_store("reading-position-policy");
    store.upsert_book_record(BookRecordInput {
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
    });
    store.upsert_book_record(BookRecordInput {
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
    });

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
        store.upsert_book_record(BookRecordInput {
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
        });
    }
    store.upsert_page_bookmark(
        "book-1",
        Path::new("C:/books/book-1"),
        0,
        "Cover",
        Some("cover.png".to_owned()),
    );
    store.upsert_page_bookmark(
        "book-2",
        Path::new("C:/books/book-2.cbz"),
        3,
        "Page",
        Some("chapter/page.jpg".to_owned()),
    );

    let entries = store.all_page_bookmarks();
    assert_eq!(entries.len(), 2);
    assert!(entries.iter().any(|entry| entry.book_id == "book-1"));
    assert_eq!(store.clear_all_page_bookmarks(), 2);
    assert!(store.all_page_bookmarks().is_empty());
    assert!(store.book_record("book-1").is_some());
    assert_eq!(store.clear_all_page_bookmarks(), 0);
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
        pending_book: None,
        state_dirty: false,
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
    store.upsert_book_record(BookRecordInput {
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
    });

    let reopened = store_at(&base);
    let position = reopened
        .reading_position("book-1", Path::new("C:/books/book-1.zip"), true)
        .expect("record persisted to its own file");
    assert_eq!(position.last_page, 5);
    assert_eq!(position.last_page_name.as_deref(), Some("006.webp"));
}

#[test]
fn deferred_write_is_flushed_when_switching_books() {
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
    });
    assert!(changed);

    // Switching to another book must persist the buffered page for book-1.
    store.upsert_book_record(BookRecordInput {
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
    });

    let reopened = store_at(&base);
    assert_eq!(reopened.book_record("book-1").unwrap().last_page, 3);
    assert_eq!(reopened.book_record("book-2").unwrap().last_page, 1);
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
        pending_book: None,
        state_dirty: false,
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
    store.upsert_book_record(BookRecordInput {
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
    });
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
    store.upsert_book_record(BookRecordInput {
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
    });

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

fn unique_base(name: &str) -> std::path::PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir()
        .join("suisuiview-tests")
        .join(format!("{name}-{stamp}"))
}

fn store_at(base: &Path) -> StateStore {
    StateStore {
        path: base.join("state.json"),
        books_dir: base.join("books"),
        state: PersistedState::default(),
        pending_book: None,
        state_dirty: false,
    }
}

fn test_store(name: &str) -> StateStore {
    store_at(&unique_base(name))
}
