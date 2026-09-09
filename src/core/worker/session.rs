//! One transition boundary for source identity, request parameters, and work
//! ownership. Encoded bytes survive size/policy changes within the same snapshot.
use super::cache::{
    clear_cache_on_source_or_decode_change, clear_published_app_cache_hints_on_context_change,
    prune_worker_cache, update_book_epoch, PublishedAppCacheHints,
};
use super::decode_ahead::{self, DecodeAhead};
use super::decode_policy::DecodeAheadPolicy;
use super::read_ahead::{self, ReadAhead};
use super::scheduler::PageJob;
use super::source_bytes::{SourceBytesCache, SourcePageBytes};
use super::{
    DecodeOptions, NavigationDirection, PreparedPage, WorkerCommand, WorkerOptions,
    DEFAULT_TARGET_LONG_EDGE,
};
use crate::core::source::{PageId, SharedSource};
use lru::LruCache;
use std::num::NonZeroUsize;
use std::sync::Arc;

struct CompletedRead {
    epoch: usize,
    page_id: PageId,
    bytes: Result<SourcePageBytes, String>,
}

struct CompletedPage {
    epoch: usize,
    page_id: PageId,
    decode: DecodeOptions,
    page: Arc<PreparedPage>,
}

pub(super) struct WorkerSession {
    pub source: Option<SharedSource>,
    pub center: usize,
    pub direction: NavigationDirection,
    pub target_long_edge: u32,
    pub visible_pages: usize,
    pub options: WorkerOptions,
    pub book_epoch: usize,
    pub cache: LruCache<String, Arc<PreparedPage>>,
    pub cache_bytes: usize,
    pub published_app_cache_hints: PublishedAppCacheHints,
    pub source_bytes_cache: Option<SourceBytesCache>,
    pub read_ahead: Option<ReadAhead>,
    pub decode_ahead: Option<DecodeAhead>,
    pub decode_ahead_policy: DecodeAheadPolicy,
    completed_read: Option<CompletedRead>,
    completed_page: Option<CompletedPage>,
}

impl Default for WorkerSession {
    fn default() -> Self {
        Self {
            source: None,
            center: 0,
            direction: NavigationDirection::Forward,
            target_long_edge: DEFAULT_TARGET_LONG_EDGE,
            visible_pages: 1,
            options: WorkerOptions::default(),
            book_epoch: 0,
            cache: LruCache::new(NonZeroUsize::new(12).unwrap()),
            cache_bytes: 0,
            published_app_cache_hints: PublishedAppCacheHints::new(),
            source_bytes_cache: SourceBytesCache::from_env(),
            read_ahead: None,
            decode_ahead: None,
            decode_ahead_policy: DecodeAheadPolicy::from_env(),
            completed_read: None,
            completed_page: None,
        }
    }
}

impl WorkerSession {
    pub fn apply(&mut self, command: WorkerCommand) -> bool {
        let previous_book_id = self
            .source
            .as_ref()
            .map(|source| source.book_id().to_owned());
        let previous_instance = self
            .source
            .as_ref()
            .map(|source| source.source_instance_id());
        let previous_cache_id = self.source.as_ref().map(|source| source.source_cache_id());
        let previous_decode = self.options.decode;
        let previous_target = self.target_long_edge;
        let mut clear_ack = None;
        match command {
            WorkerCommand::LoadBook {
                source,
                center,
                direction,
                target_long_edge,
                visible_pages,
                options,
            } => {
                self.source = Some(source);
                self.set_request(center, direction, target_long_edge, visible_pages, options);
            }
            WorkerCommand::SetPage {
                center,
                direction,
                target_long_edge,
                visible_pages,
                options,
            } => {
                self.set_request(center, direction, target_long_edge, visible_pages, options);
            }
            WorkerCommand::ClearBook { ack } => {
                self.source = None;
                self.set_request(
                    0,
                    NavigationDirection::Forward,
                    DEFAULT_TARGET_LONG_EDGE,
                    1,
                    WorkerOptions::default(),
                );
                clear_ack = Some(ack);
            }
            WorkerCommand::Shutdown => return false,
        }
        update_book_epoch(
            &mut self.book_epoch,
            &self.source,
            previous_book_id.as_deref(),
            previous_instance,
        );
        clear_published_app_cache_hints_on_context_change(
            &self.source,
            previous_book_id.as_deref(),
            previous_cache_id,
            previous_decode,
            previous_target,
            self.options.decode,
            self.target_long_edge,
            &mut self.published_app_cache_hints,
        );
        clear_cache_on_source_or_decode_change(
            &self.source,
            previous_book_id.as_deref(),
            previous_cache_id,
            previous_decode,
            self.options.decode,
            &mut self.cache,
            &mut self.cache_bytes,
        );
        let source_changed = previous_book_id.as_deref()
            != self.source.as_ref().map(|s| s.book_id())
            || previous_cache_id != self.source.as_ref().map(|s| s.source_cache_id());
        if source_changed {
            if let Some(cache) = &mut self.source_bytes_cache {
                cache.clear();
            }
        }
        if source_changed
            || previous_decode != self.options.decode
            || previous_target != self.target_long_edge
        {
            self.decode_ahead_policy.reset_context();
        }
        prune_worker_cache(
            &mut self.cache,
            &mut self.cache_bytes,
            self.options.cache_bytes,
        );
        decode_ahead::clear_pending_decode_if_context_changed(
            &mut self.decode_ahead,
            self.source.as_ref().map(|s| s.book_id()),
            self.book_epoch,
            self.target_long_edge,
            self.options.decode,
        );
        if self.source.is_none() {
            read_ahead::cancel_pending(&mut self.read_ahead, "clear_book");
            self.completed_read = None;
            self.completed_page = None;
        }
        if let Some(ack) = clear_ack {
            let _ = ack.send(());
        }
        true
    }

