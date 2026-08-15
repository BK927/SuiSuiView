use super::path_labels;
use crate::core::i18n::I18n;
use crate::core::state::{PageBookmark, PageBookmarkEntry};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub(in crate::app) enum BookmarkFilter {
    #[default]
    All,
    ThisBook,
}

impl BookmarkFilter {
    pub(in crate::app) fn label_i18n(self, i18n: I18n) -> String {
        match self {
            Self::All => i18n.text("bookmark.tab_all"),
            Self::ThisBook => i18n.text("bookmark.tab_this_book"),
        }
    }
}

#[derive(Debug, Clone)]
pub(in crate::app) struct BookmarkRow {
    pub(in crate::app) book_id: String,
    pub(in crate::app) known_path: Option<String>,
    pub(in crate::app) bookmark: PageBookmark,
    pub(in crate::app) display_name: String,
    search_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BookmarkRowsKey {
    filter: BookmarkFilter,
    book_id: Option<String>,
    source_path: Option<String>,
    query: String,
}

/// Identity of a delete scope. Unlike [`BookmarkRowsKey`] this carries no
/// query: the scope count answers "how many bookmarks would `Delete all`
/// remove", which the search box narrows the visible rows but not the scope.
#[derive(Debug, Clone, PartialEq, Eq)]
struct BookmarkScopeKey {
    filter: BookmarkFilter,
    book_id: Option<String>,
    source_path: Option<String>,
}

#[derive(Debug, Default)]
pub(in crate::app) struct BookmarkRowsCache {
    key: Option<BookmarkRowsKey>,
    rows: Vec<BookmarkRow>,
    scope_count: Option<(BookmarkScopeKey, usize)>,
}

impl BookmarkRowsCache {
    pub(in crate::app) fn clear(&mut self) {
        self.key = None;
        self.rows.clear();
        self.scope_count = None;
    }

    /// Cached count for this scope, if it was measured since the last mutation.
    ///
    /// The popover header and the delete dialog both want this number every
    /// frame, and measuring it walks every book record. Caching it here reuses
    /// the row cache's invalidation: every site that changes bookmarks already
    /// calls [`Self::clear`].
    pub(in crate::app) fn scope_count(
        &self,
        filter: BookmarkFilter,
        book_id: Option<&str>,
        source_path: Option<&str>,
    ) -> Option<usize> {
        let (key, count) = self.scope_count.as_ref()?;
        (key == &scope_key(filter, book_id, source_path)).then_some(*count)
    }

    pub(in crate::app) fn set_scope_count(
        &mut self,
        filter: BookmarkFilter,
        book_id: Option<&str>,
        source_path: Option<&str>,
        count: usize,
    ) {
        self.scope_count = Some((scope_key(filter, book_id, source_path), count));
    }

    pub(in crate::app) fn needs_refresh(
        &self,
        filter: BookmarkFilter,
        book_id: Option<&str>,
        source_path: Option<&str>,
        query: &str,
    ) -> bool {
        self.key.as_ref()
            != Some(&BookmarkRowsKey {
                filter,
                book_id: book_id.map(str::to_owned),
                source_path: source_path.map(str::to_owned),
                query: query.to_owned(),
            })
    }

    pub(in crate::app) fn refresh(
        &mut self,
        filter: BookmarkFilter,
        book_id: Option<&str>,
        source_path: Option<&str>,
        query: &str,
        entries: Vec<PageBookmarkEntry>,
    ) {
        self.key = Some(BookmarkRowsKey {
            filter,
            book_id: book_id.map(str::to_owned),
            source_path: source_path.map(str::to_owned),
            query: query.to_owned(),
        });
        self.rows = filtered_bookmark_rows(entries, query, filter);
    }

    pub(in crate::app) fn len(&self) -> usize {
        self.rows.len()
    }

    pub(in crate::app) fn row(&self, index: usize) -> Option<BookmarkRow> {
        self.rows.get(index).cloned()
    }
}

fn scope_key(
    filter: BookmarkFilter,
    book_id: Option<&str>,
    source_path: Option<&str>,
) -> BookmarkScopeKey {
    BookmarkScopeKey {
        filter,
        book_id: book_id.map(str::to_owned),
        source_path: source_path.map(str::to_owned),
    }
}

fn filtered_bookmark_rows(
    entries: Vec<PageBookmarkEntry>,
    query: &str,
    filter: BookmarkFilter,
) -> Vec<BookmarkRow> {
    let query = query.trim().to_lowercase();
    let mut output: Vec<_> = entries
        .into_iter()
        .map(bookmark_row)
        .filter(|row| {
            if query.is_empty() {
                return true;
            }
            row.search_text.contains(&query)
        })
        .collect();

    match filter {
        BookmarkFilter::ThisBook => output.sort_by_key(|row| row.bookmark.page),
        BookmarkFilter::All => output.sort_by(|left, right| {
            right
                .bookmark
                .updated_at
                .cmp(&left.bookmark.updated_at)
                .then_with(|| left.display_name.cmp(&right.display_name))
        }),
    }
    output
}

fn bookmark_row(entry: PageBookmarkEntry) -> BookmarkRow {
    let parts = path_labels::bookmark_display_parts(
        entry.known_path.as_deref(),
        entry.bookmark.page_name.as_deref(),
        &entry.bookmark.title,
    );
    let search_text = [
        parts.full.as_str(),
        parts.title.as_str(),
        parts.context.as_str(),
        entry.known_path.as_deref().unwrap_or_default(),
        entry.book_title.as_str(),
        entry.bookmark.title.as_str(),
        entry.bookmark.page_name.as_deref().unwrap_or_default(),
    ]
    .join("\n")
    .to_lowercase();

    BookmarkRow {
        book_id: entry.book_id,
        known_path: entry.known_path,
        bookmark: entry.bookmark,
        display_name: parts.full,
        search_text,
    }
}

#[cfg(test)]
mod tests {
    use super::{filtered_bookmark_rows, BookmarkFilter, BookmarkRow, BookmarkRowsCache};
    use crate::core::state::{PageBookmark, PageBookmarkEntry};

