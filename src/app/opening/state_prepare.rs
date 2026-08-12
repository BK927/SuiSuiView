use super::{
    bookmark_path_for_open, followup_seed_matches_installed_page, pending_bookmark_page,
    selected_open_page, LoaderEvent, LoaderFailure, OpenFailureAction, OpenOrigin,
    OpenViewFallback,
};
use crate::app::{SeededPreparedPage, SuiSuiViewApp};
use crate::core::source::SharedSource;
use crate::core::state::{
    BookRecordAdoptionHint, PrepareBookForOpenError, ReadingPosition, StateStore,
};
use crate::core::worker::NavigationDirection;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

const MAX_BOOK_DISCOVERY_ATTEMPTS: u8 = 3;
const BOOK_DISCOVERY_RETRY_DELAY: Duration = Duration::from_millis(2);

pub(in crate::app) struct PreparedSourceOpen {
    pub(in crate::app) source: SharedSource,
    pub(in crate::app) forced_page: Option<usize>,
    pub(in crate::app) adoption_hint: BookRecordAdoptionHint,
    pub(in crate::app) speculative_reading_position: Option<ReadingPosition>,
}

pub(super) struct PreparedSourceContext {
    pub(super) path: PathBuf,
    pub(super) origin: OpenOrigin,
    pub(super) initial_direction: NavigationDirection,
    pub(super) view_fallback: Option<OpenViewFallback>,
    pub(super) explicit_page: Option<usize>,
    pub(super) failure_action: OpenFailureAction,
    pub(super) seeded_page: Option<SeededPreparedPage>,
    pub(super) seeded_followup_page: Option<SeededPreparedPage>,
    pub(super) discovery_attempt: u8,
}

pub(in crate::app) fn prepare_source_open(
    store: &StateStore,
    source: SharedSource,
    forced_page: Option<usize>,
    origin: OpenOrigin,
    opened_path: &Path,
    allow_identity_match: bool,
) -> std::io::Result<PreparedSourceOpen> {
    let bookmark_path = bookmark_path_for_open(origin, opened_path, source.as_ref());
    let legacy_book_id = source.legacy_book_id();
    let mut attempt = 0;
    let adoption_hint = loop {
        match store.discover_book_record_adoption(
            source.book_id(),
            legacy_book_id.as_deref(),
            bookmark_path,
        ) {
            Ok(hint) => break hint,
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock
                    && attempt + 1 < MAX_BOOK_DISCOVERY_ATTEMPTS =>
            {
                attempt += 1;
                thread::sleep(BOOK_DISCOVERY_RETRY_DELAY);
            }
            Err(error) => return Err(error),
        }
    };
    let speculative_book_id = match &adoption_hint {
        BookRecordAdoptionHint::Candidate { book_id, .. } => book_id.as_str(),
        BookRecordAdoptionHint::DestinationExact { .. }
        | BookRecordAdoptionHint::NotFound { .. }
        | BookRecordAdoptionHint::Ambiguous { .. } => source.book_id(),
    };
    let speculative_reading_position =
        store.reading_position(speculative_book_id, bookmark_path, allow_identity_match);
    Ok(PreparedSourceOpen {
        source,
        forced_page,
        adoption_hint,
        speculative_reading_position,
    })
}

