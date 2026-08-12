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
    assert_eq!(
        store
            .adopt_record_for_path("folder:after", Some("folder:before"), folder)
            .unwrap(),
        BookRecordAdoption::Adopted
    );

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

    assert_eq!(
        first
            .adopt_record_for_path("folder:after", None, folder)
            .unwrap(),
        BookRecordAdoption::Adopted
    );
    let reopened = store_at(&base);
    let adopted = reopened.book_record("folder:after").unwrap();
    assert_eq!(adopted.page_bookmarks.len(), 1);
    assert_eq!(adopted.page_bookmarks[0].title, "Other window");
    assert!(reopened.book_record("folder:before").is_none());
}

#[test]
fn stale_cross_process_resume_write_cannot_recreate_a_retired_identity() {
    let base = unique_base("adopt-stale-resume-redirect");
    let folder = Path::new("C:/books/edited-folder");
    let mut migrator = store_at(&base);
    migrator
        .upsert_book_record(BookRecordInput {
            book_id: "folder:before",
            title: "Edited Folder",
            last_page: 2,
            last_page_name: Some("003.jpg"),
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
    migrator
        .upsert_page_bookmark(
            "folder:before",
            folder,
            2,
            "Manual mark",
            Some("003.jpg".into()),
        )
        .unwrap();

    let mut stale = store_at(&base);
    assert!(stale.upsert_book_record_deferred(BookRecordInput {
        book_id: "folder:before",
        title: "Edited Folder",
        last_page: 7,
        last_page_name: Some("008.jpg"),
        total_pages: 10,
        path: folder,
        reading_direction: ReadingDirection::RightToLeft,
        fit_mode: FitMode::FitWidth,
        manual_zoom: None,
        view_mode: None,
        strip_offset_frac: None,
        smart_spread_phase: 0,
    }));

    assert_eq!(
        migrator
            .adopt_record_for_path("folder-v2:after", None, folder)
            .unwrap(),
        BookRecordAdoption::Adopted
    );
    stale.flush().unwrap();

    let reopened = store_at(&base);
    assert!(reopened.book_record("folder:before").is_none());
    let adopted = reopened.book_record("folder-v2:after").unwrap();
    assert_eq!(adopted.last_page, 2);
    assert_eq!(adopted.last_page_name.as_deref(), Some("003.jpg"));
    assert_eq!(adopted.page_bookmarks.len(), 1);
    assert!(base.join("books").join("folder_before.redirect").is_file());
}

#[test]
fn stale_cross_process_manual_write_reports_the_retired_record_instead_of_recreating_it() {
    let base = unique_base("adopt-stale-manual-redirect");
    let folder = Path::new("C:/books/edited-folder");
    let mut migrator = store_at(&base);
    migrator
        .upsert_book_record(BookRecordInput {
            book_id: "folder:before",
            title: "Edited Folder",
            last_page: 2,
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
    let mut stale = store_at(&base);
    assert!(stale.book_record("folder:before").is_some());

    assert_eq!(
        migrator
            .adopt_record_for_path("folder-v2:after", None, folder)
            .unwrap(),
        BookRecordAdoption::Adopted
    );
    let error = stale
        .upsert_page_bookmark("folder:before", folder, 5, "Stale mark", None)
        .unwrap_err();

    assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
    let reopened = store_at(&base);
    assert!(reopened.book_record("folder:before").is_none());
    assert!(reopened.page_bookmarks("folder-v2:after").is_empty());
}

#[test]
fn returning_to_an_old_content_identity_reactivates_it_without_a_redirect_cycle() {
    let base = unique_base("adopt-content-id-round-trip");
    let folder = Path::new("C:/books/edited-folder");
    let mut store = store_at(&base);
    store
        .upsert_book_record(BookRecordInput {
            book_id: "folder-v2:a",
            title: "Folder A",
            last_page: 2,
            last_page_name: Some("003.jpg"),
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
    assert_eq!(
        store
            .adopt_record_for_path("folder-v2:b", None, folder)
            .unwrap(),
        BookRecordAdoption::Adopted
    );
    assert_eq!(
        store
            .adopt_record_for_path("folder-v2:a", None, folder)
            .unwrap(),
        BookRecordAdoption::Adopted
    );

    store
        .upsert_book_record(BookRecordInput {
            book_id: "folder-v2:a",
            title: "Folder A again",
            last_page: 6,
            last_page_name: Some("007.jpg"),
            total_pages: 10,
            path: folder,
            reading_direction: ReadingDirection::RightToLeft,
            fit_mode: FitMode::FitWidth,
            manual_zoom: None,
            view_mode: None,
            strip_offset_frac: None,
            smart_spread_phase: 0,
        })
        .unwrap();
    store
        .upsert_page_bookmark("folder-v2:a", folder, 6, "Round trip mark", None)
        .unwrap();

    let reopened = store_at(&base);
    let active = reopened.book_record("folder-v2:a").unwrap();
    assert_eq!(active.last_page, 6);
    assert_eq!(active.page_bookmarks.len(), 1);
    assert!(!base.join("books").join("folder-v2_a.redirect").exists());
    assert!(base.join("books").join("folder-v2_b.redirect").is_file());
}

#[test]
fn interrupted_identity_migration_is_completed_from_its_journal() {
    let base = unique_base("adopt-journal-recovery");
    let folder = Path::new("C:/books/folder");
    let mut store = store_at(&base);
    for (book_id, page) in [("folder-v2:old", 2), ("folder-v2:new", 6)] {
        store
            .upsert_book_record(BookRecordInput {
                book_id,
                title: "Folder",
                last_page: page,
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
    }
    let intended = store.book_record("folder-v2:new").unwrap();
    fs::write(
        base.join("books").join(".identity-migration.json"),
        serde_json::to_string(&serde_json::json!({
            "old_book_id": "folder-v2:old",
            "new_book_id": "folder-v2:new",
            "destination": intended,
        }))
        .unwrap(),
    )
    .unwrap();

    let mut recovered = store_at(&base);
    assert_eq!(
        recovered
            .adopt_record_for_path("folder-v2:new", None, folder)
            .unwrap(),
        BookRecordAdoption::NotNeeded
    );

    let reopened = store_at(&base);
    assert!(reopened.book_record("folder-v2:old").is_none());
    assert!(reopened.book_record("folder-v2:new").is_some());
    assert!(!base.join("books").join(".identity-migration.json").exists());
    assert!(base.join("books").join("folder-v2_old.redirect").is_file());
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
        .adopt_record_for_path("folder:after", Some("folder:before"), folder)
        .expect_err("a malformed source record must stop the re-key");
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    assert!(store.book_record("folder:after").is_none());
    assert_eq!(fs::read(old_record_path).unwrap(), corrupt);
}

#[test]
fn adopt_record_merges_into_a_destination_for_another_path() {
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
    second
        .upsert_page_bookmark(
            "folder:after",
            destination_path,
            6,
            "Destination manual mark",
            None,
        )
        .unwrap();

    assert_eq!(
        first
            .adopt_record_for_path("folder:after", None, old_path)
            .unwrap(),
        BookRecordAdoption::Adopted
    );
    let reopened = store_at(&base);
    let destination = reopened.book_record("folder:after").unwrap();
    assert_eq!(destination.title, "Destination");
    assert!(destination
        .path_positions
        .contains_key(old_path.to_string_lossy().as_ref()));
    assert!(destination
        .path_positions
        .contains_key(destination_path.to_string_lossy().as_ref()));
    assert_eq!(destination.page_bookmarks.len(), 1);
    assert_eq!(
        destination.page_bookmarks[0].title,
        "Destination manual mark"
    );
    assert!(reopened.book_record("folder:before").is_none());
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

    assert_eq!(
        store
            .adopt_record_for_path("folder:intact", None, folder)
            .unwrap(),
        BookRecordAdoption::NotNeeded
    );
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

    assert_eq!(
        store
            .adopt_record_for_path("folder:new", None, Path::new("C:/books/unrelated"),)
            .unwrap(),
        BookRecordAdoption::NotNeeded
    );
    assert!(store.book_record("folder:new").is_none());
    assert!(store.book_record("folder:elsewhere").is_some());
}

#[test]
fn adopt_record_does_not_trust_a_moved_legacy_fingerprint_without_path_proof() {
    let mut store = test_store("adopt-moved-legacy");
    let old_path = Path::new("C:/books/old-location");
    let moved_path = Path::new("D:/books/new-location");
    store
        .upsert_book_record(BookRecordInput {
            book_id: "folder:legacy",
            title: "Legacy Folder",
            last_page: 4,
            last_page_name: Some("005.jpg"),
            total_pages: 10,
            path: old_path,
            reading_direction: ReadingDirection::LeftToRight,
            fit_mode: FitMode::FitPage,
            manual_zoom: None,
            view_mode: None,
            strip_offset_frac: None,
            smart_spread_phase: 0,
        })
        .unwrap();

    assert_eq!(
        store
            .adopt_record_for_path("folder-v2:new", Some("folder:legacy"), moved_path)
            .unwrap(),
        BookRecordAdoption::NotNeeded
    );
    assert!(store.book_record("folder:legacy").is_some());
    assert!(store.book_record("folder-v2:new").is_none());
}

#[test]
fn adopt_record_refuses_to_move_other_copy_scopes() {
    let mut store = test_store("adopt-multiple-scopes");
    let first_path = Path::new("C:/books/copy-a");
    let second_path = Path::new("D:/books/copy-b");
    for (path, page) in [(first_path, 2), (second_path, 6)] {
        store
            .upsert_book_record(BookRecordInput {
                book_id: "folder:shared",
                title: "Copied Folder",
                last_page: page,
                last_page_name: None,
                total_pages: 10,
                path,
                reading_direction: ReadingDirection::LeftToRight,
                fit_mode: FitMode::FitPage,
                manual_zoom: None,
                view_mode: None,
                strip_offset_frac: None,
                smart_spread_phase: 0,
            })
            .unwrap();
    }
    store
        .upsert_page_bookmark("folder:shared", second_path, 6, "Other copy mark", None)
        .unwrap();

    assert_eq!(
        store
            .adopt_record_for_path("folder-v2:edited-a", None, first_path)
            .unwrap(),
        BookRecordAdoption::Conflict
    );
    let source = store.book_record("folder:shared").unwrap();
    assert_eq!(source.path_positions.len(), 2);
    assert_eq!(source.page_bookmarks.len(), 1);
    assert!(store.book_record("folder-v2:edited-a").is_none());
}

#[test]
fn adopt_record_keeps_both_records_when_current_path_bookmarks_conflict() {
    let mut store = test_store("adopt-manual-conflict");
    let folder = Path::new("C:/books/folder");
    for (book_id, page) in [("folder:legacy", 2), ("folder-v2:new", 4)] {
        store
            .upsert_book_record(BookRecordInput {
                book_id,
                title: "Folder",
                last_page: page,
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
    }
    store
        .upsert_page_bookmark("folder:legacy", folder, 2, "Legacy mark", None)
        .unwrap();

    assert_eq!(
        store
            .adopt_record_for_path("folder-v2:new", Some("folder:legacy"), folder)
            .unwrap(),
        BookRecordAdoption::Conflict
    );
    assert_eq!(store.page_bookmarks("folder:legacy").len(), 1);
    assert!(store.page_bookmarks("folder-v2:new").is_empty());
}

#[test]
fn adopt_record_does_not_pick_the_newest_of_ambiguous_candidates() {
    let mut store = test_store("adopt-ambiguous");
    let folder = Path::new("C:/books/folder");
    for (book_id, page) in [("folder:first", 1), ("folder:second", 7)] {
        store
            .upsert_book_record(BookRecordInput {
                book_id,
                title: "Folder",
                last_page: page,
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
    }

    assert_eq!(
        store
            .adopt_record_for_path("folder-v2:new", None, folder)
            .unwrap(),
        BookRecordAdoption::Ambiguous
    );
    assert!(store.book_record("folder:first").is_some());
    assert!(store.book_record("folder:second").is_some());
    assert!(store.book_record("folder-v2:new").is_none());
}

#[test]
fn manual_only_adoption_does_not_invent_an_automatic_resume_position() {
    let mut store = test_store("adopt-manual-domain-only");
    let folder = Path::new("C:/books/folder");
    store
        .upsert_book_record(BookRecordInput {
            book_id: "folder:legacy",
            title: "Folder",
            last_page: 8,
            last_page_name: Some("009.jpg"),
            total_pages: 10,
            path: folder,
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
            "folder:legacy",
            folder,
            8,
            "Manual mark",
            Some("009.jpg".into()),
        )
        .unwrap();
    store
        .mutate_book_record("folder:legacy", |record| {
            record.path_positions.clear();
            ((), true)
        })
        .unwrap();

    assert_eq!(
        store
            .adopt_record_for_path("folder-v2:new", Some("folder:legacy"), folder)
            .unwrap(),
        BookRecordAdoption::Adopted
    );
    let adopted = store.book_record("folder-v2:new").unwrap();
    assert_eq!(adopted.page_bookmarks.len(), 1);
    assert_eq!(adopted.last_page, 0);
    assert_eq!(adopted.last_page_name, None);
    assert_eq!(adopted.reading_direction, ReadingDirection::RightToLeft);
    assert_eq!(adopted.fit_mode, FitMode::FitPage);
    assert_eq!(adopted.manual_zoom, None);
    assert_eq!(adopted.view_mode, None);
    assert_eq!(adopted.smart_spread_phase, 0);
}

#[test]
fn manual_bookmark_shell_does_not_create_an_automatic_resume_position() {
    let mut store = test_store("manual-bookmark-shell");
    let folder = Path::new("C:/books/folder");

    store
        .ensure_book_record_shell("folder-v2:manual", "Folder", 10)
        .unwrap();
    store
        .upsert_page_bookmark(
            "folder-v2:manual",
            folder,
            7,
            "Manual mark",
            Some("008.jpg".into()),
        )
        .unwrap();

    let record = store.book_record("folder-v2:manual").unwrap();
    assert!(record.known_paths.is_empty());
    assert!(record.path_positions.is_empty());
    assert_eq!(record.page_bookmarks.len(), 1);
    assert!(store
        .reading_position("folder-v2:manual", folder, true)
        .is_none());
    assert!(store
        .reading_position("folder-v2:manual", folder, false)
        .is_none());
}

#[test]
fn metadata_shell_flushes_an_existing_deferred_automatic_position() {
    let base = unique_base("metadata-shell-pending-resume");
    let mut store = store_at(&base);
    let folder = Path::new("C:/books/folder");
    assert!(store.upsert_book_record_deferred(BookRecordInput {
        book_id: "folder-v2:pending",
        title: "Folder",
        last_page: 5,
        last_page_name: Some("006.jpg"),
        total_pages: 10,
        path: folder,
        reading_direction: ReadingDirection::LeftToRight,
        fit_mode: FitMode::FitWidth,
        manual_zoom: None,
        view_mode: None,
        strip_offset_frac: None,
        smart_spread_phase: 0,
    }));

    store
        .ensure_book_record_shell("folder-v2:pending", "Folder", 10)
        .unwrap();

    let reopened = store_at(&base);
    let position = reopened
        .reading_position("folder-v2:pending", folder, false)
        .unwrap();
    assert_eq!(position.last_page, 5);
    assert_eq!(position.last_page_name.as_deref(), Some("006.jpg"));
}