    #[test]
    fn scope_count_is_kept_per_scope_and_dropped_on_clear() {
        let mut cache = BookmarkRowsCache::default();
        assert_eq!(
            cache.scope_count(BookmarkFilter::All, Some("book-1"), Some("C:/books/book-1")),
            None
        );

        cache.set_scope_count(
            BookmarkFilter::All,
            Some("book-1"),
            Some("C:/books/book-1"),
            7,
        );
        assert_eq!(
            cache.scope_count(BookmarkFilter::All, Some("book-1"), Some("C:/books/book-1")),
            Some(7)
        );

        // A different scope, book, or path is a different question.
        assert_eq!(
            cache.scope_count(
                BookmarkFilter::ThisBook,
                Some("book-1"),
                Some("C:/books/book-1")
            ),
            None
        );
        assert_eq!(
            cache.scope_count(BookmarkFilter::All, Some("book-2"), Some("C:/books/book-1")),
            None
        );
        assert_eq!(
            cache.scope_count(BookmarkFilter::All, Some("book-1"), Some("C:/books/other")),
            None
        );

        // Every bookmark mutation clears the row cache; the count must go too,
        // or the popover header and delete dialog keep quoting a stale total.
        cache.clear();
        assert_eq!(
            cache.scope_count(BookmarkFilter::All, Some("book-1"), Some("C:/books/book-1")),
            None
        );
    }

    #[test]
    fn filtered_bookmark_rows_searches_display_path_and_title() {
        let entries = sample_entries();

        let title = filtered_bookmark_rows(entries.clone(), "cover", BookmarkFilter::All);
        assert_eq!(pages(&title), vec![0]);

        let page = filtered_bookmark_rows(entries, "page-012", BookmarkFilter::All);
        assert_eq!(pages(&page), vec![11]);
    }

    #[test]
    fn filtered_bookmark_rows_sorts_by_filter_mode() {
        let entries = sample_entries();

        let all = filtered_bookmark_rows(entries.clone(), "", BookmarkFilter::All);
        assert_eq!(pages(&all), vec![0, 5, 11]);

        let this_book_entries: Vec<_> = entries
            .into_iter()
            .filter(|entry| entry.book_id == "book-1")
            .collect();
        let this_book = filtered_bookmark_rows(this_book_entries, "", BookmarkFilter::ThisBook);
        assert_eq!(pages(&this_book), vec![0, 5]);
    }

    fn sample_entries() -> Vec<PageBookmarkEntry> {
        vec![
            entry(
                "book-1",
                "C:/books/book-1",
                bookmark("C:/books/book-1", 5, "Middle", 20, "middle.png"),
            ),
            entry(
                "book-1",
                "C:/books/book-1",
                bookmark("C:/books/book-1", 0, "Cover", 30, "cover.png"),
            ),
            entry(
                "book-2",
                "C:/books/book-2.cbz",
                bookmark(
                    "C:/books/book-2.cbz",
                    11,
                    "Pinned",
                    10,
                    "chapter/page-012.jpg",
                ),
            ),
        ]
    }

    fn entry(book_id: &str, known_path: &str, bookmark: PageBookmark) -> PageBookmarkEntry {
        PageBookmarkEntry {
            book_id: book_id.to_owned(),
            book_title: book_id.to_owned(),
            known_path: Some(known_path.to_owned()),
            bookmark,
        }
    }

    fn bookmark(
        source_path: &str,
        page: usize,
        title: &str,
        updated_at: u64,
        page_name: &str,
    ) -> PageBookmark {
        PageBookmark {
            page,
            source_path: source_path.to_owned(),
            title: title.to_owned(),
            page_name: Some(page_name.to_owned()),
            pinned: false,
            created_at: 1,
            updated_at,
        }
    }

    fn pages(bookmarks: &[BookmarkRow]) -> Vec<usize> {
        bookmarks.iter().map(|row| row.bookmark.page).collect()
    }
}
