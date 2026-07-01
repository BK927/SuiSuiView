use super::{now_unix_seconds, FitMode, ReadingDirection, StateStore};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BookRecord {
    pub book_id: String,
    pub title: String,
    pub last_page: usize,
    #[serde(default)]
    pub last_page_name: Option<String>,
    pub total_pages: usize,
    pub known_paths: Vec<String>,
    pub reading_direction: ReadingDirection,
    pub fit_mode: FitMode,
    #[serde(default)]
    pub manual_zoom: Option<f32>,
    #[serde(default)]
    pub path_positions: BTreeMap<String, ReadingPosition>,
    #[serde(default)]
    pub page_bookmarks: Vec<PageBookmark>,
    pub updated_at: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReadingPosition {
    pub last_page: usize,
    #[serde(default)]
    pub last_page_name: Option<String>,
    pub reading_direction: ReadingDirection,
    pub fit_mode: FitMode,
    #[serde(default)]
    pub manual_zoom: Option<f32>,
    pub updated_at: u64,
}

impl ReadingPosition {
    pub(super) fn from_input(input: &BookRecordInput<'_>, now: u64) -> Self {
        Self {
            last_page: input.last_page.min(input.total_pages.saturating_sub(1)),
            last_page_name: input.last_page_name.map(ToOwned::to_owned),
            reading_direction: input.reading_direction,
            fit_mode: input.fit_mode,
            manual_zoom: input.manual_zoom,
            updated_at: now,
        }
    }

    pub(super) fn from_record(record: &BookRecord) -> Self {
        Self {
            last_page: record.last_page,
            last_page_name: record.last_page_name.clone(),
            reading_direction: record.reading_direction,
            fit_mode: record.fit_mode,
            manual_zoom: record.manual_zoom,
            updated_at: record.updated_at,
        }
    }

    pub(super) fn matches_input(&self, input: &BookRecordInput<'_>) -> bool {
        self.last_page == input.last_page.min(input.total_pages.saturating_sub(1))
            && self.last_page_name.as_deref() == input.last_page_name
            && self.reading_direction == input.reading_direction
            && self.fit_mode == input.fit_mode
            && self.manual_zoom == input.manual_zoom
    }
}

pub struct BookRecordInput<'a> {
    pub book_id: &'a str,
    pub title: &'a str,
    pub last_page: usize,
    pub last_page_name: Option<&'a str>,
    pub total_pages: usize,
    pub path: &'a Path,
    pub reading_direction: ReadingDirection,
    pub fit_mode: FitMode,
    pub manual_zoom: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageBookmark {
    pub page: usize,
    #[serde(default)]
    pub source_path: String,
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

pub(super) fn path_key(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

impl StateStore {
    pub fn recent_books(&self, limit: usize) -> Vec<BookRecord> {
        let mut records = self.load_all_book_records();
        records.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
        records.truncate(limit);
        records
    }

    pub fn page_bookmarks(&self, book_id: &str) -> Vec<PageBookmark> {
        self.read_book_record(book_id)
            .map(|record| record.page_bookmarks)
            .unwrap_or_default()
    }

    pub fn all_page_bookmarks(&self) -> Vec<PageBookmarkEntry> {
        self.load_all_book_records()
            .iter()
            .flat_map(page_bookmark_entries_for_book)
            .collect()
    }

    pub fn page_bookmark_entries(
        &self,
        book_id: &str,
        source_path: &Path,
    ) -> Vec<PageBookmarkEntry> {
        self.read_book_record(book_id)
            .map(|record| page_bookmark_entries_for_path(&record, path_key(source_path).as_str()))
            .unwrap_or_default()
    }

    pub fn all_page_bookmark_count(&self) -> usize {
        self.load_all_book_records()
            .iter()
            .map(|record| {
                record
                    .page_bookmarks
                    .iter()
                    .filter(|bookmark| !bookmark.source_path.is_empty())
                    .count()
            })
            .sum()
    }

    pub fn has_page_bookmark(&self, book_id: &str, source_path: &Path, page: usize) -> bool {
        let source_path = path_key(source_path);
        self.read_book_record(book_id).is_some_and(|record| {
            record
                .page_bookmarks
                .iter()
                .any(|bookmark| bookmark.source_path == source_path && bookmark.page == page)
        })
    }

    pub fn upsert_page_bookmark(
        &mut self,
        book_id: &str,
        source_path: &Path,
        page: usize,
        title: impl Into<String>,
        page_name: Option<String>,
    ) {
        let now = now_unix_seconds();
        let title = title.into();
        let source_path = path_key(source_path);
        let Some(mut record) = self.read_book_record(book_id) else {
            return;
        };

        if let Some(existing) = record
            .page_bookmarks
            .iter_mut()
            .find(|bookmark| bookmark.source_path == source_path && bookmark.page == page)
        {
            existing.title = title;
            existing.page_name = page_name;
            existing.updated_at = now;
        } else {
            record.page_bookmarks.push(PageBookmark {
                page,
                source_path,
                title,
                page_name,
                pinned: false,
                created_at: now,
                updated_at: now,
            });
        }
        record.page_bookmarks.sort_by(page_bookmark_order);
        record.updated_at = now;
        let _ = self.write_book_record(&record);
    }

    pub fn remove_page_bookmark(&mut self, book_id: &str, source_path: &Path, page: usize) {
        let Some(mut record) = self.read_book_record(book_id) else {
            return;
        };
        let source_path = path_key(source_path);
        let previous_len = record.page_bookmarks.len();
        record.page_bookmarks.retain(|page_bookmark| {
            page_bookmark.source_path != source_path || page_bookmark.page != page
        });
        if record.page_bookmarks.len() == previous_len {
            return;
        }
        record.updated_at = now_unix_seconds();
        let _ = self.write_book_record(&record);
    }

    pub fn clear_page_bookmarks(&mut self, book_id: &str, source_path: &Path) -> usize {
        let Some(mut record) = self.read_book_record(book_id) else {
            return 0;
        };
        let source_path = path_key(source_path);
        let previous_len = record.page_bookmarks.len();
        record
            .page_bookmarks
            .retain(|page_bookmark| page_bookmark.source_path != source_path);
        let removed = previous_len - record.page_bookmarks.len();
        if removed == 0 {
            return 0;
        }
        record.updated_at = now_unix_seconds();
        let _ = self.write_book_record(&record);
        removed
    }

    pub fn clear_all_page_bookmarks(&mut self) -> usize {
        self.flush_pending_book();
        let mut removed = 0;
        let now = now_unix_seconds();
        for mut record in self.load_all_book_records() {
            let previous_len = record.page_bookmarks.len();
            record
                .page_bookmarks
                .retain(|page_bookmark| page_bookmark.source_path.is_empty());
            let count = previous_len - record.page_bookmarks.len();
            if count == 0 {
                continue;
            }
            removed += count;
            record.updated_at = now;
            let _ = self.write_book_record(&record);
        }
        removed
    }
}

fn page_bookmark_entries_for_book(book: &BookRecord) -> Vec<PageBookmarkEntry> {
    book.page_bookmarks
        .iter()
        .filter(|bookmark| !bookmark.source_path.is_empty())
        .map(|bookmark| PageBookmarkEntry {
            book_id: book.book_id.clone(),
            book_title: book.title.clone(),
            known_path: Some(bookmark.source_path.clone()),
            bookmark: bookmark.clone(),
        })
        .collect()
}

fn page_bookmark_entries_for_path(book: &BookRecord, source_path: &str) -> Vec<PageBookmarkEntry> {
    book.page_bookmarks
        .iter()
        .filter(|bookmark| bookmark.source_path == source_path)
        .map(|bookmark| PageBookmarkEntry {
            book_id: book.book_id.clone(),
            book_title: book.title.clone(),
            known_path: Some(bookmark.source_path.clone()),
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
        assert!(!state.settings.show_status_bar);
        assert!(state.settings.resume_by_file_identity);
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
}
