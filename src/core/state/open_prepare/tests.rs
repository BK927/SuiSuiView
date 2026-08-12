use super::super::{
    BookRecordAdoption, BookRecordAdoptionHint, BookRecordInput, FitMode, PersistedState,
    PrepareBookForOpenError, ReadingDirection, StateStore,
};
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

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
fn adopt_record_does_not_use_a_canonical_alias_as_race_free_path_proof() {
    let base = unique_base("adopt-canonical-alias");
    let folder = base.join("folder");
    fs::create_dir_all(&folder).unwrap();
    let alias = folder.join("..").join("folder");
    let mut store = store_at(&base);
    store
        .upsert_book_record(BookRecordInput {
            book_id: "folder:legacy",
            title: "Aliased Folder",
            last_page: 3,
            last_page_name: Some("004.jpg"),
            total_pages: 8,
            path: &alias,
            reading_direction: ReadingDirection::LeftToRight,
            fit_mode: FitMode::FitWidth,
            manual_zoom: None,
            view_mode: None,
            strip_offset_frac: None,
            smart_spread_phase: 0,
        })
        .unwrap();
    store
        .upsert_page_bookmark(
            "folder:legacy",
            &alias,
            3,
            "Manual mark",
            Some("004.jpg".into()),
        )
        .unwrap();

    assert_eq!(
        store
            .adopt_record_for_path("folder-v2:new", Some("folder:legacy"), &folder)
            .unwrap(),
        BookRecordAdoption::NotNeeded
    );
    assert!(store.book_record("folder-v2:new").is_none());
    let original = store.book_record("folder:legacy").unwrap();
    assert!(original
        .path_positions
        .contains_key(alias.to_string_lossy().as_ref()));
    assert_eq!(
        original.page_bookmarks[0].source_path,
        alias.to_string_lossy()
    );
}

#[test]
fn adopt_record_treats_a_canonical_alias_as_a_distinct_scope() {
    let base = unique_base("adopt-canonical-alias-scope");
    let folder = base.join("folder");
    fs::create_dir_all(&folder).unwrap();
    let alias = folder.join("..").join("folder");
    let mut store = store_at(&base);
    for (path, page) in [(&folder, 1), (&alias, 4)] {
        store
            .upsert_book_record(BookRecordInput {
                book_id: "folder:legacy",
                title: "Aliased Folder",
                last_page: page,
                last_page_name: None,
                total_pages: 8,
                path,
                reading_direction: ReadingDirection::RightToLeft,
                fit_mode: FitMode::FitPage,
                manual_zoom: None,
                view_mode: None,
                strip_offset_frac: None,
                smart_spread_phase: 0,
            })
            .unwrap();
    }
    let before = store.book_record("folder:legacy").unwrap();

    assert_eq!(
        store
            .adopt_record_for_path("folder-v2:new", Some("folder:legacy"), &folder)
            .unwrap(),
        BookRecordAdoption::Conflict
    );
    assert!(store.book_record("folder-v2:new").is_none());
    let after = store.book_record("folder:legacy").unwrap();
    assert_eq!(after.path_positions, before.path_positions);
}

#[test]
fn background_fork_keeps_pending_positions_without_copying_the_book_cache() {
    let mut store = test_store("background-fork");
    let path = Path::new("C:/synthetic/background.cbz");
    store
        .upsert_book_record(BookRecordInput {
            book_id: "zip:background",
            title: "Background",
            last_page: 1,
            last_page_name: Some("002.jpg"),
            total_pages: 20,
            path,
            reading_direction: ReadingDirection::RightToLeft,
            fit_mode: FitMode::FitPage,
            manual_zoom: None,
            view_mode: None,
            strip_offset_frac: None,
            smart_spread_phase: 0,
        })
        .unwrap();
    assert!(store.upsert_book_record_deferred(BookRecordInput {
        book_id: "zip:background",
        title: "Background",
        last_page: 8,
        last_page_name: Some("009.jpg"),
        total_pages: 20,
        path,
        reading_direction: ReadingDirection::RightToLeft,
        fit_mode: FitMode::FitWidth,
        manual_zoom: Some(1.25),
        view_mode: Some("single"),
        strip_offset_frac: None,
        smart_spread_phase: 1,
    }));
    assert!(!store.books.borrow().records.is_empty());

    let background = store.fork_for_background();

    assert_eq!(background.pending_books["zip:background"].last_page, 8);
    assert!(background.books.borrow().records.is_empty());
    assert!(!background.books.borrow().all_loaded);
    assert!(background.state.books.is_empty());
}