    fn set_request(
        &mut self,
        center: usize,
        direction: NavigationDirection,
        target: u32,
        visible: usize,
        options: WorkerOptions,
    ) {
        self.center = center;
        self.direction = direction;
        self.target_long_edge = target;
        self.visible_pages = visible.max(1);
        self.options = options.normalized();
    }

    pub fn reconcile(&mut self, source: &SharedSource, jobs: &[PageJob]) {
        let contains = |id| jobs.iter().any(|job| source.page_id(job.index) == Some(id));
        if self
            .completed_read
            .as_ref()
            .is_some_and(|read| read.epoch != self.book_epoch || !contains(read.page_id))
        {
            self.completed_read = None;
        }
        if self.completed_page.as_ref().is_some_and(|prepared| {
            prepared.epoch != self.book_epoch
                || prepared.decode != self.options.decode
                || !jobs.iter().any(|job| {
                    source.page_id(job.index) == Some(prepared.page_id)
                        && job.target_long_edge == prepared.page.target_long_edge
                })
        }) {
            self.completed_page = None;
        }
        read_ahead::cancel_if_not_scheduled(&mut self.read_ahead, source, self.book_epoch, jobs);
        decode_ahead::cancel_pending_decode_if_not_scheduled(
            &mut self.decode_ahead,
            source,
            source.book_id(),
            self.book_epoch,
            jobs,
            self.center,
            self.visible_pages,
            &self.options,
            &self.cache,
            &self.published_app_cache_hints,
        );
    }

    pub fn keep_read(&mut self, page_id: PageId, bytes: Result<SourcePageBytes, String>) {
        self.completed_read = Some(CompletedRead {
            epoch: self.book_epoch,
            page_id,
            bytes,
        });
    }

    pub fn take_read(&mut self, page_id: PageId) -> Option<Result<SourcePageBytes, String>> {
        if self
            .completed_read
            .as_ref()
            .is_some_and(|read| read.epoch == self.book_epoch && read.page_id == page_id)
        {
            self.completed_read.take().map(|read| read.bytes)
        } else {
            None
        }
    }

    pub fn keep_page(&mut self, page_id: PageId, decode: DecodeOptions, page: Arc<PreparedPage>) {
        // Cached pixels already survive replanning. Do not let their repeated
        // blocked publication displace the only copy of an uncached result.
        if self.completed_page.is_some()
            && self
                .cache
                .iter()
                .any(|(_, cached)| Arc::ptr_eq(cached, &page))
        {
            return;
        }
        self.completed_page = Some(CompletedPage {
            epoch: self.book_epoch,
            page_id,
            decode,
            page,
        });
    }

    pub fn take_page(&mut self, page_id: PageId, target: u32) -> Option<Arc<PreparedPage>> {
        if self.completed_page.as_ref().is_some_and(|prepared| {
            prepared.epoch == self.book_epoch
                && prepared.page_id == page_id
                && prepared.decode == self.options.decode
                && prepared.page.target_long_edge == target
        }) {
            self.completed_page.take().map(|prepared| prepared.page)
        } else {
            None
        }
    }

    pub fn has_completed_work(&self) -> bool {
        self.completed_read.is_some() || self.completed_page.is_some()
    }

    pub fn finish_schedule(&mut self) {
        self.completed_read = None;
        self.completed_page = None;
    }
}

impl Drop for WorkerSession {
    fn drop(&mut self) {
        read_ahead::clear_pending(&mut self.read_ahead, "worker_exit");
        decode_ahead::clear_pending_decode(&mut self.decode_ahead, "worker_exit");
    }
}
