use super::{opening, OpenOrigin, SuiSuiViewApp};
use crate::core::source::{BookSource, SharedSource, SourceError};
use crate::core::worker::NavigationDirection;
use std::path::PathBuf;
use std::thread;

/// One in-flight snapshot rebuild for the currently open folder book.
pub(in crate::app) struct RefreshTicket {
    pub(in crate::app) book_id: String,
    pub(in crate::app) opened_path: PathBuf,
    pub(in crate::app) source_instance_id: u64,
}

pub(in crate::app) struct RefreshOutcome {
    pub(in crate::app) ticket: RefreshTicket,
    pub(in crate::app) result: Result<SharedSource, SourceError>,
}

/// True when `ticket` still describes the book the app currently has open, so a
/// completed rebuild may be applied. A stale ticket (book closed, swapped, or
/// already refreshed to a newer instance) is dropped.
pub(in crate::app) fn ticket_matches(
    current_book_id: Option<&str>,
    current_path: Option<&std::path::Path>,
    current_instance: Option<u64>,
    ticket: &RefreshTicket,
) -> bool {
    current_book_id == Some(ticket.book_id.as_str())
        && current_path == Some(ticket.opened_path.as_path())
        && current_instance == Some(ticket.source_instance_id)
}

/// True when the page at `index` has a backing file that no longer exists on
/// disk (a stale-snapshot signal). A missing `page_file_path` or an existing
/// file both return false, so a plain corrupt-decode failure does not trigger a
/// refresh.
pub(in crate::app) fn folder_page_file_vanished(source: &dyn BookSource, index: usize) -> bool {
    source
        .page_file_path(index)
        .is_some_and(|path| !path.exists())
}

/// Resolves the page to show after a snapshot swap. If the current page's id
/// still maps in `new`, that index wins. Otherwise walk the OLD order from
/// `current` along `direction` (then the opposite side) and return the new
/// index of the first surviving id. Falls back to 0.
pub(in crate::app) fn remap_current_page(
    old: &dyn BookSource,
    new: &dyn BookSource,
    current: usize,
    direction: NavigationDirection,
) -> usize {
    if let Some(index) = old
        .page_id(current)
        .and_then(|id| new.page_index_for_id(id))
    {
        return index;
    }

    let old_count = old.page_count();
    let (primary, secondary): (
        Box<dyn Iterator<Item = usize>>,
        Box<dyn Iterator<Item = usize>>,
    ) = match direction {
        NavigationDirection::Forward => (
            Box::new((current + 1..old_count).chain(std::iter::once(current))),
            Box::new((0..current).rev()),
        ),
        NavigationDirection::Backward => (
            Box::new((0..current).rev()),
            Box::new(current + 1..old_count),
        ),
    };
    for old_index in primary.chain(secondary) {
        if let Some(index) = old
            .page_id(old_index)
            .and_then(|id| new.page_index_for_id(id))
        {
            return index;
        }
    }
    0
}

impl SuiSuiViewApp {
    /// Rebuild the current folder book's snapshot off-thread. No-op unless a
    /// folder source is open; debounced against an already-running rebuild for
    /// the same source instance.
    pub(in crate::app) fn request_folder_refresh(&mut self) {
        if self.open_origin != Some(OpenOrigin::Folder) {
            return;
        }
        let (Some(source), Some(book_id), Some(opened_path)) = (
            self.source.as_ref(),
            self.book_id.as_ref(),
            self.opened_path.as_ref(),
        ) else {
            return;
        };
        let source_instance_id = source.source_instance_id();
        if self
            .refresh_inflight
            .as_ref()
            .is_some_and(|ticket| ticket.source_instance_id == source_instance_id)
        {
            return;
        }
        let ticket = RefreshTicket {
            book_id: book_id.clone(),
            opened_path: opened_path.clone(),
            source_instance_id,
        };
        let old_source = source.clone();
        let tx = self.refresh_tx.clone();
        let ctx = self.egui_ctx.clone();
        self.refresh_inflight = Some(RefreshTicket {
            book_id: ticket.book_id.clone(),
            opened_path: ticket.opened_path.clone(),
            source_instance_id,
        });
        let spawned = thread::Builder::new()
            .name("suisuiview-folder-refresh".to_owned())
            .spawn(move || {
                // A non-refreshable source kind returns None; send nothing.
                let Some(result) = old_source.refresh_snapshot() else {
                    return;
                };
                let _ = tx.send(RefreshOutcome { ticket, result });
                ctx.request_repaint();
            });
        if spawned.is_err() {
            self.refresh_inflight = None;
        }
    }