impl SuiSuiViewApp {
    pub(super) fn finish_prepared_source_open(
        &mut self,
        prepared: PreparedSourceOpen,
        context: PreparedSourceContext,
    ) {
        let PreparedSourceOpen {
            source,
            forced_page,
            adoption_hint,
            ..
        } = prepared;
        let bookmark_path =
            bookmark_path_for_open(context.origin, &context.path, source.as_ref()).to_path_buf();
        let book_id = source.book_id().to_owned();
        let prepared_book_state = match self.store.prepare_book_for_open_from_hint(
            &book_id,
            &bookmark_path,
            self.settings.resume_by_file_identity,
            adoption_hint,
        ) {
            Ok(book_state) => book_state,
            Err(PrepareBookForOpenError::StaleHint) => {
                // Discovery is read-only. If another instance changed the catalog,
                // keep the source and repeat only its background state lookup.
                if context.discovery_attempt < MAX_BOOK_DISCOVERY_ATTEMPTS {
                    self.retry_prepared_source_open(source, forced_page, context);
                } else {
                    self.finish_book_state_open_failure(
                        &context.path,
                        format!(
                            "Could not open {}: book history kept changing",
                            context.path.display()
                        ),
                        context.failure_action,
                    );
                }
                return;
            }
            Err(PrepareBookForOpenError::Io(error)) => {
                self.finish_book_state_open_failure(
                    &context.path,
                    format!("Could not open {}: {}", context.path.display(), error),
                    context.failure_action,
                );
                return;
            }
        };
        let pending_page = self
            .pending_bookmark_jump
            .as_ref()
            .and_then(|pending| pending_bookmark_page(source.as_ref(), pending));
        let final_page = selected_open_page(
            source.as_ref(),
            context.explicit_page,
            forced_page,
            prepared_book_state.reading_position.as_ref(),
            pending_page,
        );
        // The background seed is speculative; main-store finalization can
        // observe a newer automatic position before the source is installed.
        let seeded_page = context.seeded_page.filter(|seed| seed.index == final_page);
        let seeded_page_index = seeded_page.as_ref().map(|seed| seed.index);
        let seeded_followup_page = if seeded_page.is_some() {
            context.seeded_followup_page
        } else {
            None
        };
        self.install_source(
            source,
            forced_page,
            prepared_book_state,
            context.origin,
            context.path,
            seeded_page,
            context.initial_direction,
            context.view_fallback,
            context.explicit_page,
        );
        if followup_seed_matches_installed_page(seeded_page_index, self.current_page) {
            self.insert_seeded_page_if_matching_target(seeded_followup_page);
        }
    }

    fn retry_prepared_source_open(
        &mut self,
        source: SharedSource,
        forced_page: Option<usize>,
        context: PreparedSourceContext,
    ) {
        self.loader_generation = self.loader_generation.wrapping_add(1);
        let generation = self.loader_generation;
        let store = self.store.fork_for_background();
        let resume_by_file_identity = self.settings.resume_by_file_identity;
        let tx = self.loader_tx.clone();
        let ctx = self.egui_ctx.clone();
        self.set_status(self.i18n().text("status.opening"));
        let failure_action = context.failure_action;
        let failure_path = context.path.clone();
        let spawn_result = thread::Builder::new()
            .name("suisuiview-book-state-retry".to_owned())
            .spawn(move || {
                let result = prepare_source_open(
                    &store,
                    source,
                    forced_page,
                    context.origin,
                    &context.path,
                    resume_by_file_identity,
                )
                .map_err(|error| LoaderFailure::State(error.to_string()));
                let _ = tx.send(LoaderEvent {
                    generation,
                    path: context.path,
                    origin: context.origin,
                    initial_direction: context.initial_direction,
                    view_fallback: context.view_fallback,
                    explicit_page: context.explicit_page,
                    failure_action: context.failure_action,
                    result,
                    seeded_page: context.seeded_page,
                    seeded_followup_page: context.seeded_followup_page,
                    discovery_attempt: context.discovery_attempt + 1,
                });
                ctx.request_repaint();
            });
        match spawn_result {
            Ok(_) => self.loader_pending = true,
            Err(error) => self.finish_book_state_open_failure(
                &failure_path,
                format!("Could not restart book history lookup: {error}"),
                failure_action,
            ),
        }
    }

    fn finish_book_state_open_failure(
        &mut self,
        path: &Path,
        message: String,
        failure_action: OpenFailureAction,
    ) {
        self.sibling_open_retry = None;
        if self
            .pending_bookmark_jump
            .as_ref()
            .is_some_and(|pending| pending.path == path)
        {
            self.pending_bookmark_jump = None;
        }
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        {
            self.open_to_first_visible_trace = None;
        }
        self.sibling_book_visual_pending = false;
        self.clear_pending_sibling_book_turns();
        self.handle_open_failure(message, failure_action);
    }
}
