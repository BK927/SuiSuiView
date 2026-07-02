#[cfg(any(test, feature = "perf-dev", feature = "perf-diagnostics"))]
use super::perf;
use super::SuiSuiViewApp;
use crate::core::effects::ViewEffects;
use crate::core::state::{
    AppSettings, CacheMemoryMode, CpuScaleFilter, FitMode, WgpuDownscaleMethod, WgpuScalePlan,
    WgpuUpscaleMethod, MANUAL_CACHE_MB_MAX, MANUAL_CACHE_MB_MIN,
};
use crate::core::worker::{
    clamp_target_long_edge, preview_prefetch_indices, CachedPageKey, DecodeOptions,
    NavigationDirection, PreparedTargetIntent, FULL_QUALITY_PREFETCH_BACKWARD_PAGES,
    FULL_QUALITY_PREFETCH_FORWARD_PAGES, MAX_TARGET_LONG_EDGE, PREVIEW_TARGET_LONG_EDGE,
};
use egui::{Rect, TextureHandle};
use lru::LruCache;
use std::collections::HashSet;
use std::sync::{Arc, OnceLock};

mod original;

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
    pub(in crate::app) sampling: TextureSampling,
}

pub(in crate::app) struct TextureEntry {
    pub(in crate::app) texture: TextureHandle,
    pub(in crate::app) byte_size: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(in crate::app) enum TextureSampling {
    Linear,
    Nearest,
}

impl TextureSampling {
    pub(in crate::app) fn for_target_intent(intent: PreparedTargetIntent) -> Self {
        if intent.is_original_inspection() {
            Self::Nearest
        } else {
            Self::Linear
        }
    }
}

impl SuiSuiViewApp {
    pub(in crate::app) fn current_prepared_target_intent(&self) -> PreparedTargetIntent {
        self.prepared_target_intent_for_target(self.target_long_edge)
    }

    pub(in crate::app) fn prepared_target_intent_for_target(
        &self,
        target_long_edge: u32,
    ) -> PreparedTargetIntent {
        prepared_target_intent_for_view(self.fit_mode, self.manual_zoom, target_long_edge)
    }

    pub(in crate::app) fn texture_sampling_for_page_key(
        &self,
        key: PageCacheKey,
    ) -> TextureSampling {
        TextureSampling::for_target_intent(
            self.prepared_target_intent_for_target(key.target_long_edge),
        )
    }

    pub(in crate::app) fn schedule_high_target_cleanup_if_leaving_target_intent(
        &mut self,
        previous_intent: PreparedTargetIntent,
    ) {
        if previous_intent.keeps_exact_prefetch_lightweight()
            && self.current_prepared_target_intent() == PreparedTargetIntent::NormalNavigation
        {
            let ctx = self.egui_ctx.clone();
            self.schedule_original_inspection_cache_cleanup(&ctx);
        }
    }

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
        if key.target_long_edge > MAX_TARGET_LONG_EDGE {
            let visible = self.spread_indices();
            let decode = self.decode_options();
            touch_normal_navigation_page_keys(&mut self.decoded_pages, &visible, decode);
        }
        if self
            .decoded_pages
            .get(&key)
            .is_some_and(|cached| Arc::ptr_eq(cached, &page))
        {
            return;
        }
        if let Some((evicted_key, evicted_page)) = self.decoded_pages.push(key, page.clone()) {
            self.decoded_bytes = self.decoded_bytes.saturating_sub(evicted_page.byte_size);
            self.drop_textures_for_page(evicted_key);
        }
        self.decoded_bytes = self.decoded_bytes.saturating_add(page.byte_size);
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
        if self.target_long_edge > MAX_TARGET_LONG_EDGE {
            pinned.extend(self.normal_navigation_pin_keys_for_visible_pages());
        }
        pinned.extend(self.full_quality_prefetch_pin_keys());
        pinned.extend(self.queued_page_turn_pin_keys());
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

    fn full_quality_prefetch_pin_keys(&self) -> HashSet<PageCacheKey> {
        if !self.settings.prefetch_enabled || self.target_long_edge > MAX_TARGET_LONG_EDGE {
            return HashSet::new();
        }
        let Some(source) = self.source.as_ref() else {
            return HashSet::new();
        };
        let indices = full_quality_prefetch_indices(
            self.worker_center_page(),
            source.page_count(),
            self.last_nav_direction,
        );
        self.exact_prefetch_pin_keys_for_indices(&indices, self.exact_prefetch_pin_budget_bytes())
    }