#[test]
fn background_fork_does_not_clone_legacy_monolithic_book_records() {
    let mut store = test_store("background-legacy-books");
    store.state.books.insert(
        "legacy:only".into(),
        super::BookRecord {
            book_id: "legacy:only".into(),
            title: "Legacy".into(),
            last_page: 3,
            last_page_name: Some("004.jpg".into()),
            total_pages: 10,
            known_paths: vec!["C:/synthetic/legacy".into()],
            reading_direction: ReadingDirection::RightToLeft,
            fit_mode: FitMode::FitPage,
            manual_zoom: None,
            view_mode: None,
            strip_offset_frac: None,
            smart_spread_phase: 0,
            path_positions: Default::default(),
            page_bookmarks: Vec::new(),
            upscale_probe: None,
            updated_at: 1,
        },
    );

    let background = store.fork_for_background();

    assert!(background.state.books.is_empty());
    assert_eq!(background.settings(), store.settings());
    assert_eq!(background.window_placement(), store.window_placement());
}

#[test]
fn prepare_book_for_open_rekeys_and_resolves_only_the_automatic_position() {
    let base = unique_base("prepare-open-rekey");
    let path = Path::new("C:/synthetic/edited-folder");
    let mut store = store_at(&base);
    store
        .upsert_book_record(BookRecordInput {
            book_id: "folder:before",
            title: "Edited folder",
            last_page: 6,
            last_page_name: Some("007.jpg"),
            total_pages: 12,
            path,
            reading_direction: ReadingDirection::LeftToRight,
            fit_mode: FitMode::FitWidth,
            manual_zoom: Some(1.5),
            view_mode: Some("double"),
            strip_offset_frac: None,
            smart_spread_phase: 1,
        })
        .unwrap();
    store
        .upsert_page_bookmark(
            "folder:before",
            path,
            3,
            "Manual mark",
            Some("004.jpg".into()),
        )
        .unwrap();

    let background = store.fork_for_background();
    let hint = background
        .discover_book_record_adoption("folder-v2:after", Some("folder:before"), path)
        .unwrap();
    let prepared = store
        .prepare_book_for_open_from_hint("folder-v2:after", path, false, hint)
        .unwrap();

    assert_eq!(prepared.adoption, BookRecordAdoption::Adopted);
    let position = prepared.reading_position.as_ref().unwrap();
    assert_eq!(position.last_page, 6);
    assert_eq!(position.last_page_name.as_deref(), Some("007.jpg"));
    let record = store.book_record("folder-v2:after").unwrap();
    assert_eq!(record.book_id, "folder-v2:after");
    assert_eq!(record.page_bookmarks.len(), 1);
    assert_eq!(record.page_bookmarks[0].page, 3);
}

#[test]
fn prepare_book_for_open_does_not_turn_a_manual_bookmark_into_resume() {
    let path = Path::new("C:/synthetic/manual-only");
    let mut store = test_store("prepare-manual-only");
    store
        .ensure_book_record_shell("folder-v2:manual-only", "Manual only", 10)
        .unwrap();
    store
        .upsert_page_bookmark(
            "folder-v2:manual-only",
            path,
            7,
            "Manual mark",
            Some("008.jpg".into()),
        )
        .unwrap();

    let background = store.fork_for_background();
    let hint = background
        .discover_book_record_adoption("folder-v2:manual-only", None, path)
        .unwrap();
    let prepared = store
        .prepare_book_for_open_from_hint("folder-v2:manual-only", path, true, hint)
        .unwrap();

    assert!(prepared.reading_position.is_none());
    assert_eq!(store.page_bookmarks("folder-v2:manual-only").len(), 1);
}

