use super::{now_unix_seconds, Bookmark, StateStore};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageBookmark {
    pub page: usize,
    pub title: String,
    #[serde(default)]
    pub page_name: Option<String>,
    pub pinned: bool,
    pub created_at: u64,
    pub updated_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageBookmarkEntry {
    pub book_id: String,
    pub book_title: String,
    pub known_path: Option<String>,
    pub bookmark: PageBookmark,
}

impl StateStore {
    pub fn recent_books(&self, limit: usize) -> Vec<Bookmark> {
        let mut books: Vec<_> = self.state.books.values().collect();
        books.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
        books.into_iter().take(limit).cloned().collect()
    }

    pub fn page_bookmarks(&self, book_id: &str) -> &[PageBookmark] {
        self.state
            .books
            .get(book_id)
            .map(|bookmark| bookmark.page_bookmarks.as_slice())
            .unwrap_or_default()
    }

    pub fn all_page_bookmarks(&self) -> Vec<PageBookmarkEntry> {
        self.state
            .books
            .values()
            .flat_map(page_bookmark_entries_for_book)
            .collect()
    }

    pub fn page_bookmark_entries(&self, book_id: &str) -> Vec<PageBookmarkEntry> {
        self.state
            .books
            .get(book_id)
            .map(page_bookmark_entries_for_book)
            .unwrap_or_default()
    }

    pub fn all_page_bookmark_count(&self) -> usize {
        self.state
            .books
            .values()
            .map(|book| book.page_bookmarks.len())
            .sum()
    }

    pub fn has_page_bookmark(&self, book_id: &str, page: usize) -> bool {
        self.page_bookmarks(book_id)
            .iter()
            .any(|bookmark| bookmark.page == page)
    }

    pub fn upsert_page_bookmark(
        &mut self,
        book_id: &str,
        page: usize,
        title: impl Into<String>,
        page_name: Option<String>,
    ) {
        self.state.version = 4;
        let now = now_unix_seconds();
        let title = title.into();
        let entry = self.state.books.get_mut(book_id);
        let Some(bookmark) = entry else {
            return;
        };

        if let Some(existing) = bookmark
            .page_bookmarks
            .iter_mut()
            .find(|bookmark| bookmark.page == page)
        {
            existing.title = title;
            existing.page_name = page_name;
            existing.updated_at = now;
        } else {
            bookmark.page_bookmarks.push(PageBookmark {
                page,
                title,
                page_name,
                pinned: false,
                created_at: now,
                updated_at: now,
            });
        }
        bookmark.page_bookmarks.sort_by(page_bookmark_order);
        bookmark.updated_at = now;
        let _ = self.save();
    }

    pub fn remove_page_bookmark(&mut self, book_id: &str, page: usize) {
        self.state.version = 4;
        let Some(bookmark) = self.state.books.get_mut(book_id) else {
            return;
        };
        let previous_len = bookmark.page_bookmarks.len();
        bookmark
            .page_bookmarks
            .retain(|page_bookmark| page_bookmark.page != page);
        if bookmark.page_bookmarks.len() == previous_len {
            return;
        }
        bookmark.updated_at = now_unix_seconds();
        let _ = self.save();
    }

    pub fn clear_page_bookmarks(&mut self, book_id: &str) -> usize {
        let Some(bookmark) = self.state.books.get_mut(book_id) else {
            return 0;
        };
        let removed = bookmark.page_bookmarks.len();
        if removed == 0 {
            return 0;
        }
        self.state.version = 4;
        bookmark.page_bookmarks.clear();
        bookmark.updated_at = now_unix_seconds();
        let _ = self.save();
        removed
    }

    pub fn clear_all_page_bookmarks(&mut self) -> usize {
        let mut removed = 0;
        let now = now_unix_seconds();
        for bookmark in self.state.books.values_mut() {
            let count = bookmark.page_bookmarks.len();
            if count == 0 {
                continue;
            }
            removed += count;
            bookmark.page_bookmarks.clear();
            bookmark.updated_at = now;
        }
        if removed > 0 {
            self.state.version = 4;
            let _ = self.save();
        }
        removed
    }
}

fn page_bookmark_entries_for_book(book: &Bookmark) -> Vec<PageBookmarkEntry> {
    let known_path = book.known_paths.last().cloned();
    book.page_bookmarks
        .iter()
        .map(|bookmark| PageBookmarkEntry {
            book_id: book.book_id.clone(),
            book_title: book.title.clone(),
            known_path: known_path.clone(),
            bookmark: bookmark.clone(),
        })
        .collect()
}

fn page_bookmark_order(left: &PageBookmark, right: &PageBookmark) -> std::cmp::Ordering {
    right
        .pinned
        .cmp(&left.pinned)
        .then_with(|| right.updated_at.cmp(&left.updated_at))
        .then_with(|| left.page.cmp(&right.page))
}

#[cfg(test)]
mod tests {
    use super::super::{
        AppSettings, BookmarkInput, FitMode, PersistedState, ReadingDirection, StateStore,
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
        assert!(!state.settings.show_status_bar);
    }

    #[test]
    fn page_bookmarks_add_and_remove() {
        let mut store = test_store("page-bookmarks");
        store.upsert_bookmark(BookmarkInput {
            book_id: "book-1",
            title: "Book One",
            last_page: 0,
            last_page_name: None,
            total_pages: 20,
            path: Path::new("C:/books/book-1"),
            reading_direction: ReadingDirection::RightToLeft,
            fit_mode: FitMode::FitPage,
            manual_zoom: None,
        });

        store.upsert_page_bookmark("book-1", 4, "Middle", Some("page-005.jpg".to_owned()));
        store.upsert_page_bookmark("book-1", 1, "Start", Some("page-002.jpg".to_owned()));

        let bookmarks = store.page_bookmarks("book-1");
        assert_eq!(bookmarks[0].page, 1);
        assert_eq!(bookmarks[1].page, 4);
        assert_eq!(bookmarks[1].page_name.as_deref(), Some("page-005.jpg"));

        store.remove_page_bookmark("book-1", 4);
        assert!(!store.has_page_bookmark("book-1", 4));
        assert!(store.has_page_bookmark("book-1", 1));

        assert_eq!(store.clear_page_bookmarks("book-1"), 1);
        assert!(store.page_bookmarks("book-1").is_empty());
        assert_eq!(store.clear_page_bookmarks("book-1"), 0);
    }

    #[test]
    fn all_page_bookmarks_and_clear_all_keep_book_records() {
        let mut store = test_store("all-page-bookmarks");
        for (book_id, path) in [
            ("book-1", "C:/books/book-1"),
            ("book-2", "C:/books/book-2.cbz"),
        ] {
            store.upsert_bookmark(BookmarkInput {
                book_id,
                title: book_id,
                last_page: 0,
                last_page_name: None,
                total_pages: 20,
                path: Path::new(path),
                reading_direction: ReadingDirection::RightToLeft,
                fit_mode: FitMode::FitPage,
                manual_zoom: None,
            });
        }
        store.upsert_page_bookmark("book-1", 0, "Cover", Some("cover.png".to_owned()));
        store.upsert_page_bookmark("book-2", 3, "Page", Some("chapter/page.jpg".to_owned()));

        let entries = store.all_page_bookmarks();
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().any(|entry| entry.book_id == "book-1"));
        assert_eq!(store.clear_all_page_bookmarks(), 2);
        assert!(store.all_page_bookmarks().is_empty());
        assert!(store.bookmark("book-1").is_some());
        assert_eq!(store.clear_all_page_bookmarks(), 0);
    }

    #[test]
    fn pruning_auto_bookmarks_preserves_manual_page_bookmarks() {
        let mut store = test_store("prune-auto-bookmarks");
        for index in 0..3 {
            let book_id = format!("book-{index}");
            store.upsert_bookmark(BookmarkInput {
                book_id: &book_id,
                title: &book_id,
                last_page: index,
                last_page_name: None,
                total_pages: 10,
                path: Path::new("C:/books/book.zip"),
                reading_direction: ReadingDirection::LeftToRight,
                fit_mode: FitMode::FitPage,
                manual_zoom: None,
            });
        }
        store.upsert_page_bookmark("book-0", 2, "manual", Some("002.png".to_owned()));

        let removed = store.prune_auto_bookmarks(1);

        assert_eq!(removed, 2);
        assert!(store.bookmark("book-0").is_some());
        assert_eq!(store.all_page_bookmark_count(), 1);
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
    fn settings_default_uses_slide_fade_transition() {
        assert!(AppSettings::default().transition_effect);
        assert_eq!(
            AppSettings::default().page_transition_style,
            super::super::PageTransitionStyle::SlideFade
        );
    }

    fn test_store(name: &str) -> StateStore {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        StateStore {
            path: std::env::temp_dir()
                .join("suisuiview-tests")
                .join(format!("{name}-{stamp}.json")),
            state: PersistedState::default(),
        }
    }
}
