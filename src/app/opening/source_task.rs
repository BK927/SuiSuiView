use super::*;
use crate::app::sibling_books::{same_path, sibling_book_path};

const SIBLING_OPEN_RETRY_LIMIT: usize = 16;

pub(in crate::app) struct SourceTaskOutput {
    generation: u64,
    result: Result<Option<LoaderEvent>, String>,
    failure_action: OpenFailureAction,
}

impl SuiSuiViewApp {
    pub(in crate::app) fn open_sibling_async(&mut self, current: PathBuf, direction: isize) {
        self.pending_bookmark_jump = None;
        let fallback = Some(self.open_view_fallback());
        self.clear_adjacent_seed_cache();
        self.start_source_task(
            current,
            if direction >= 0 {
                NavigationDirection::Forward
            } else {
                NavigationDirection::Backward
            },
            fallback,
            None,
            OpenFailureAction::KeepCurrent,
            Some(direction),
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn start_source_task(
        &mut self,
        path: PathBuf,
        initial_direction: NavigationDirection,
        view_fallback: Option<OpenViewFallback>,
        explicit_page: Option<usize>,
        failure_action: OpenFailureAction,
        sibling_direction: Option<isize>,
    ) {
        self.source_task_failure_action = failure_action;
        self.pending_delete_dialog = None;
        self.loader_generation = self.loader_generation.wrapping_add(1);
        let generation = self.loader_generation;
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        {
            self.open_to_first_visible_trace =
                Some(perf::OpenToFirstVisibleTrace::new("async_open"));
        }
        let store = self.store.fork_for_background();
        let settings = self.settings.clone();
        let seed_target_long_edge = open_seed_target_long_edge(self.target_long_edge);
        let seed_plan = self.open_seed_plan();
        let pending_jump = self.pending_bookmark_jump.clone();
        self.loader_pending = true;
        self.refresh_worker_prefetch();
        self.set_status(self.i18n().text("status.opening"));
        self.source_task.request_cancellable(&self.egui_ctx, move |cancellation| {
            let result = (|| {
                let _stall_scope = crate::core::stall_trace::scope(crate::core::stall_trace::Stage::OpenPath);
                let origin_path = path.clone();
                let mut path = path;
                let mut last_failure = None;
                for attempt in 0..=SIBLING_OPEN_RETRY_LIMIT {
                    if cancellation.cancelled() { return Ok(None); }
                    if let Some(direction) = sibling_direction {
                        let _stall_scope = crate::core::stall_trace::scope(crate::core::stall_trace::Stage::SiblingNavigation);
                        let Some(next) = sibling_book_path(&path, direction) else { return last_failure.map_or(Ok(None), Err); };
                        // The first candidate may legitimately wrap to the only
                        // book. After a failed attempt, stop when we circle back.
                        if attempt > 0 && same_path(&next, &origin_path) { return last_failure.map_or(Ok(None), Err); }
                        path = next;
                    }
                    if cancellation.cancelled() { return Ok(None); }
                    let kind = classify_path(&path);
                    if cancellation.cancelled() { return Ok(None); }
                    let Some(origin) = open_origin_for_source_kind(kind) else {
                        let message = if kind == SourceKind::UnsupportedRar {
                            "CBR/RAR requires the restricted read-only archive backend before it can be opened.".to_owned()
                        } else {
                            unsupported_message_for_extension(path.extension().and_then(|value| value.to_str()).unwrap_or_default())
                                .unwrap_or_else(|| format!("Unsupported file type: {}", path.display()))
                        };
                        return Err(message);
                    };
                    let started = Instant::now();
                    let source_result = open_source_from_path(&path);
                    if cancellation.cancelled() { return Ok(None); }
                    perf::record_open_source(started, origin.perf_label(), source_result.is_ok());
                    let result = match source_result {
                        Ok((source, forced_page)) => prepare_source_open(&store, source, forced_page, origin, &path, settings.resume_by_file_identity)
                            .map_err(|error| LoaderFailure::State(error.to_string())),
                        Err(error) => Err(LoaderFailure::Source(error.to_string())),
                    };
                    if sibling_direction.is_some() && attempt < SIBLING_OPEN_RETRY_LIMIT {
                        if let Err(LoaderFailure::Source(error)) = &result {
                            last_failure = Some(format!("Could not open {}: {error}", path.display()));
                            continue;
                        }
                    }
                    if cancellation.cancelled() { return Ok(None); }
                    let seeded_page = result.as_ref().ok().and_then(|prepared| {
                        let (decode, seed_target_view) = seed_plan.resolve(&settings, prepared.speculative_reading_position.as_ref(), view_fallback);
                        let pending_page = pending_jump.as_ref().and_then(|pending| pending_bookmark_page(prepared.source.as_ref(), pending));
                        let page_index = selected_open_page(prepared.source.as_ref(), explicit_page, prepared.forced_page, prepared.speculative_reading_position.as_ref(), pending_page);
                        let started = Instant::now();
                        let seeded = prepare_seeded_first_page(prepared.source.as_ref(), page_index, seed_target_long_edge, decode, false, seed_target_view);
                        perf::record_startup_seed_prepare(started, origin.perf_label(), page_index, seed_target_long_edge, seeded.is_some());
                        seeded
                    });
                    if cancellation.cancelled() { return Ok(None); }
                    return Ok(Some(LoaderEvent { generation, path, origin, initial_direction, view_fallback, explicit_page, failure_action, result, seeded_page, seeded_followup_page: None, discovery_attempt: 1 }));
                }
                unreachable!("the last attempt always returns its loader result")
            })();
            SourceTaskOutput { generation, result, failure_action }
        });
    }

    pub(in crate::app) fn drain_source_task(&mut self) {
        let Some(result) = self.source_task.poll() else {
            return;
        };
        let (result, failure_action) = match result {
            Ok(output) if output.generation == self.loader_generation => {
                (output.result, output.failure_action)
            }
            Ok(_) => return,
            Err(error) => (Err(error), self.source_task_failure_action),
        };
        match result {
            Ok(Some(event)) => {
                let _ = self.loader_tx.send(event);
            }
            result => {
                self.loader_pending = false;
                self.pending_bookmark_jump = None;
                self.sibling_book_visual_pending = false;
                #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
                {
                    self.open_to_first_visible_trace = None;
                }
                match result {
                    Ok(None) => {
                        self.refresh_worker_prefetch();
                        self.set_status(self.i18n().text("status.no_sibling_book"));
                    }
                    Err(error) => {
                        self.clear_pending_sibling_book_turns();
                        self.handle_open_failure(error, failure_action);
                    }
                    Ok(Some(_)) => unreachable!(),
                }
            }
        }
    }
}