    fn queued_page_turn_pin_keys(&self) -> HashSet<PageCacheKey> {
        let Some(source) = self.source.as_ref() else {
            return HashSet::new();
        };
        if source.page_count() == 0 {
            return HashSet::new();
        }

        let mut keys = HashSet::new();
        let mut page = self.current_page;
        let mut direction = None;
        if let Some(pending) = self.pending_page_turn {
            page = pending.target;
            direction = Some(pending.direction);
            keys.extend(
                self.pin_keys_for_indices(&self.spread_indices_for(page), self.target_long_edge),
            );
        }

        let Some(queued) = self.queued_page_turns else {
            return keys;
        };
        let direction = direction.unwrap_or(queued.direction);
        if queued.direction != direction {
            return keys;
        }

        let mut queued_indices = Vec::new();
        for _ in 0..queued.remaining.min(MAX_QUEUED_PINNED_PAGE_TURNS) {
            let Some(next_page) = self.page_turn_target_from(page, direction) else {
                break;
            };
            page = next_page;
            queued_indices.extend(self.spread_indices_for(page));
        }
        keys.extend(self.exact_prefetch_pin_keys_for_indices(
            &queued_indices,
            self.exact_prefetch_pin_budget_bytes(),
        ));
        keys
    }

    fn exact_prefetch_pin_budget_bytes(&self) -> usize {
        self.cpu_cache_budget_bytes()
            .saturating_mul(3)
            .clamp(MIN_EXACT_PREFETCH_PIN_BYTES, MAX_EXACT_PREFETCH_PIN_BYTES)
    }