#[test]
fn ui_finalize_keeps_new_ui_resume_and_fresh_manual_metadata() {
    use super::super::{UpscaleProbeRecord, UPSCALE_PROBE_VERSION};

    let base = unique_base("accept-prepared-open");
    let path = Path::new("C:/synthetic/shared.cbz");
    let mut ui = store_at(&base);
    ui.upsert_book_record(BookRecordInput {
        book_id: "zip:prepared",
        title: "Prepared",
        last_page: 1,
        last_page_name: Some("002.jpg"),
        total_pages: 20,
        path,
        reading_direction: ReadingDirection::RightToLeft,
        fit_mode: FitMode::FitPage,
        manual_zoom: None,
        view_mode: None,
        strip_offset_frac: None,
        smart_spread_phase: 0,
    })
    .unwrap();
    assert!(ui.upsert_book_record_deferred(BookRecordInput {
        book_id: "zip:prepared",
        title: "Prepared",
        last_page: 4,
        last_page_name: Some("005.jpg"),
        total_pages: 20,
        path,
        reading_direction: ReadingDirection::RightToLeft,
        fit_mode: FitMode::FitWidth,
        manual_zoom: None,
        view_mode: Some("single"),
        strip_offset_frac: None,
        smart_spread_phase: 0,
    }));
    let background = ui.fork_for_background();

    let mut other = store_at(&base);
    other
        .upsert_page_bookmark(
            "zip:prepared",
            path,
            2,
            "Fresh manual mark",
            Some("003.jpg".into()),
        )
        .unwrap();
    let probe = UpscaleProbeRecord {
        winner: "wgsl_fsr1_easu_rcas".into(),
        ssim_anime4k: 0.9,
        ssim_fsr: 0.95,
        pages: 2,
        version: UPSCALE_PROBE_VERSION,
    };
    other
        .set_book_upscale_probe("zip:prepared", probe.clone())
        .unwrap();
    let hint = background
        .discover_book_record_adoption("zip:prepared", None, path)
        .unwrap();

    assert!(ui.upsert_book_record_deferred(BookRecordInput {
        book_id: "zip:prepared",
        title: "Prepared",
        last_page: 9,
        last_page_name: Some("010.jpg"),
        total_pages: 20,
        path,
        reading_direction: ReadingDirection::LeftToRight,
        fit_mode: FitMode::Original,
        manual_zoom: Some(2.0),
        view_mode: Some("double"),
        strip_offset_frac: None,
        smart_spread_phase: 1,
    }));
    ui.prepare_book_for_open_from_hint("zip:prepared", path, false, hint)
        .unwrap();

    let accepted = ui.book_record("zip:prepared").unwrap();
    assert_eq!(accepted.last_page, 9);
    assert_eq!(accepted.last_page_name.as_deref(), Some("010.jpg"));
    assert_eq!(accepted.page_bookmarks.len(), 1);
    assert_eq!(accepted.page_bookmarks[0].title, "Fresh manual mark");
    assert_eq!(accepted.upscale_probe, Some(probe));
}

#[test]
fn preferred_prepare_candidate_still_fails_closed_on_another_matching_record() {
    let base = unique_base("prepare-preferred-ambiguous");
    let path = Path::new("C:/synthetic/shared-folder");
    let mut store = store_at(&base);
    for book_id in ["folder:preferred", "folder:competing"] {
        store
            .upsert_book_record(BookRecordInput {
                book_id,
                title: "Shared folder",
                last_page: 1,
                last_page_name: Some("002.jpg"),
                total_pages: 8,
                path,
                reading_direction: ReadingDirection::RightToLeft,
                fit_mode: FitMode::FitPage,
                manual_zoom: None,
                view_mode: None,
                strip_offset_frac: None,
                smart_spread_phase: 0,
            })
            .unwrap();
    }

    let background = store.fork_for_background();
    let hint = background
        .discover_book_record_adoption("folder-v2:new", Some("folder:preferred"), path)
        .unwrap();
    let prepared = store
        .prepare_book_for_open_from_hint("folder-v2:new", path, false, hint)
        .unwrap();

    assert_eq!(prepared.adoption, BookRecordAdoption::Ambiguous);
    assert!(store.book_record("folder-v2:new").is_none());
    assert!(store.book_record("folder:preferred").is_some());
    assert!(store.book_record("folder:competing").is_some());
}

#[test]
fn stale_adoption_hint_cannot_rekey_after_path_scope_changes() {
    let base = unique_base("prepare-stale-hint");
    let path = Path::new("C:/synthetic/stale-folder");
    let mut ui = store_at(&base);
    ui.upsert_book_record(BookRecordInput {
        book_id: "folder:old",
        title: "Stale folder",
        last_page: 2,
        last_page_name: Some("003.jpg"),
        total_pages: 8,
        path,
        reading_direction: ReadingDirection::RightToLeft,
        fit_mode: FitMode::FitPage,
        manual_zoom: None,
        view_mode: None,
        strip_offset_frac: None,
        smart_spread_phase: 0,
    })
    .unwrap();
    let background = ui.fork_for_background();
    let hint = background
        .discover_book_record_adoption("folder-v2:new", Some("folder:old"), path)
        .unwrap();

    let mut other = store_at(&base);
    other
        .upsert_page_bookmark(
            "folder:old",
            Path::new("C:/synthetic/other-copy"),
            1,
            "Other scope",
            Some("002.jpg".into()),
        )
        .unwrap();

    assert!(matches!(
        ui.prepare_book_for_open_from_hint("folder-v2:new", path, false, hint),
        Err(PrepareBookForOpenError::StaleHint)
    ));
    assert!(store_at(&base).book_record("folder:old").is_some());
    assert!(store_at(&base).book_record("folder-v2:new").is_none());
}

