use super::perf;
use super::{
    texture_options_for_sampling, SuiSuiViewApp, TextureCacheKey, TextureEntry, ViewMode,
    BYTES_PER_RGBA_PIXEL,
};
use crate::core::effects::ViewEffects;
use crate::core::state::WgpuUpscaleMethod;
use crate::core::worker::{NavigationDirection, MAX_TARGET_LONG_EDGE};
use egui::{self, ImageData};
use std::sync::Arc;
use std::time::Duration;
#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
use std::time::Instant;

const MAX_TEXTURE_PREWARMS_PER_FRAME: usize = 1;
const PRIMARY_DIRECTION_TURNS: usize = 2;
const SECONDARY_DIRECTION_TURNS: usize = 1;
const PREWARM_REPAINT_DELAY: Duration = Duration::from_millis(16);

impl SuiSuiViewApp {
    pub(in crate::app) fn prewarm_neighbor_textures(&mut self, ctx: &egui::Context) {
        if !self.can_prewarm_neighbor_textures() {
            return;
        }

        let Some(source) = self.source.as_ref() else {
            return;
        };
        if source.page_count() == 0 {
            return;
        }

        let candidates = prewarm_candidate_pages_from_turns(
            self.current_page,
            self.last_nav_direction,
            PRIMARY_DIRECTION_TURNS,
            SECONDARY_DIRECTION_TURNS,
            |page, direction| self.page_turn_target_from(page, direction),
        );
        let visible = self.spread_indices();
        let mut uploads = 0usize;

        'candidates: for page in candidates {
            for index in self.spread_indices_for(page) {
                if visible.contains(&index) {
                    continue;
                }
                if !self.prewarm_page_texture(ctx, index) {
                    continue;
                }
                uploads += 1;
                if uploads >= MAX_TEXTURE_PREWARMS_PER_FRAME {
                    break 'candidates;
                }
            }
        }

        if uploads >= MAX_TEXTURE_PREWARMS_PER_FRAME {
            ctx.request_repaint_after(PREWARM_REPAINT_DELAY);
        }
    }

    fn can_prewarm_neighbor_textures(&self) -> bool {
        // The neighbor-texture prewarm is a paged-turn optimization: its candidate
        // pages come from `page_turn_target_from`/`spread_indices_for`, and it
        // warms textures a couple of discrete turns out so a page turn lands on an
        // already-uploaded texture. The vertical strip has no discrete turns; it
        // paints its continuous visible window plus a one-page margin every frame
        // (`paint_strip`), uploading each visible page's texture during paint. This
        // prewarm therefore only duplicates that work or warms pages beyond the
        // strip's own margin, competing for the per-frame upload slot and texture
        // budget, so skip it in strip mode.
        self.view_mode != ViewMode::VerticalStrip
            && self.settings.prefetch_enabled
            && perf::texture_prewarm_enabled()
            && self.source.is_some()
            && self.target_long_edge <= MAX_TARGET_LONG_EDGE
            && self.effects == ViewEffects::default()
            && self.active_wgpu_upscale_method() == WgpuUpscaleMethod::None
            && !self.debug_compare.enabled
            && self.transition.is_none()
            && self.pending_page_turn.is_none()
    }

    fn prewarm_page_texture(&mut self, ctx: &egui::Context, index: usize) -> bool {
        let Some(requested) = self.page_key_at(index, self.target_long_edge) else {
            return false;
        };
        let Some(best_key) = self.final_quality_page_key(requested) else {
            return false;
        };
        let texture_key = TextureCacheKey {
            page: best_key,
            effects: self.effects,
            sampling: self.texture_sampling_for_page_key(best_key),
        };
        if self.textures.peek(&texture_key).is_some() {
            return false;
        }

        let page = self.decoded_pages.get(&best_key).cloned();
        let Some(page) = page else {
            return false;
        };
        // egui textures are always RGBA regardless of how the page retained its pixels, so budget
        // against the RGBA footprint (a luma page's `byte_size` is only a quarter of that).
        let texture_byte_size = page
            .display_width
            .saturating_mul(page.display_height)
            .saturating_mul(BYTES_PER_RGBA_PIXEL);
        if !self.texture_cache_has_room_for(texture_byte_size) {
            return false;
        }

        let image = Arc::new(page.color_image());
        let byte_size = texture_byte_size;
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        let texture_started = Instant::now();
        let texture = ctx.load_texture(
            format!(
                "page-{index}-{}-{:?}",
                best_key.target_long_edge, self.effects
            ),
            ImageData::Color(image),
            texture_options_for_sampling(texture_key.sampling),
        );
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        perf::record_texture_prewarm(texture_started, index, best_key.target_long_edge);
        self.textures
            .put(texture_key, TextureEntry { texture, byte_size });
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        self.record_cache_snapshot("texture_prewarm");
        true
    }

    fn texture_cache_has_room_for(&self, byte_size: usize) -> bool {
        self.texture_cache_bytes().saturating_add(byte_size) <= self.texture_cache_budget_bytes()
    }
}

fn prewarm_candidate_pages_from_turns(
    current_page: usize,
    direction: NavigationDirection,
    primary_turns: usize,
    secondary_turns: usize,
    mut page_turn_target_from: impl FnMut(usize, NavigationDirection) -> Option<usize>,
) -> Vec<usize> {
    let mut pages = Vec::with_capacity(primary_turns.saturating_add(secondary_turns));
    push_direction_pages(
        &mut pages,
        current_page,
        direction,
        primary_turns,
        &mut page_turn_target_from,
    );
    push_direction_pages(
        &mut pages,
        current_page,
        opposite_direction(direction),
        secondary_turns,
        &mut page_turn_target_from,
    );
    pages
}

fn push_direction_pages(
    pages: &mut Vec<usize>,
    mut page: usize,
    direction: NavigationDirection,
    turns: usize,
    page_turn_target_from: &mut impl FnMut(usize, NavigationDirection) -> Option<usize>,
) {
    for _ in 0..turns {
        let Some(next_page) = page_turn_target_from(page, direction) else {
            return;
        };
        page = next_page;
        if !pages.contains(&page) {
            pages.push(page);
        }
    }
}

fn opposite_direction(direction: NavigationDirection) -> NavigationDirection {
    match direction {
        NavigationDirection::Forward => NavigationDirection::Backward,
        NavigationDirection::Backward => NavigationDirection::Forward,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidate_pages_prefer_last_navigation_direction() {
        let pages = prewarm_candidate_pages_from_turns(
            10,
            NavigationDirection::Forward,
            2,
            1,
            |page, direction| match direction {
                NavigationDirection::Forward => Some(page + 2),
                NavigationDirection::Backward => page.checked_sub(2),
            },
        );

        assert_eq!(pages, vec![12, 14, 8]);
    }

    #[test]
    fn candidate_pages_skip_duplicates_and_edges() {
        let pages = prewarm_candidate_pages_from_turns(
            0,
            NavigationDirection::Backward,
            2,
            2,
            |page, direction| match direction {
                NavigationDirection::Forward => Some(page + 1),
                NavigationDirection::Backward => page.checked_sub(1),
            },
        );

        assert_eq!(pages, vec![1, 2]);
    }
}
