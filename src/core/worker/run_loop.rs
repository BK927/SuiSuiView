use super::api::{
    CachedPageKey, DecodeOptions, NavigationDirection, PreparedPage, WorkerCommand, WorkerEvent,
    WorkerOptions,
};
#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
use super::cache::record_worker_cache_snapshot;
use super::cache::{
    clear_cache_on_book_or_decode_change, clear_published_app_cache_hints_on_context_change,
    insert_worker_cache_with_budget, page_cache_key, prune_worker_cache,
    remember_published_app_cache_hint, should_skip_published_app_cache_hint, update_book_epoch,
    PublishedAppCacheHints,
};
use super::decode_ahead::{
    self, cancel_pending_decode as cancel_pending_decode_ahead,
    cancel_pending_decode_if_not_scheduled, clear_pending_decode as clear_pending_decode_ahead,
    clear_pending_decode_if_context_changed, consume_matching_decode, DecodeAhead,
};
use super::decode_policy::DecodeAheadPolicy;
use super::prepare::prepare_page_with_perf;
use super::read_ahead::{self, clear_pending as clear_pending_read_ahead, ReadAhead};
use super::scheduler::{
    is_visible_page_index, prioritized_jobs, should_skip_ai_preview_or_prefetch,
};
use super::source_bytes::{read_source_bytes, SourceBytesCache};
use super::DEFAULT_TARGET_LONG_EDGE;
#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
use crate::core::perf_trace::{self, PerfField};
use crate::core::source::SharedSource;
use crossbeam_channel::{Receiver, Sender};
use egui::Context;
use lru::LruCache;
use std::num::NonZeroUsize;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
use std::time::Duration;

const WORKER_CACHE_ENTRY_LIMIT: usize = 12;

