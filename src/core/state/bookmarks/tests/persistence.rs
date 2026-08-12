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