    fn exact_prefetch_pin_keys_for_indices(
        &self,
        indices: &[usize],
        budget_bytes: usize,
    ) -> HashSet<PageCacheKey> {
        let mut keys = HashSet::new();
        let mut pinned_bytes = 0usize;
        for index in indices {
            let key = PageCacheKey {
                index: *index,
                target_long_edge: self.target_long_edge,
                decode: self.decode_options(),
            };
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

    fn preview_prefetch_pin_keys(
        &self,
        already_pinned: &HashSet<PageCacheKey>,
        budget_bytes: usize,
    ) -> HashSet<PageCacheKey> {
        if !self.settings.prefetch_enabled
            || !self.settings.progressive_preview_enabled
            || self.target_long_edge > MAX_TARGET_LONG_EDGE
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
                && target_long_edge <= MAX_TARGET_LONG_EDGE
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

    fn normal_navigation_pin_keys_for_visible_pages(&self) -> HashSet<PageCacheKey> {
        normal_navigation_page_keys_in_cache(
            &self.decoded_pages,
            &self.spread_indices(),
            self.decode_options(),
        )
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
        if self.settings.progressive_preview_enabled {
            best_page_key_in_cache(&self.decoded_pages, requested)
        } else {
            best_page_key_excluding_preview_fallback_in_cache(&self.decoded_pages, requested)
        }
    }

    pub(in crate::app) fn final_quality_page_key(
        &self,
        requested: PageCacheKey,
    ) -> Option<PageCacheKey> {
        final_quality_page_key_in_cache(&self.decoded_pages, requested)
    }

    #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
    pub(in crate::app) fn page_turn_cache_state(
        &self,
        requested: PageCacheKey,
    ) -> perf::PageCacheState {
        if let Some(key) = self.final_quality_page_key(requested) {
            return page_cache_state_from_hit(Some(key), requested);
        }
        perf::PageCacheState::Miss
    }
}

pub(in crate::app) fn cache_budget_bytes(settings: &AppSettings) -> usize {
    match settings.cache_memory_mode {
        CacheMemoryMode::Auto => automatic_cache_budget_bytes(),
        CacheMemoryMode::Manual => {
            (settings
                .manual_cache_mb
                .clamp(MANUAL_CACHE_MB_MIN, MANUAL_CACHE_MB_MAX) as usize)
                * 1024
                * 1024
        }
    }
}

pub(in crate::app) fn should_allow_cpu_display_upscale(
    fit_mode: FitMode,
    manual_zoom: f32,
    gpu_display_upscale_can_own_upscale: bool,
    cpu_upscale_filter: CpuScaleFilter,
) -> bool {
    let _ = manual_zoom;
    // Manual zoom and original-size viewing never re-prepare at an enlarged size.
    if matches!(fit_mode, FitMode::Manual | FitMode::Original) {
        return false;
    }
    // In WGPU mode the GPU display upscaler owns the enlargement, so preparing an
    // already-enlarged page on the CPU would be wasted work and cache memory.
    if gpu_display_upscale_can_own_upscale {
        return false;
    }
    // A Bilinear CPU upscale is identical to the free hardware texture sampler, so
    // keep the page native and let the sampler enlarge it: same result, no large
    // upscaled page held in the cache.
    !matches!(cpu_upscale_filter, CpuScaleFilter::Bilinear)
}

pub(in crate::app) fn prepared_target_intent_for_view(
    fit_mode: FitMode,
    manual_zoom: f32,
    target_long_edge: u32,
) -> PreparedTargetIntent {
    match fit_mode {
        FitMode::Original => PreparedTargetIntent::OriginalInspection,
        FitMode::Manual if manual_zoom >= 1.0 => PreparedTargetIntent::OriginalInspection,
        FitMode::FitPage | FitMode::FitWidth | FitMode::FitHeight
            if target_long_edge > MAX_TARGET_LONG_EDGE =>
        {
            PreparedTargetIntent::LargeFitDisplay
        }
        FitMode::Manual | FitMode::FitPage | FitMode::FitWidth | FitMode::FitHeight => {
            PreparedTargetIntent::NormalNavigation
        }
    }
}

pub(in crate::app) fn gpu_visual_needs_wgsl(
    image_size: [usize; 2],
    target_size: [u32; 2],
    effects: ViewEffects,
    wgpu_upscale_method: WgpuUpscaleMethod,
    wgpu_downscale_method: WgpuDownscaleMethod,
) -> bool {
    let scale_plan = WgpuScalePlan::resolve(
        image_size,
        target_size,
        wgpu_upscale_method,
        wgpu_downscale_method,
    );
    effects != ViewEffects::default()
        || scale_plan.effective_upscale_method != WgpuUpscaleMethod::None
        || scale_plan.effective_downscale_method != WgpuDownscaleMethod::Bilinear
}

pub(in crate::app) fn rect_target_size(rect: Rect, pixels_per_point: f32) -> [u32; 2] {
    [
        (rect.width() * pixels_per_point).round().max(1.0) as u32,
        (rect.height() * pixels_per_point).round().max(1.0) as u32,
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
const MIN_TEXTURE_CACHE_BYTES: usize = 64 * 1024 * 1024;
const MAX_TEXTURE_CACHE_BYTES: usize = 128 * 1024 * 1024;
const MIN_EXACT_PREFETCH_PIN_BYTES: usize = 128 * 1024 * 1024;
const MAX_EXACT_PREFETCH_PIN_BYTES: usize = 192 * 1024 * 1024;
const MAX_QUEUED_PINNED_PAGE_TURNS: usize = 24;

fn worker_cache_budget_bytes_for(
    cpu_budget_bytes: usize,
    target_long_edge: u32,
    visible_pages: usize,
) -> usize {
    let nearby_page_goal = visible_pages
        .max(1)
        .saturating_add(FULL_QUALITY_PREFETCH_FORWARD_PAGES)
        .saturating_add(FULL_QUALITY_PREFETCH_BACKWARD_PAGES);
    let desired_prefetch_bytes =
        estimated_page_bytes_for_target(target_long_edge).saturating_mul(nearby_page_goal);
    let cpu_bounded_goal = desired_prefetch_bytes.min(cpu_budget_bytes.max(MIN_WORKER_CACHE_BYTES));
    (cpu_budget_bytes / 2)
        .max(cpu_bounded_goal)
        .clamp(MIN_WORKER_CACHE_BYTES, MAX_WORKER_CACHE_BYTES)
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

fn full_quality_prefetch_indices(
    center: usize,
    page_count: usize,
    direction: NavigationDirection,
) -> Vec<usize> {
    let mut indices = Vec::with_capacity(
        FULL_QUALITY_PREFETCH_FORWARD_PAGES
            .saturating_add(FULL_QUALITY_PREFETCH_BACKWARD_PAGES)
            .saturating_add(1),
    );
    push_prefetch_index(&mut indices, center, page_count);
    match direction {
        NavigationDirection::Forward => {
            for offset in 1..=FULL_QUALITY_PREFETCH_FORWARD_PAGES {
                if let Some(index) = center.checked_add(offset) {
                    push_prefetch_index(&mut indices, index, page_count);
                }
            }
            for offset in 1..=FULL_QUALITY_PREFETCH_BACKWARD_PAGES {
                if let Some(index) = center.checked_sub(offset) {
                    push_prefetch_index(&mut indices, index, page_count);
                }
            }
        }
        NavigationDirection::Backward => {
            for offset in 1..=FULL_QUALITY_PREFETCH_FORWARD_PAGES {
                if let Some(index) = center.checked_sub(offset) {
                    push_prefetch_index(&mut indices, index, page_count);
                }
            }
            for offset in 1..=FULL_QUALITY_PREFETCH_BACKWARD_PAGES {
                if let Some(index) = center.checked_add(offset) {
                    push_prefetch_index(&mut indices, index, page_count);
                }
            }
        }
    }
    indices
}

fn push_prefetch_index(indices: &mut Vec<usize>, index: usize, page_count: usize) {
    if index < page_count && !indices.contains(&index) {
        indices.push(index);
    }
}

pub(in crate::app) fn best_page_key_in_cache(
    cache: &LruCache<PageCacheKey, Arc<crate::core::worker::PreparedPage>>,
    requested: PageCacheKey,
) -> Option<PageCacheKey> {
    if let Some(final_key) = final_quality_page_key_in_cache(cache, requested) {
        return Some(final_key);
    }

    let mut best_smaller = None;
    let mut smallest_any = None;
    let requested_allows_original = requested.target_long_edge > MAX_TARGET_LONG_EDGE;
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
        if !requested_allows_original && key.target_long_edge > MAX_TARGET_LONG_EDGE {
            continue;
        }
        if smallest_any
            .is_none_or(|smallest: PageCacheKey| key.target_long_edge < smallest.target_long_edge)
        {
            smallest_any = Some(*key);
        }
    }

    best_smaller.or(smallest_any)
}

pub(in crate::app) fn best_page_key_excluding_preview_fallback_in_cache(
    cache: &LruCache<PageCacheKey, Arc<crate::core::worker::PreparedPage>>,
    requested: PageCacheKey,
) -> Option<PageCacheKey> {
    if cache.peek(&requested).is_some() {
        return Some(requested);
    }
    if requested.target_long_edge == PREVIEW_TARGET_LONG_EDGE {
        return best_page_key_in_cache(cache, requested);
    }
    if let Some(final_key) = final_quality_page_key_in_cache(cache, requested) {
        return Some(final_key);
    }

    cache
        .iter()
        .filter_map(|(key, _page)| {
            (key.index == requested.index
                && key.decode == requested.decode
                && key.target_long_edge > PREVIEW_TARGET_LONG_EDGE
                && key.target_long_edge <= requested.target_long_edge)
                .then_some(*key)
        })
        .max_by_key(|key| key.target_long_edge)
}

pub(in crate::app) fn final_quality_page_key_in_cache(
    cache: &LruCache<PageCacheKey, Arc<crate::core::worker::PreparedPage>>,
    requested: PageCacheKey,
) -> Option<PageCacheKey> {
    let requested_allows_original = requested.target_long_edge > MAX_TARGET_LONG_EDGE;
    cache
        .iter()
        .filter_map(|(key, _page)| {
            if key.index != requested.index || key.decode != requested.decode {
                return None;
            }
            if key.target_long_edge < requested.target_long_edge {
                return None;
            }
            if !requested_allows_original && key.target_long_edge > MAX_TARGET_LONG_EDGE {
                return None;
            }
            Some(*key)
        })
        .min_by_key(|key| key.target_long_edge)
}

pub(in crate::app) fn lower_resolution_page_keys(
    cache: &LruCache<PageCacheKey, Arc<crate::core::worker::PreparedPage>>,
    inserted: PageCacheKey,
) -> Vec<PageCacheKey> {
    let inserted_is_original = inserted.target_long_edge > MAX_TARGET_LONG_EDGE;
    cache
        .iter()
        .filter_map(|(key, _page)| {
            (key.index == inserted.index
                && key.decode == inserted.decode
                && key.target_long_edge < inserted.target_long_edge
                && (!inserted_is_original || key.target_long_edge > MAX_TARGET_LONG_EDGE))
                .then_some(*key)
        })
        .collect()
}

pub(in crate::app) fn normal_navigation_page_keys_in_cache(
    cache: &LruCache<PageCacheKey, Arc<crate::core::worker::PreparedPage>>,
    visible_indices: &[usize],
    decode: DecodeOptions,
) -> HashSet<PageCacheKey> {
    cache
        .iter()
        .filter_map(|(key, _page)| {
            (visible_indices.contains(&key.index)
                && key.decode == decode
                && key.target_long_edge > PREVIEW_TARGET_LONG_EDGE
                && key.target_long_edge <= MAX_TARGET_LONG_EDGE)
                .then_some(*key)
        })
        .collect()
}

pub(in crate::app) fn touch_normal_navigation_page_keys(
    cache: &mut LruCache<PageCacheKey, Arc<crate::core::worker::PreparedPage>>,
    visible_indices: &[usize],
    decode: DecodeOptions,
) {
    let keys = normal_navigation_page_keys_in_cache(cache, visible_indices, decode);
    for key in keys {
        let _ = cache.get(&key);
    }
}

#[cfg(any(test, feature = "perf-dev", feature = "perf-diagnostics"))]
pub(in crate::app) fn page_cache_state_from_hit(
    hit: Option<PageCacheKey>,
    requested: PageCacheKey,
) -> perf::PageCacheState {
    use std::cmp::Ordering;

    let Some(hit) = hit else {
        return perf::PageCacheState::Miss;
    };
    match hit.target_long_edge.cmp(&requested.target_long_edge) {
        Ordering::Equal => perf::PageCacheState::DecodedExact,
        Ordering::Less => perf::PageCacheState::DecodedPreview,
        Ordering::Greater => perf::PageCacheState::DecodedFallback,
    }
}