pub(in crate::core::worker) fn run_worker(
    command_rx: Receiver<WorkerCommand>,
    event_tx: Sender<WorkerEvent>,
    ctx: Context,
    shutdown_requested: Arc<AtomicBool>,
) {
    let mut source: Option<SharedSource> = None;
    let mut center = 0usize;
    let mut direction = NavigationDirection::Forward;
    let mut target_long_edge = DEFAULT_TARGET_LONG_EDGE;
    let mut visible_pages = 1usize;
    let mut options = WorkerOptions::default();
    let mut cache: LruCache<String, Arc<PreparedPage>> =
        LruCache::new(NonZeroUsize::new(WORKER_CACHE_ENTRY_LIMIT).unwrap());
    let mut cache_bytes = 0usize;
    let mut book_epoch = 0usize;
    let mut published_app_cache_hints = PublishedAppCacheHints::new();
    let mut read_ahead: Option<ReadAhead> = None;
    let mut decode_ahead: Option<DecodeAhead> = None;
    let mut decode_ahead_policy = DecodeAheadPolicy::from_env();
    let mut source_bytes_cache = SourceBytesCache::from_env();

    while !shutdown_requested.load(Ordering::Acquire) {
        let Ok(command) = command_rx.recv() else {
            break;
        };
        clear_pending_read_ahead(&mut read_ahead, "command");
        let previous_book_id = source.as_ref().map(|source| source.book_id().to_owned());
        let previous_instance_id = source.as_ref().map(|source| source.source_instance_id());
        let previous_decode = options.decode;
        let previous_target_long_edge = target_long_edge;
        if !apply_command(
            command,
            &mut source,
            &mut center,
            &mut direction,
            &mut target_long_edge,
            &mut visible_pages,
            &mut options,
        ) {
            break;
        }
        update_book_epoch(
            &mut book_epoch,
            &source,
            previous_book_id.as_deref(),
            previous_instance_id,
        );
        clear_published_app_cache_hints_on_context_change(
            &source,
            previous_book_id.as_deref(),
            previous_decode,
            previous_target_long_edge,
            options.decode,
            target_long_edge,
            &mut published_app_cache_hints,
        );
        clear_cache_on_book_or_decode_change(
            &source,
            previous_book_id.as_deref(),
            previous_decode,
            options.decode,
            &mut cache,
            &mut cache_bytes,
        );
        clear_source_bytes_cache_on_book_change(
            &mut source_bytes_cache,
            &source,
            previous_book_id.as_deref(),
        );
        prune_worker_cache(&mut cache, &mut cache_bytes, options.cache_bytes);
        reset_decode_ahead_policy_if_context_changed(
            &mut decode_ahead_policy,
            &source,
            previous_book_id.as_deref(),
            previous_decode,
            previous_target_long_edge,
            options.decode,
            target_long_edge,
        );
        clear_decode_ahead_if_context_changed(
            &mut decode_ahead,
            &source,
            book_epoch,
            target_long_edge,
            options.decode,
        );

        'work: loop {
            if shutdown_requested.load(Ordering::Acquire) {
                break;
            }
            let Some(active_source) = source.as_ref().cloned() else {
                break;
            };
            let book_id = active_source.book_id().to_owned();
            let jobs = prioritized_jobs(
                center,
                active_source.page_count(),
                direction,
                target_long_edge,
                options.target_intent,
                visible_pages,
                options.prefetch_enabled,
                options.progressive_preview_enabled,
            );
            cancel_pending_decode_if_not_scheduled(
                &mut decode_ahead,
                &active_source,
                &book_id,
                book_epoch,
                &jobs,
                center,
                visible_pages,
                &options,
                &cache,
                &published_app_cache_hints,
            );

            for (job_position, job) in jobs.iter().copied().enumerate() {
                if shutdown_requested.load(Ordering::Acquire) {
                    break 'work;
                }
                let Some(page_id) = active_source.page_id(job.index) else {
                    continue;
                };
                if should_skip_ai_preview_or_prefetch(
                    active_source.page_name(job.index),
                    center,
                    visible_pages,
                    job.index,
                    job.target_long_edge,
                ) {
                    continue;
                }
                if let Some(command) = drain_latest_command(&command_rx) {
                    clear_pending_read_ahead(&mut read_ahead, "command");
                    let previous_book_id =
                        source.as_ref().map(|source| source.book_id().to_owned());
                    let previous_instance_id =
                        source.as_ref().map(|source| source.source_instance_id());
                    let previous_decode = options.decode;
                    let previous_target_long_edge = target_long_edge;
                    if !apply_command(
                        command,
                        &mut source,
                        &mut center,
                        &mut direction,
                        &mut target_long_edge,
                        &mut visible_pages,
                        &mut options,
                    ) {
                        return;
                    }
                    update_book_epoch(
                        &mut book_epoch,
                        &source,
                        previous_book_id.as_deref(),
                        previous_instance_id,
                    );
                    clear_published_app_cache_hints_on_context_change(
                        &source,
                        previous_book_id.as_deref(),
                        previous_decode,
                        previous_target_long_edge,
                        options.decode,
                        target_long_edge,
                        &mut published_app_cache_hints,
                    );
                    clear_cache_on_book_or_decode_change(
                        &source,
                        previous_book_id.as_deref(),
                        previous_decode,
                        options.decode,
                        &mut cache,
                        &mut cache_bytes,
                    );
                    clear_source_bytes_cache_on_book_change(
                        &mut source_bytes_cache,
                        &source,
                        previous_book_id.as_deref(),
                    );
                    prune_worker_cache(&mut cache, &mut cache_bytes, options.cache_bytes);
                    reset_decode_ahead_policy_if_context_changed(
                        &mut decode_ahead_policy,
                        &source,
                        previous_book_id.as_deref(),
                        previous_decode,
                        previous_target_long_edge,
                        options.decode,
                        target_long_edge,
                    );
                    clear_decode_ahead_if_context_changed(
                        &mut decode_ahead,
                        &source,
                        book_epoch,
                        target_long_edge,
                        options.decode,
                    );
                    continue 'work;
                }

                let key = page_cache_key(&book_id, page_id, job.target_long_edge, options.decode);
                #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
                perf_trace::record_duration(
                    "page_worker_job_start",
                    Duration::ZERO,
                    &[
                        PerfField::Usize("page", job.index),
                        PerfField::Usize("book_epoch", book_epoch),
                        PerfField::U32("target_long_edge", job.target_long_edge),
                    ],
                );
                if let Some(page) = cache.get(&key).cloned() {
                    let _ = event_tx.send(WorkerEvent::PageReady {
                        book_id: book_id.clone(),
                        page_id,
                        decode: options.decode,
                        page,
                    });
                    #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
                    perf_trace::record_duration(
                        "page_worker_publish",
                        Duration::ZERO,
                        &[
                            PerfField::Usize("page", job.index),
                            PerfField::Usize("book_epoch", book_epoch),
                            PerfField::U32("target_long_edge", job.target_long_edge),
                            PerfField::Bool("cache_hit", true),
                        ],
                    );
                    #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
                    record_worker_cache_snapshot(
                        "publish_hit",
                        job.index,
                        job.target_long_edge,
                        cache.len(),
                        cache_bytes,
                        options.cache_bytes,
                        true,
                    );
                    ctx.request_repaint();
                    remember_published_app_cache_hint(
                        &mut published_app_cache_hints,
                        CachedPageKey::new(page_id, job.target_long_edge, options.decode),
                    );
                    continue;
                }
                if options.app_cache_covers(page_id, job.target_long_edge)
                    || should_skip_published_app_cache_hint(
                        &published_app_cache_hints,
                        is_visible_page_index(job.index, center, visible_pages),
                        page_id,
                        job.target_long_edge,
                        options.decode,
                    )
                {
                    continue;
                }

                let result = consume_matching_decode(
                    &mut decode_ahead,
                    &book_id,
                    book_epoch,
                    page_id,
                    job.target_long_edge,
                    options.decode,
                )
                .unwrap_or_else(|| {
                    let read_result = read_source_bytes(
                        source_bytes_cache.as_mut(),
                        &mut read_ahead,
                        &active_source,
                        &book_id,
                        book_epoch,
                        page_id,
                        job.index,
                    );
                    if shutdown_requested.load(Ordering::Acquire) {
                        return Err("Page worker shutdown requested".to_owned());
                    }

                    if read_result.is_ok() {
                        if let Some(candidate) = decode_ahead_policy.candidate() {
                            let decode_ahead_reserved = decode_ahead::maybe_start_decode(
                                &mut decode_ahead,
                                &command_rx,
                                &active_source,
                                &book_id,
                                book_epoch,
                                &jobs,
                                job_position.saturating_add(1),
                                center,
                                visible_pages,
                                &options,
                                &cache,
                                &published_app_cache_hints,
                                candidate,
                                decode_ahead_policy
                                    .needs_prepare_timing_for(&active_source, job.index),
                            );
                            if !decode_ahead_reserved {
                                read_ahead::maybe_start(
                                    &mut read_ahead,
                                    &command_rx,
                                    &active_source,
                                    &book_id,
                                    book_epoch,
                                    &jobs,
                                    job_position.saturating_add(1),
                                    center,
                                    visible_pages,
                                    &options,
                                    &cache,
                                    &published_app_cache_hints,
                                );
                            }
                        } else {
                            read_ahead::maybe_start(
                                &mut read_ahead,
                                &command_rx,
                                &active_source,
                                &book_id,
                                book_epoch,
                                &jobs,
                                job_position.saturating_add(1),
                                center,
                                visible_pages,
                                &options,
                                &cache,
                                &published_app_cache_hints,
                            );
                        }
                    }

                    read_result.and_then(|bytes| {
                        prepare_page_with_perf(
                            bytes.as_ref(),
                            job,
                            book_epoch,
                            options.decode,
                            false,
                            decode_ahead_policy.needs_prepare_timing_for(&active_source, job.index),
                        )
                    })
                });
                if shutdown_requested.load(Ordering::Acquire) {
                    break 'work;
                }

                match result {
                    Ok(prepared) => {
                        let page = prepared.page;
                        let page = Arc::new(page);

                        let cached = insert_worker_cache_with_budget(
                            &mut cache,
                            &mut cache_bytes,
                            key,
                            &page,
                            options.cache_bytes,
                        );
                        if cached {
                            prune_worker_cache(&mut cache, &mut cache_bytes, options.cache_bytes);
                        }
                        let _ = event_tx.send(WorkerEvent::PageReady {
                            book_id: book_id.clone(),
                            page_id,
                            decode: options.decode,
                            page: page.clone(),
                        });
                        remember_published_app_cache_hint(
                            &mut published_app_cache_hints,
                            CachedPageKey::new(page_id, job.target_long_edge, options.decode),
                        );
                        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
                        perf_trace::record_duration(
                            "page_worker_publish",
                            Duration::ZERO,
                            &[
                                PerfField::Usize("page", job.index),
                                PerfField::Usize("book_epoch", book_epoch),
                                PerfField::U32("target_long_edge", job.target_long_edge),
                                PerfField::Bool("cache_hit", false),
                            ],
                        );
                        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
                        record_worker_cache_snapshot(
                            if cached {
                                "publish_miss"
                            } else {
                                "publish_miss_uncached_oversize"
                            },
                            job.index,
                            job.target_long_edge,
                            cache.len(),
                            cache_bytes,
                            options.cache_bytes,
                            false,
                        );
                        ctx.request_repaint();

                        decode_ahead_policy.observe_prepare(
                            &active_source,
                            job.index,
                            page.as_ref(),
                            prepared.prepare_duration,
                        );
                        if decode_ahead_policy.candidate().is_none() {
                            cancel_pending_decode_ahead(&mut decode_ahead, "policy");
                        }

                        if let Some(command) = drain_latest_command(&command_rx) {
                            clear_pending_read_ahead(&mut read_ahead, "command");
                            let previous_book_id =
                                source.as_ref().map(|source| source.book_id().to_owned());
                            let previous_instance_id =
                                source.as_ref().map(|source| source.source_instance_id());
                            let previous_decode = options.decode;
                            let previous_target_long_edge = target_long_edge;
                            if !apply_command(
                                command,
                                &mut source,
                                &mut center,
                                &mut direction,
                                &mut target_long_edge,
                                &mut visible_pages,
                                &mut options,
                            ) {
                                return;
                            }
                            update_book_epoch(
                                &mut book_epoch,
                                &source,
                                previous_book_id.as_deref(),
                                previous_instance_id,
                            );
                            clear_published_app_cache_hints_on_context_change(
                                &source,
                                previous_book_id.as_deref(),
                                previous_decode,
                                previous_target_long_edge,
                                options.decode,
                                target_long_edge,
                                &mut published_app_cache_hints,
                            );
                            clear_cache_on_book_or_decode_change(
                                &source,
                                previous_book_id.as_deref(),
                                previous_decode,
                                options.decode,
                                &mut cache,
                                &mut cache_bytes,
                            );
                            clear_source_bytes_cache_on_book_change(
                                &mut source_bytes_cache,
                                &source,
                                previous_book_id.as_deref(),
                            );
                            prune_worker_cache(&mut cache, &mut cache_bytes, options.cache_bytes);
                            reset_decode_ahead_policy_if_context_changed(
                                &mut decode_ahead_policy,
                                &source,
                                previous_book_id.as_deref(),
                                previous_decode,
                                previous_target_long_edge,
                                options.decode,
                                target_long_edge,
                            );
                            clear_decode_ahead_if_context_changed(
                                &mut decode_ahead,
                                &source,
                                book_epoch,
                                target_long_edge,
                                options.decode,
                            );
                            continue 'work;
                        }
                    }
                    Err(message) => {
                        let _ = event_tx.send(WorkerEvent::PageFailed {
                            book_id: book_id.clone(),
                            page_id,
                            target_long_edge: job.target_long_edge,
                            decode: options.decode,
                            message,
                        });
                        ctx.request_repaint();
                    }
                }
            }

            break;
        }
    }
    clear_pending_decode_ahead(&mut decode_ahead, "worker_exit");
}

