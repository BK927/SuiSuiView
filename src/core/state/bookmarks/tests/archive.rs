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