#[test]
fn ui_pending_competitor_invalidates_a_background_candidate_hint() {
    let base = unique_base("prepare-pending-competitor");
    let path = Path::new("C:/synthetic/pending-competitor");
    let mut ui = store_at(&base);
    ui.upsert_book_record(BookRecordInput {
        book_id: "folder:old",
        title: "Old",
        last_page: 1,
        last_page_name: None,
        total_pages: 6,
        path,
        reading_direction: ReadingDirection::RightToLeft,
        fit_mode: FitMode::FitPage,
        manual_zoom: None,
        view_mode: None,
        strip_offset_frac: None,
        smart_spread_phase: 0,
    })
    .unwrap();
    let hint = ui
        .fork_for_background()
        .discover_book_record_adoption("folder-v2:new", Some("folder:old"), path)
        .unwrap();

    assert!(ui.upsert_book_record_deferred(BookRecordInput {
        book_id: "folder:pending-competitor",
        title: "Pending competitor",
        last_page: 2,
        last_page_name: None,
        total_pages: 6,
        path,
        reading_direction: ReadingDirection::RightToLeft,
        fit_mode: FitMode::FitPage,
        manual_zoom: None,
        view_mode: None,
        strip_offset_frac: None,
        smart_spread_phase: 0,
    }));

    assert!(matches!(
        ui.prepare_book_for_open_from_hint("folder-v2:new", path, false, hint),
        Err(PrepareBookForOpenError::StaleHint)
    ));
    assert!(store_at(&base).book_record("folder:old").is_some());
    assert!(store_at(&base).book_record("folder-v2:new").is_none());
}

#[test]
fn fallback_candidate_stales_if_the_destination_becomes_pending_exact() {
    let base = unique_base("prepare-fallback-destination-pending");
    let path = Path::new("C:/synthetic/fallback-destination-pending");
    let mut ui = store_at(&base);
    ui.upsert_book_record(BookRecordInput {
        book_id: "folder:fallback",
        title: "Fallback",
        last_page: 1,
        last_page_name: None,
        total_pages: 6,
        path,
        reading_direction: ReadingDirection::RightToLeft,
        fit_mode: FitMode::FitPage,
        manual_zoom: None,
        view_mode: None,
        strip_offset_frac: None,
        smart_spread_phase: 0,
    })
    .unwrap();
    let hint = ui
        .fork_for_background()
        .discover_book_record_adoption("folder-v2:new", None, path)
        .unwrap();
    assert!(matches!(
        hint,
        BookRecordAdoptionHint::Candidate {
            preferred: false,
            ..
        }
    ));

    assert!(ui.upsert_book_record_deferred(BookRecordInput {
        book_id: "folder-v2:new",
        title: "Destination",
        last_page: 4,
        last_page_name: None,
        total_pages: 6,
        path,
        reading_direction: ReadingDirection::LeftToRight,
        fit_mode: FitMode::FitWidth,
        manual_zoom: None,
        view_mode: None,
        strip_offset_frac: None,
        smart_spread_phase: 0,
    }));

    assert!(matches!(
        ui.prepare_book_for_open_from_hint("folder-v2:new", path, false, hint),
        Err(PrepareBookForOpenError::StaleHint)
    ));
    assert!(store_at(&base).book_record("folder:fallback").is_some());
}

