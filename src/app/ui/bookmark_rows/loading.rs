use super::*;

#[cfg(test)]
mod tests;

pub(super) struct LoadedCatalog {
    scope: BookmarkScopeKey,
    entries: Arc<Vec<PageBookmarkEntry>>,
}

impl BookmarkRowsCache {
    // Disk scope and search are independent. Typing while a catalog read is
    // blocked retains that read and filters its result with the latest query.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::app) fn refresh_async(
        &mut self,
        ctx: &egui::Context,
        fork_store: impl FnOnce() -> StateStore,
        filter: BookmarkFilter,
        book_id: Option<&str>,
        source_path: Option<&str>,
        query: &str,
    ) {
        let scope = scope_key(filter, book_id, source_path);
        if self.loading_scope.as_ref() != Some(&scope) {
            let store = fork_store();
            let requested_scope = scope.clone();
            self.loading.request(ctx, move || {
                let _stall_scope =
                    crate::core::stall_trace::scope(crate::core::stall_trace::Stage::BookmarkList);
                let entries = match requested_scope.filter {
                    BookmarkFilter::All => store.try_all_page_bookmarks(),
                    BookmarkFilter::ThisBook => requested_scope
                        .book_id
                        .as_deref()
                        .zip(requested_scope.source_path.as_deref())
                        .map(|(id, path)| {
                            store.try_page_bookmark_entries(id, std::path::Path::new(path))
                        })
                        .unwrap_or_else(|| Ok(Vec::new())),
                }
                .map_err(|error| error.to_string())?;
                Ok(LoadedCatalog {
                    scope: requested_scope,
                    entries: Arc::new(entries),
                })
            });
            self.loading_scope = Some(scope.clone());
            self.catalog = None;
            self.scope_count = None;
            self.error = None;
            self.key = None;
            self.rows.clear();
            self.filtering.cancel();
        }
        if let Some(result) = self.loading.poll() {
            match result.and_then(|result| result) {
                Ok(loaded) => {
                    self.scope_count = Some((loaded.scope.clone(), loaded.entries.len()));
                    self.catalog = Some((loaded.scope, loaded.entries));
                }
                Err(error) => self.error = Some(error),
            }
        }
        if let Some((_, entries)) = &self.catalog {
            if self.needs_refresh(filter, book_id, source_path, query) {
                let entries = entries.clone();
                let search = query.to_owned();
                self.filtering.request(ctx, move || {
                    filtered_bookmark_rows(entries.iter().cloned(), &search, filter)
                });
                self.key = Some(BookmarkRowsKey {
                    scope,
                    query: query.to_owned(),
                });
                self.rows.clear();
                self.error = None;
            }
        }
        if let Some(result) = self.filtering.poll() {
            match result {
                Ok(rows) => self.rows = rows,
                Err(error) => self.error = Some(error),
            }
        }
    }

    pub(in crate::app) fn is_loading(&self) -> bool {
        self.loading.pending() || self.filtering.pending()
    }
    pub(in crate::app) fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }
    pub(in crate::app) fn cancel_loading(&mut self) {
        if self.is_loading() {
            self.clear();
        }
    }
}
