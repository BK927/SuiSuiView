use super::{ai_prefetch_pages_for, gpu_paint, SuiSuiViewApp};
#[cfg(any(test, feature = "perf-dev", feature = "perf-diagnostics"))]
use super::perf;
use crate::core::effects::ViewEffects;
use crate::core::state::{AppSettings, CacheMemoryMode, DisplayUpscaler, FitMode};
use crate::core::worker::{
    clamp_target_long_edge, preview_prefetch_indices, CachedPageKey, DecodeOptions,
    PREVIEW_TARGET_LONG_EDGE,
};
use eframe::egui::{Rect, TextureHandle};
use lru::LruCache;
use std::collections::HashSet;
use std::sync::{Arc, OnceLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(in crate::app) struct PageCacheKey {
    pub(in crate::app) index: usize,
    pub(in crate::app) target_long_edge: u32,
    pub(in crate::app) decode: DecodeOptions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(in crate::app) struct TextureCacheKey {
    pub(in crate::app) page: PageCacheKey,
    pub(in crate::app) effects: ViewEffects,
    pub(in crate::app) upscaled: bool,
}

pub(in crate::app) struct TextureEntry {
    pub(in crate::app) texture: TextureHandle,
    pub(in crate::app) byte_size: usize,
}

impl SuiSuiViewApp {
    pub(in crate::app) fn app_cached_page_keys(&self) -> Vec<CachedPageKey> {
        self.decoded_pages
            .iter()
            .map(|(key, _)| CachedPageKey::new(key.index, key.target_long_edge, key.decode))
            .collect()
    }

    pub(in crate::app) fn cpu_cache_budget_bytes(&self) -> usize {
        cache_budget_bytes(&self.settings)
    }

    pub(in crate::app) fn worker_cache_budget_bytes(&self) -> usize {
        worker_cache_budget_bytes_for(
            self.cpu_cache_budget_bytes(),
            self.target_long_edge,
            self.visible_page_count(),
        )
    }

    pub(in crate::app) fn insert_prepared_page(
        &mut self,
        key: PageCacheKey,
        page: Arc<crate::core::worker::PreparedPage>,
    ) {
        self.drop_lower_resolution_pages_for(key);
        if let Some((evicted_key, evicted_page)) = self.decoded_pages.push(key, page.clone()) {
            self.decoded_bytes = self.decoded_bytes.saturating_sub(evicted_page.byte_size);
            self.drop_textures_for_page(evicted_key);
        }
        self.decoded_bytes = self.decoded_bytes.saturating_add(page.byte_size);
    }

    pub(in crate::app) fn insert_upscaled_page(
        &mut self,
        key: PageCacheKey,
        page: Arc<crate::core::worker::PreparedPage>,
    ) {
        if let Some((evicted_key, evicted_page)) = self.upscaled_pages.push(key, page.clone()) {
            self.upscaled_bytes = self.upscaled_bytes.saturating_sub(evicted_page.byte_size);
            self.drop_textures_for_page(evicted_key);
        }
        self.upscaled_bytes = self.upscaled_bytes.saturating_add(page.byte_size);
        self.prune_upscaled_cache();
    }

    pub(in crate::app) fn drop_textures_for_page(&mut self, page: PageCacheKey) {
        let stale_keys = self
            .textures
            .iter()
            .filter_map(|(key, _entry)| (key.page == page).then_some(*key))
            .collect::<Vec<_>>();
        for key in stale_keys {
            let _ = self.textures.pop(&key);
        }
    }

    fn drop_lower_resolution_pages_for(&mut self, key: PageCacheKey) {
        let stale_keys = lower_resolution_page_keys(&self.decoded_pages, key);
        for stale_key in stale_keys {
            if let Some(page) = self.decoded_pages.pop(&stale_key) {
                self.decoded_bytes = self.decoded_bytes.saturating_sub(page.byte_size);
                self.drop_textures_for_page(stale_key);
            }
        }
    }

    pub(in crate::app) fn prune_decoded_cache(&mut self) {
        let pinned = self.pinned_page_indices();
        let mut retained = Vec::new();
        let max_pops = self.decoded_pages.len();
        let mut pops = 0usize;

        let budget_bytes = self.cpu_cache_budget_bytes();
        while self.decoded_bytes > budget_bytes && pops < max_pops {
            let Some((key, page)) = self.decoded_pages.pop_lru() else {
                break;
            };
            pops += 1;

            if pinned.contains(&key) {
                retained.push((key, page));
                continue;
            }

            self.decoded_bytes = self.decoded_bytes.saturating_sub(page.byte_size);
            self.drop_textures_for_page(key);
        }

        for (key, page) in retained {
            self.decoded_pages.put(key, page);
        }
    }

    pub(in crate::app) fn prune_upscaled_cache(&mut self) {
        let pinned = self.pinned_upscaled_page_indices();
        let mut retained = Vec::new();
        let max_pops = self.upscaled_pages.len();
        let mut pops = 0usize;
        let budget_bytes = upscaled_cache_budget_bytes_for(self.cpu_cache_budget_bytes());
        while self.upscaled_bytes > budget_bytes && pops < max_pops {
            let Some((key, page)) = self.upscaled_pages.pop_lru() else {
                break;
            };
            pops += 1;

            if pinned.contains(&key) {
                retained.push((key, page));
                continue;
            }

            self.upscaled_bytes = self.upscaled_bytes.saturating_sub(page.byte_size);
            self.drop_textures_for_page(key);
        }

        for (key, page) in retained {
            self.upscaled_pages.put(key, page);
        }
    }

    pub(in crate::app) fn prune_texture_cache(&mut self) {
        let budget_bytes = self.texture_cache_budget_bytes();
        let mut texture_bytes = self.texture_cache_bytes();
        if texture_bytes <= budget_bytes {
            return;
        }

        let pinned_pages = self.pinned_page_indices();
        let mut retained = Vec::new();
        let max_pops = self.textures.len();
        let mut pops = 0usize;
        while texture_bytes > budget_bytes && pops < max_pops {
            let Some((key, entry)) = self.textures.pop_lru() else {
                break;
            };
            pops += 1;

            if pinned_pages.contains(&key.page) {
                retained.push((key, entry));
                continue;
            }

            texture_bytes = texture_bytes.saturating_sub(entry.byte_size);
        }

        for (key, entry) in retained {
            self.textures.put(key, entry);
        }
    }

    #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
    pub(in crate::app) fn record_cache_snapshot(&self, reason: &'static str) {
        perf::record_app_cache_snapshot(perf::AppCacheSnapshot {
            reason,
            current_page: self.current_page,
            target_long_edge: self.target_long_edge,
            decoded_pages: self.decoded_pages.len(),
            decoded_bytes: self.decoded_bytes,
            decoded_budget_bytes: self.cpu_cache_budget_bytes(),
            upscaled_pages: self.upscaled_pages.len(),
            upscaled_bytes: self.upscaled_bytes,
            upscaled_budget_bytes: upscaled_cache_budget_bytes_for(self.cpu_cache_budget_bytes()),
            textures: self.textures.len(),
            texture_bytes: self.texture_cache_bytes(),
        });
    }

    pub(in crate::app) fn texture_cache_bytes(&self) -> usize {
        self.textures
            .iter()
            .map(|(_key, entry)| entry.byte_size)
            .sum()
    }

    pub(in crate::app) fn texture_cache_budget_bytes(&self) -> usize {
        texture_cache_budget_bytes_for(
            self.target_long_edge,
            self.visible_page_count(),
            self.transition.is_some(),
        )
    }

    fn pinned_page_indices(&self) -> HashSet<PageCacheKey> {
        let mut pinned = self.pin_keys_for_indices(&self.spread_indices(), self.target_long_edge);
        pinned.extend(self.debug_compare_pin_keys());
        if let Some(transition) = self.transition.as_ref() {
            pinned.extend(
                self.pin_keys_for_indices(&transition.from_indices, transition.target_long_edge),
            );
        }
        let preview_budget = self
            .cpu_cache_budget_bytes()
            .saturating_sub(self.cached_decoded_bytes_for_keys(&pinned));
        pinned.extend(self.preview_prefetch_pin_keys(&pinned, preview_budget));
        pinned
    }

    fn preview_prefetch_pin_keys(
        &self,
        already_pinned: &HashSet<PageCacheKey>,
        budget_bytes: usize,
    ) -> HashSet<PageCacheKey> {
        if !self.settings.prefetch_enabled
            || !self.settings.progressive_preview_enabled
            || self.target_long_edge <= PREVIEW_TARGET_LONG_EDGE
        {
            return HashSet::new();
        }
        let Some(source) = self.source.as_ref() else {
            return HashSet::new();
        };

        let indices = preview_prefetch_indices(
            self.worker_center_page(),
            source.page_count(),
            self.last_nav_direction,
            self.visible_page_count(),
        );
        self.preview_pin_keys_for_indices(&indices, already_pinned, budget_bytes)
    }

    fn pinned_upscaled_page_indices(&self) -> HashSet<PageCacheKey> {
        let mut pinned = self.pin_keys_for_indices(&self.spread_indices(), self.target_long_edge);
        if let Some(source) = self.source.as_ref() {
            let mode = self.settings.ai_upscale.prefetch_mode;
            let prefetch_pages = ai_prefetch_pages_for(
                self.current_page,
                source.page_count(),
                self.view_mode.step(),
                self.last_nav_direction,
                mode,
            );
            pinned.extend(self.pin_keys_for_indices(&prefetch_pages, self.target_long_edge));
        }
        pinned
    }

    fn pin_keys_for_indices(
        &self,
        indices: &[usize],
        target_long_edge: u32,
    ) -> HashSet<PageCacheKey> {
        let mut keys = HashSet::with_capacity(indices.len() * 2);
        for index in indices {
            keys.insert(PageCacheKey {
                index: *index,
                target_long_edge,
                decode: self.decode_options(),
            });
            if self.settings.progressive_preview_enabled
                && target_long_edge > PREVIEW_TARGET_LONG_EDGE
            {
                keys.insert(PageCacheKey {
                    index: *index,
                    target_long_edge: PREVIEW_TARGET_LONG_EDGE,
                    decode: self.decode_options(),
                });
            }
        }
        keys
    }

    fn preview_pin_keys_for_indices(
        &self,
        indices: &[usize],
        already_pinned: &HashSet<PageCacheKey>,
        budget_bytes: usize,
    ) -> HashSet<PageCacheKey> {
        let mut keys = HashSet::with_capacity(indices.len());
        let mut pinned_bytes = 0usize;
        for index in indices {
            let key = PageCacheKey {
                index: *index,
                target_long_edge: PREVIEW_TARGET_LONG_EDGE,
                decode: self.decode_options(),
            };
            if already_pinned.contains(&key) {
                continue;
            }
            let Some(byte_size) = self.decoded_page_byte_size(key) else {
                continue;
            };
            if pinned_bytes.saturating_add(byte_size) > budget_bytes {
                continue;
            }
            pinned_bytes = pinned_bytes.saturating_add(byte_size);
            keys.insert(key);
        }
        keys
    }

    fn cached_decoded_bytes_for_keys(&self, keys: &HashSet<PageCacheKey>) -> usize {
        self.decoded_pages
            .iter()
            .filter_map(|(key, page)| keys.contains(key).then_some(page.byte_size))
            .sum()
    }

    fn decoded_page_byte_size(&self, requested: PageCacheKey) -> Option<usize> {
        self.decoded_pages
            .iter()
            .find_map(|(key, page)| (*key == requested).then_some(page.byte_size))
    }

    pub(in crate::app) fn best_page_key(&self, requested: PageCacheKey) -> Option<PageCacheKey> {
        if !self.settings.progressive_preview_enabled
            && requested.target_long_edge != PREVIEW_TARGET_LONG_EDGE
        {
            return self.decoded_pages.peek(&requested).map(|_| requested);
        }
        best_page_key_in_cache(&self.decoded_pages, requested)
    }

    pub(in crate::app) fn best_upscaled_page_key(
        &self,
        requested: PageCacheKey,
    ) -> Option<PageCacheKey> {
        best_page_key_at_or_below_in_cache(&self.upscaled_pages, requested)
    }

    #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
    pub(in crate::app) fn page_turn_cache_state(
        &self,
        requested: PageCacheKey,
    ) -> perf::PageCacheState {
        if let Some(key) = self.preferred_upscaled_page_key(requested) {
            return page_cache_state_from_hit(Some(key), requested, true);
        }
        if let Some(key) = self.best_page_key(requested) {
            return page_cache_state_from_hit(Some(key), requested, false);
        }
        perf::PageCacheState::Miss
    }

    pub(in crate::app) fn preferred_upscaled_page_key(
        &self,
        requested: PageCacheKey,
    ) -> Option<PageCacheKey> {
        self.use_ai_upscaled_pages
            .then(|| best_page_key_at_or_below_in_cache(&self.upscaled_pages, requested))
            .flatten()
    }
}

pub(in crate::app) fn cache_budget_bytes(settings: &AppSettings) -> usize {
    match settings.cache_memory_mode {
        CacheMemoryMode::Auto => automatic_cache_budget_bytes(),
        CacheMemoryMode::Manual => {
            (settings.manual_cache_mb.clamp(64, 2048) as usize) * 1024 * 1024
        }
    }
}

pub(in crate::app) fn should_allow_cpu_display_upscale(
    fit_mode: FitMode,
    manual_zoom: f32,
    gpu_display_upscale_can_own_upscale: bool,
) -> bool {
    let _ = (fit_mode, manual_zoom, gpu_display_upscale_can_own_upscale);
    false
}

pub(in crate::app) fn gpu_visual_needs_wgsl(
    image_size: [usize; 2],
    target_size: [u32; 2],
    effects: ViewEffects,
    display_upscaler: DisplayUpscaler,
) -> bool {
    effects != ViewEffects::default()
        || display_upscaler
            .resolve_for_render(image_size, target_size)
            .is_some()
}

pub(in crate::app) fn rect_target_size(rect: Rect) -> [u32; 2] {
    [
        rect.width().round().max(1.0) as u32,
        rect.height().round().max(1.0) as u32,
    ]
}

fn automatic_cache_budget_bytes() -> usize {
    static AUTO_CACHE_BUDGET: OnceLock<usize> = OnceLock::new();
    *AUTO_CACHE_BUDGET.get_or_init(|| {
        let total = sysinfo::System::new_with_specifics(
            sysinfo::RefreshKind::nothing()
                .with_memory(sysinfo::MemoryRefreshKind::nothing().with_ram()),
        )
        .total_memory() as usize;
        automatic_cache_budget_bytes_for_total(total)
    })
}

pub(in crate::app) fn automatic_cache_budget_bytes_for_total(total_memory_bytes: usize) -> usize {
    let target = total_memory_bytes / 100;
    target.clamp(64 * 1024 * 1024, 96 * 1024 * 1024)
}

pub(in crate::app) const BYTES_PER_RGBA_PIXEL: usize = 4;
const MIN_WORKER_CACHE_BYTES: usize = 32 * 1024 * 1024;
const MAX_WORKER_CACHE_BYTES: usize = 48 * 1024 * 1024;
const MAX_UPSCALED_CACHE_BYTES: usize = 256 * 1024 * 1024;
const MIN_TEXTURE_CACHE_BYTES: usize = 64 * 1024 * 1024;
const MAX_TEXTURE_CACHE_BYTES: usize = 128 * 1024 * 1024;
const PREFETCH_FORWARD_PAGES: usize = 3;
const PREFETCH_BACKWARD_PAGES: usize = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CacheBudgetSummary {
    pub(super) cpu_prepared_bytes: usize,
    pub(super) worker_prefetch_bytes: usize,
    pub(super) upscaled_bytes: usize,
    pub(super) gpu_source_texture_bytes: usize,
    pub(super) gpu_intermediate_texture_bytes: usize,
    pub(super) estimated_page_bytes: usize,
    pub(super) estimated_cpu_pages: usize,
    pub(super) estimated_worker_pages: usize,
}

pub(super) fn cache_budget_summary(
    settings: &AppSettings,
    target_long_edge: u32,
    visible_pages: usize,
) -> CacheBudgetSummary {
    let cpu_prepared_bytes = cache_budget_bytes(settings);
    let estimated_page_bytes = estimated_page_bytes_for_target(target_long_edge);
    let worker_prefetch_bytes =
        worker_cache_budget_bytes_for(cpu_prepared_bytes, target_long_edge, visible_pages);
    let upscaled_bytes = upscaled_cache_budget_bytes_for(cpu_prepared_bytes);
    CacheBudgetSummary {
        cpu_prepared_bytes,
        worker_prefetch_bytes,
        upscaled_bytes,
        gpu_source_texture_bytes: gpu_paint::GPU_SOURCE_TEXTURE_BUDGET_BYTES,
        gpu_intermediate_texture_bytes: gpu_paint::GPU_INTERMEDIATE_TEXTURE_BUDGET_BYTES,
        estimated_page_bytes,
        estimated_cpu_pages: estimated_page_capacity(cpu_prepared_bytes, estimated_page_bytes),
        estimated_worker_pages: estimated_page_capacity(
            worker_prefetch_bytes,
            estimated_page_bytes,
        ),
    }
}

fn worker_cache_budget_bytes_for(
    cpu_budget_bytes: usize,
    target_long_edge: u32,
    visible_pages: usize,
) -> usize {
    let nearby_page_goal = visible_pages
        .max(1)
        .saturating_add(PREFETCH_FORWARD_PAGES)
        .saturating_add(PREFETCH_BACKWARD_PAGES);
    let desired_prefetch_bytes =
        estimated_page_bytes_for_target(target_long_edge).saturating_mul(nearby_page_goal);
    let cpu_bounded_goal = desired_prefetch_bytes.min(cpu_budget_bytes.max(MIN_WORKER_CACHE_BYTES));
    (cpu_budget_bytes / 2)
        .max(cpu_bounded_goal)
        .clamp(MIN_WORKER_CACHE_BYTES, MAX_WORKER_CACHE_BYTES)
}

fn upscaled_cache_budget_bytes_for(cpu_budget_bytes: usize) -> usize {
    (cpu_budget_bytes / 2).clamp(MIN_WORKER_CACHE_BYTES, MAX_UPSCALED_CACHE_BYTES)
}

pub(in crate::app) fn texture_cache_budget_bytes_for(
    target_long_edge: u32,
    visible_page_count: usize,
    transition_active: bool,
) -> usize {
    let visible_page_count = visible_page_count.max(1);
    let transition_pages = if transition_active {
        visible_page_count
    } else {
        0
    };
    let texture_page_goal = visible_page_count
        .saturating_add(transition_pages)
        .saturating_add(1);
    estimated_page_bytes_for_target(target_long_edge)
        .saturating_mul(texture_page_goal)
        .clamp(MIN_TEXTURE_CACHE_BYTES, MAX_TEXTURE_CACHE_BYTES)
}

fn estimated_page_bytes_for_target(target_long_edge: u32) -> usize {
    let edge = clamp_target_long_edge(target_long_edge) as usize;
    edge.saturating_mul(edge)
        .saturating_mul(BYTES_PER_RGBA_PIXEL)
}

fn estimated_page_capacity(budget_bytes: usize, page_bytes: usize) -> usize {
    if page_bytes == 0 {
        return 0;
    }
    (budget_bytes / page_bytes).max(1)
}

pub(in crate::app) fn best_page_key_in_cache(
    cache: &LruCache<PageCacheKey, Arc<crate::core::worker::PreparedPage>>,
    requested: PageCacheKey,
) -> Option<PageCacheKey> {
    if cache.peek(&requested).is_some() {
        return Some(requested);
    }

    let mut best_smaller = None;
    let mut smallest_any = None;
    for (key, _page) in cache.iter() {
        if key.index != requested.index || key.decode != requested.decode {
            continue;
        }
        if key.target_long_edge <= requested.target_long_edge
            && best_smaller
                .is_none_or(|best: PageCacheKey| key.target_long_edge > best.target_long_edge)
        {
            best_smaller = Some(*key);
        }
        if smallest_any
            .is_none_or(|smallest: PageCacheKey| key.target_long_edge < smallest.target_long_edge)
        {
            smallest_any = Some(*key);
        }
    }

    best_smaller.or(smallest_any)
}

pub(in crate::app) fn best_page_key_at_or_below_in_cache(
    cache: &LruCache<PageCacheKey, Arc<crate::core::worker::PreparedPage>>,
    requested: PageCacheKey,
) -> Option<PageCacheKey> {
    if cache.peek(&requested).is_some() {
        return Some(requested);
    }

    cache
        .iter()
        .filter_map(|(key, _page)| {
            (key.index == requested.index
                && key.decode == requested.decode
                && key.target_long_edge <= requested.target_long_edge)
                .then_some(*key)
        })
        .max_by_key(|key| key.target_long_edge)
}

pub(in crate::app) fn lower_resolution_page_keys(
    cache: &LruCache<PageCacheKey, Arc<crate::core::worker::PreparedPage>>,
    inserted: PageCacheKey,
) -> Vec<PageCacheKey> {
    cache
        .iter()
        .filter_map(|(key, _page)| {
            (key.index == inserted.index
                && key.decode == inserted.decode
                && key.target_long_edge < inserted.target_long_edge)
                .then_some(*key)
        })
        .collect()
}

#[cfg(any(test, feature = "perf-dev", feature = "perf-diagnostics"))]
pub(in crate::app) fn page_cache_state_from_hit(
    hit: Option<PageCacheKey>,
    requested: PageCacheKey,
    upscaled: bool,
) -> perf::PageCacheState {
    use std::cmp::Ordering;

    let Some(hit) = hit else {
        return perf::PageCacheState::Miss;
    };
    match (
        upscaled,
        hit.target_long_edge.cmp(&requested.target_long_edge),
    ) {
        (true, Ordering::Equal) => perf::PageCacheState::UpscaledExact,
        (true, Ordering::Less) => perf::PageCacheState::UpscaledPreview,
        (true, Ordering::Greater) => perf::PageCacheState::UpscaledFallback,
        (false, Ordering::Equal) => perf::PageCacheState::DecodedExact,
        (false, Ordering::Less) => perf::PageCacheState::DecodedPreview,
        (false, Ordering::Greater) => perf::PageCacheState::DecodedFallback,
    }
}

#[cfg(test)]
pub(in crate::app) fn preferred_page_key_in_cache(
    cache: &LruCache<PageCacheKey, Arc<crate::core::worker::PreparedPage>>,
    requested: PageCacheKey,
    enabled: bool,
) -> Option<PageCacheKey> {
    enabled
        .then(|| best_page_key_in_cache(cache, requested))
        .flatten()
}
