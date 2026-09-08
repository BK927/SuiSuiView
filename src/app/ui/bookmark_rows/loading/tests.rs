use super::*;
use std::time::{Duration, Instant};

#[test]
fn changing_search_and_active_book_during_initial_read_reuses_catalog() {
    let ctx = egui::Context::default();
    let mut cache = BookmarkRowsCache::default();
    let scope = scope_key(BookmarkFilter::All, Some("book-1"), Some("book.zip"));
    cache.loading_scope = Some(scope.clone());
    let (tx, rx) = crossbeam_channel::bounded(1);
    cache.loading.request(&ctx, move || {
        rx.recv().unwrap();
        Ok(LoadedCatalog { scope, entries: Arc::new(super::super::tests::sample_entries()) })
    });
    for search in ["c", "co", "cover"] {
        cache.refresh_async(&ctx, || panic!("must not reread catalog"), BookmarkFilter::All, Some("book-2"), Some("other.zip"), search);
        assert!(cache.is_loading());
    }
    tx.send(()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while cache.is_loading() {
        cache.refresh_async(&ctx, || panic!("must reuse completed catalog"), BookmarkFilter::All, Some("book-2"), Some("other.zip"), "cover");
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert_eq!(cache.len(), 1);
    assert_eq!(cache.row(0).unwrap().bookmark.page, 0);
    assert_eq!(cache.scope_count(BookmarkFilter::All, None, None), Some(3));
    cache.clear();
    assert!(cache.catalog.is_none());
    assert!(cache.loading_scope.is_none());
}