#[test]
fn ui_pending_candidate_invalidates_a_background_not_found_hint() {
    let base = unique_base("prepare-pending-after-not-found");
    let path = Path::new("C:/synthetic/pending-after-not-found");
    let mut ui = store_at(&base);
    let hint = ui
        .fork_for_background()
        .discover_book_record_adoption("folder-v2:new", None, path)
        .unwrap();
    assert!(matches!(hint, BookRecordAdoptionHint::NotFound { .. }));

    assert!(ui.upsert_book_record_deferred(BookRecordInput {
        book_id: "folder:pending",
        title: "Pending",
        last_page: 3,
        last_page_name: None,
        total_pages: 6,
        path,
        reading_direction: ReadingDirection::RightToLeft,
        fit_mode: FitMode::FitPage,
        manual_zoom: None,
        view_mode: None,
        strip_offset_frac: None,
        smart_spread_phase: 0,
    }));

    assert!(matches!(
        ui.prepare_book_for_open_from_hint("folder-v2:new", path, false, hint),
        Err(PrepareBookForOpenError::StaleHint)
    ));
}

#[test]
fn destination_exact_hint_ignores_a_new_old_identity_pending_candidate() {
    let base = unique_base("prepare-destination-pending-old");
    let path = Path::new("C:/synthetic/destination-pending-old");
    let mut ui = store_at(&base);
    ui.upsert_book_record(BookRecordInput {
        book_id: "folder-v2:destination",
        title: "Destination",
        last_page: 5,
        last_page_name: None,
        total_pages: 8,
        path,
        reading_direction: ReadingDirection::RightToLeft,
        fit_mode: FitMode::FitPage,
        manual_zoom: None,
        view_mode: None,
        strip_offset_frac: None,
        smart_spread_phase: 0,
    })
    .unwrap();
    let hint = ui
        .fork_for_background()
        .discover_book_record_adoption("folder-v2:destination", None, path)
        .unwrap();
    assert!(matches!(
        hint,
        BookRecordAdoptionHint::DestinationExact { .. }
    ));

    assert!(ui.upsert_book_record_deferred(BookRecordInput {
        book_id: "folder:stale-old",
        title: "Stale old",
        last_page: 1,
        last_page_name: None,
        total_pages: 8,
        path,
        reading_direction: ReadingDirection::RightToLeft,
        fit_mode: FitMode::FitPage,
        manual_zoom: None,
        view_mode: None,
        strip_offset_frac: None,
        smart_spread_phase: 0,
    }));

    let prepared = ui
        .prepare_book_for_open_from_hint("folder-v2:destination", path, false, hint)
        .unwrap();
    assert_eq!(prepared.adoption, BookRecordAdoption::NotNeeded);
    assert_eq!(prepared.reading_position.unwrap().last_page, 5);
    assert!(ui.pending_books.contains_key("folder:stale-old"));
}

#[test]
fn destination_exact_hint_stales_if_its_preferred_pending_candidate_appears() {
    let base = unique_base("prepare-destination-preferred-pending");
    let path = Path::new("C:/synthetic/destination-preferred-pending");
    let mut ui = store_at(&base);
    ui.upsert_book_record(BookRecordInput {
        book_id: "folder-v2:destination",
        title: "Destination",
        last_page: 5,
        last_page_name: None,
        total_pages: 8,
        path,
        reading_direction: ReadingDirection::RightToLeft,
        fit_mode: FitMode::FitPage,
        manual_zoom: None,
        view_mode: None,
        strip_offset_frac: None,
        smart_spread_phase: 0,
    })
    .unwrap();
    let hint = ui
        .fork_for_background()
        .discover_book_record_adoption("folder-v2:destination", Some("folder:preferred"), path)
        .unwrap();

    assert!(ui.upsert_book_record_deferred(BookRecordInput {
        book_id: "folder:preferred",
        title: "Preferred",
        last_page: 2,
        last_page_name: None,
        total_pages: 8,
        path,
        reading_direction: ReadingDirection::RightToLeft,
        fit_mode: FitMode::FitPage,
        manual_zoom: None,
        view_mode: None,
        strip_offset_frac: None,
        smart_spread_phase: 0,
    }));

    assert!(matches!(
        ui.prepare_book_for_open_from_hint("folder-v2:destination", path, false, hint),
        Err(PrepareBookForOpenError::StaleHint)
    ));
}