    /// Applies completed folder-refresh rebuilds. Mirrors the frame-loop drain
    /// of loader events. Stale tickets are dropped; matching outcomes swap the
    /// snapshot in, preserving reading position by page identity.
    pub(in crate::app) fn drain_refresh_outcomes(&mut self) {
        while let Ok(outcome) = self.refresh_rx.try_recv() {
            let current_instance = self
                .source
                .as_ref()
                .map(|source| source.source_instance_id());
            if !ticket_matches(
                self.book_id.as_deref(),
                self.opened_path.as_deref(),
                current_instance,
                &outcome.ticket,
            ) {
                continue;
            }
            // Consuming a matching ticket: this rebuild is no longer in flight.
            self.refresh_inflight = None;
            match outcome.result {
                Ok(new_source) => self.apply_refreshed_source(new_source),
                Err(SourceError::NoPages(_)) => {
                    self.clear_local_book_state(self.i18n().text("status.folder_emptied"));
                }
                Err(other) => {
                    self.notify(
                        self.i18n()
                            .with_vars("status.refresh_failed", &[("error", other.to_string())]),
                    );
                }
            }
        }
    }

    fn apply_refreshed_source(&mut self, new_source: SharedSource) {
        let Some(old_source) = self.source.clone() else {
            return;
        };
        let page_count = new_source.page_count();
        self.current_page = remap_current_page(
            old_source.as_ref(),
            new_source.as_ref(),
            self.current_page,
            self.last_nav_direction,
        );

        // Retain id-keyed side maps only where the id still maps in `new_source`;
        // decoded/texture LRUs are bounded and identity-keyed, so vanished ids
        // simply age out instead of being force-evicted.
        self.page_metrics
            .retain(|page_id, _| new_source.page_index_for_id(*page_id).is_some());
        self.strip_dim_hints
            .retain(|page_id, _| new_source.page_index_for_id(*page_id).is_some());
        self.note_strip_dims_changed();
        self.page_errors
            .retain(|key, _| new_source.page_index_for_id(key.page_id).is_some());

        // Saved bookmarks are stored by page index, so the snapshot's new ordering
        // has to be written back or every bookmark past a removed file points one
        // image off. `page_name` is the identity to re-resolve them by; a name that
        // no longer exists means the file itself is gone.
        if let Some(book_id) = self.book_id.clone() {
            let source_path = old_source.source_path().to_path_buf();
            self.store
                .remap_page_bookmarks(&book_id, &source_path, |page_name| {
                    opening::page_index_for_name(new_source.as_ref(), page_name)
                });
            self.bookmark_rows.clear();
        }

        self.transition = None;
        // A page turn waiting on its target's decode requested this refresh's
        // snapshot swap; dropping it swallows the keypress outright. Carry the
        // target across by identity and re-issue it against the new snapshot once
        // the source is installed.
        let pending_turn = self.pending_page_turn.take().and_then(|pending| {
            let page_id = old_source.page_id(pending.target)?;
            let target = new_source.page_index_for_id(page_id)?;
            Some((target, pending.direction))
        });
        self.clear_pending_page_turns();
        self.source = Some(new_source.clone());

        // Instance change bumps the worker epoch; book_id equality preserves its
        // caches (LRU / source bytes / hints), so nothing re-decodes.
        self.worker.load_book(
            new_source,
            self.worker_center_page(),
            self.last_nav_direction,
            self.target_long_edge,
            self.visible_page_count(),
            self.worker_options(),
        );
        self.persist_reading_position();
        self.set_status(self.i18n().with_vars(
            "status.folder_refreshed",
            &[("count", page_count.to_string())],
        ));
        if let Some((target, direction)) = pending_turn {
            // Either commits on the refreshed snapshot or re-defers against it,
            // exactly as the original press would have.
            self.set_page(target, direction);
        }
        self.egui_ctx.request_repaint();
    }
}

#[cfg(test)]
mod tests;