fn clear_decode_ahead_if_context_changed(
    decode_ahead: &mut Option<DecodeAhead>,
    source: &Option<SharedSource>,
    book_epoch: usize,
    target_long_edge: u32,
    decode: DecodeOptions,
) {
    clear_pending_decode_if_context_changed(
        decode_ahead,
        source.as_ref().map(|source| source.book_id()),
        book_epoch,
        target_long_edge,
        decode,
    );
}

fn reset_decode_ahead_policy_if_context_changed(
    policy: &mut DecodeAheadPolicy,
    source: &Option<SharedSource>,
    previous_book_id: Option<&str>,
    previous_decode: DecodeOptions,
    previous_target_long_edge: u32,
    current_decode: DecodeOptions,
    current_target_long_edge: u32,
) {
    let current_book_id = source.as_ref().map(|source| source.book_id());
    if previous_book_id != current_book_id
        || previous_decode != current_decode
        || previous_target_long_edge != current_target_long_edge
    {
        policy.reset_context();
    }
}

fn clear_source_bytes_cache_on_book_change(
    cache: &mut Option<SourceBytesCache>,
    source: &Option<SharedSource>,
    previous_book_id: Option<&str>,
) {
    let current_book_id = source.as_ref().map(|source| source.book_id());
    if previous_book_id != current_book_id {
        if let Some(cache) = cache {
            cache.clear();
        }
    }
}