#[test]
fn value_only_resume_update_keeps_an_adoption_hint_valid() {
    let base = unique_base("prepare-resume-value-hint");
    let path = Path::new("C:/synthetic/resume-folder");
    let mut ui = store_at(&base);
    ui.upsert_book_record(BookRecordInput {
        book_id: "folder:old",
        title: "Resume folder",
        last_page: 1,
        last_page_name: Some("002.jpg"),
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
    let hint = ui
        .fork_for_background()
        .discover_book_record_adoption("folder-v2:new", Some("folder:old"), path)
        .unwrap();

    assert!(ui.upsert_book_record_deferred(BookRecordInput {
        book_id: "folder:old",
        title: "Resume folder",
        last_page: 9,
        last_page_name: Some("010.jpg"),
        total_pages: 12,
        path,
        reading_direction: ReadingDirection::LeftToRight,
        fit_mode: FitMode::FitWidth,
        manual_zoom: Some(1.5),
        view_mode: Some("double"),
        strip_offset_frac: None,
        smart_spread_phase: 1,
    }));

    let prepared = ui
        .prepare_book_for_open_from_hint("folder-v2:new", path, false, hint)
        .unwrap();
    assert_eq!(prepared.adoption, BookRecordAdoption::Adopted);
    assert_eq!(prepared.reading_position.unwrap().last_page, 9);
}

#[test]
fn non_scope_manual_metadata_update_keeps_an_adoption_hint_valid() {
    let base = unique_base("prepare-bookmark-value-hint");
    let path = Path::new("C:/synthetic/bookmark-folder");
    let mut ui = store_at(&base);
    ui.upsert_book_record(BookRecordInput {
        book_id: "folder:old",
        title: "Bookmark folder",
        last_page: 0,
        last_page_name: Some("001.jpg"),
        total_pages: 4,
        path,
        reading_direction: ReadingDirection::RightToLeft,
        fit_mode: FitMode::FitPage,
        manual_zoom: None,
        view_mode: None,
        strip_offset_frac: None,
        smart_spread_phase: 0,
    })
    .unwrap();
    ui.upsert_page_bookmark("folder:old", path, 1, "Before", Some("002.jpg".into()))
        .unwrap();
    let hint = ui
        .fork_for_background()
        .discover_book_record_adoption("folder-v2:new", Some("folder:old"), path)
        .unwrap();

    ui.upsert_page_bookmark("folder:old", path, 1, "After", Some("002.jpg".into()))
        .unwrap();
    let prepared = ui
        .prepare_book_for_open_from_hint("folder-v2:new", path, false, hint)
        .unwrap();

    assert_eq!(prepared.adoption, BookRecordAdoption::Adopted);
    assert_eq!(
        ui.book_record("folder-v2:new").unwrap().page_bookmarks[0].title,
        "After"
    );
}

#[test]
fn odd_catalog_revision_is_recovered_before_the_next_writer() {
    let base = unique_base("catalog-revision-recovery");
    let path = Path::new("C:/synthetic/recovery.cbz");
    fs::create_dir_all(base.join("books")).unwrap();
    fs::write(base.join("books").join(".catalog-revision"), "7").unwrap();
    let mut store = store_at(&base);

    store
        .upsert_book_record(BookRecordInput {
            book_id: "zip:recovered",
            title: "Recovered",
            last_page: 1,
            last_page_name: Some("002.jpg"),
            total_pages: 3,
            path,
            reading_direction: ReadingDirection::RightToLeft,
            fit_mode: FitMode::FitPage,
            manual_zoom: None,
            view_mode: None,
            strip_offset_frac: None,
            smart_spread_phase: 0,
        })
        .unwrap();

    let revision: u64 = fs::read_to_string(base.join("books").join(".catalog-revision"))
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(revision % 2, 0);
    assert!(store.book_record("zip:recovered").is_some());
}

#[test]
fn background_discovery_recovers_an_abandoned_odd_revision() {
    let base = unique_base("catalog-discovery-recovery");
    fs::create_dir_all(base.join("books")).unwrap();
    fs::write(base.join("books").join(".catalog-revision"), "1").unwrap();
    let store = store_at(&base);

    let hint = store
        .fork_for_background()
        .discover_book_record_adoption("folder-v2:missing", None, Path::new("C:/synthetic/missing"))
        .unwrap();

    let revision: u64 = fs::read_to_string(base.join("books").join(".catalog-revision"))
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(revision, 2);
    assert!(matches!(
        hint,
        BookRecordAdoptionHint::NotFound { revision: 2 }
    ));
}

#[test]
fn discovery_hint_variants_carry_the_stable_revision() {
    let store = test_store("hint-revisions");
    let hint = store
        .fork_for_background()
        .discover_book_record_adoption("folder-v2:missing", None, Path::new("C:/synthetic/missing"))
        .unwrap();
    assert!(matches!(
        hint,
        BookRecordAdoptionHint::NotFound { revision: 0 }
    ));
}