fn apply_command(
    command: WorkerCommand,
    source: &mut Option<SharedSource>,
    center: &mut usize,
    direction: &mut NavigationDirection,
    target_long_edge: &mut u32,
    visible_pages: &mut usize,
    options: &mut WorkerOptions,
) -> bool {
    match command {
        WorkerCommand::LoadBook {
            source: new_source,
            center: new_center,
            direction: new_direction,
            target_long_edge: new_target_long_edge,
            visible_pages: new_visible_pages,
            options: new_options,
        } => {
            *source = Some(new_source);
            *center = new_center;
            *direction = new_direction;
            *target_long_edge = new_target_long_edge;
            *visible_pages = new_visible_pages.max(1);
            *options = new_options.normalized();
            true
        }
        WorkerCommand::SetPage {
            center: new_center,
            direction: new_direction,
            target_long_edge: new_target_long_edge,
            visible_pages: new_visible_pages,
            options: new_options,
        } => {
            *center = new_center;
            *direction = new_direction;
            *target_long_edge = new_target_long_edge;
            *visible_pages = new_visible_pages.max(1);
            *options = new_options.normalized();
            true
        }
        WorkerCommand::ClearBook { ack } => {
            *source = None;
            *center = 0;
            *direction = NavigationDirection::Forward;
            *target_long_edge = DEFAULT_TARGET_LONG_EDGE;
            *visible_pages = 1;
            *options = WorkerOptions::default();
            let _ = ack.send(());
            true
        }
        WorkerCommand::Shutdown => false,
    }
}

fn drain_latest_command(command_rx: &Receiver<WorkerCommand>) -> Option<WorkerCommand> {
    let mut latest = None;
    while let Ok(command) = command_rx.try_recv() {
        latest = Some(command);
    }
    latest
}
