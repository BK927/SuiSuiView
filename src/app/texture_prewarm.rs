use super::perf;
use super::viewer::spread_natural_size;
use super::{
    gpu_visual_needs_wgsl, rect_target_size, texture_options_for_sampling, SuiSuiViewApp,
    TextureCacheKey, TextureEntry, ViewMode, BYTES_PER_RGBA_PIXEL,
};
use crate::core::deband::ResolvedDeband;
use crate::core::effects::ViewEffects;
use crate::core::state::WgpuUpscaleMethod;
#[cfg(test)]
use crate::core::state::WGPU_DOWNSCALE_METHOD;
use crate::core::worker::{NavigationDirection, MAX_TARGET_LONG_EDGE};
use egui::{self, ImageData, Pos2, Rect, Vec2};
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
            let indices = self.spread_indices_for(page);
            let scale = self.prewarm_spread_scale(ctx, &indices);
            for index in indices {
                if visible.contains(&index) {
                    continue;
                }
                if !self.prewarm_page_texture(ctx, index, scale) {
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
            && self.display_effects() == ViewEffects::default()
            && self.active_wgpu_upscale_method() == WgpuUpscaleMethod::None
            && !self.debug_compare.enabled
            && self.transition.is_none()
            && self.pending_page_turn.is_none()
    }

    fn prewarm_spread_scale(&self, ctx: &egui::Context, indices: &[usize]) -> Option<f32> {
        if !self.can_paint_wgsl_effects() || self.sibling_book_transition_stabilizing() {
            return None;
        }
        let viewport = self.last_viewer_size_points?;
        // Share settled spread geometry with painting. Unknown partner dimensions
        // retain the existing prewarm behavior instead of guessing a render route.
        let sizes = indices
            .iter()
            .map(|index| {
                self.page_metrics_at(*index)
                    .map(|m| Vec2::new(m.width, m.height))
            })
            .collect::<Option<Vec<_>>>()?;
        Some(self.scale_for(viewport, spread_natural_size(sizes), ctx.pixels_per_point()))
    }

    fn prewarm_page_texture(
        &mut self,
        ctx: &egui::Context,
        index: usize,
        scale: Option<f32>,
    ) -> bool {
        let Some(requested) = self.page_key_at(index, self.target_long_edge) else {
            return false;
        };
        let Some(best_key) = self.final_quality_page_key(requested) else {
            return false;
        };
        let texture_key = TextureCacheKey {
            page: best_key,
            effects: self.display_effects(),
            sampling: self.texture_sampling_for_page_key(best_key),
        };
        if self.textures.peek(&texture_key).is_some() {
            return false;
        }

        let Some(page) = self.decoded_pages.peek(&best_key) else {
            return false;
        };
        if !prewarm_uses_egui_texture_with_downscale(
            page.image_size(),
            Vec2::new(page.original_width as f32, page.original_height as f32),
            scale,
            ctx.pixels_per_point(),
            self.active_deband(),
            self.settings.expert_downscale.gpu.method(),
        ) {
            // WGSL uses its own source texture; this egui texture would never
            // be consumed at the predicted scale. Keep decoded-page prefetch.
            return false;
        }
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
        self.decoded_pages.promote(&best_key);
        let byte_size = texture_byte_size;
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        let texture_started = Instant::now();
        let texture = ctx.load_texture(
            format!(
                "page-{index}-{}-{:?}",
                best_key.target_long_edge,
                self.display_effects()
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

fn prewarm_uses_egui_texture_with_downscale(
    image_size: [usize; 2],
    original_size: Vec2,
    spread_scale: Option<f32>,
    pixels_per_point: f32,
    deband: ResolvedDeband,
    downscale: crate::core::state::WgpuDownscaleMethod,
) -> bool {
    let Some(scale) = spread_scale else {
        // Glow, temporary fallback, or unknown layout: retain ordinary prewarm.
        return true;
    };
    // Eligibility already excludes effects and display upscaling. Use the same
    // downscale/deband routing as painting, including its near-native bypass.
    !gpu_visual_needs_wgsl(
        image_size,
        rect_target_size(
            Rect::from_min_size(Pos2::ZERO, original_size * scale),
            pixels_per_point,
        ),
        ViewEffects::default(),
        WgpuUpscaleMethod::None,
        downscale,
        1.0,
        deband,
    )
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
    fn prewarm_follows_paint_route_at_display_scale_and_dpi() {
        let decoded = [2240, 3360];
        let original = Vec2::new(4480.0, 6720.0);
        // Half-size display uses quality downscaling and its own source texture.
        assert!(!prewarm_uses_egui_texture(
            decoded,
            original,
            Some(0.25),
            1.0,
            ResolvedDeband::Off
        ));
        // Native and near-native display keep useful egui prewarm, including DPI scaling.
        for (scale, dpi) in [(0.5, 1.0), (0.475, 1.0), (0.25, 2.0)] {
            assert!(prewarm_uses_egui_texture(
                decoded,
                original,
                Some(scale),
                dpi,
                ResolvedDeband::Off
            ));
        }
        assert!(prewarm_uses_egui_texture(
            decoded,
            original,
            None,
            1.0,
            ResolvedDeband::Off
        ));
        for deband in [
            ResolvedDeband::Weak,
            ResolvedDeband::Medium,
            ResolvedDeband::Strong,
        ] {
            assert!(!prewarm_uses_egui_texture(
                decoded,
                original,
                Some(0.5),
                1.0,
                deband
            ));
        }
    }

    #[test]
    fn spread_geometry_includes_partner_and_gap_for_prewarm() {
        let left = Vec2::new(1600.0, 2400.0);
        let right = Vec2::new(1700.0, 2500.0);
        assert_eq!(spread_natural_size([left]), left);
        assert_eq!(
            spread_natural_size([left, right]),
            Vec2::new(3314.0, 2500.0)
        );
        assert_eq!(
            spread_natural_size([right, left]),
            Vec2::new(3314.0, 2500.0)
        );
    }

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

#[cfg(test)]
fn prewarm_uses_egui_texture(
    image_size: [usize; 2],
    original_size: Vec2,
    spread_scale: Option<f32>,
    pixels_per_point: f32,
    deband: ResolvedDeband,
) -> bool {
    prewarm_uses_egui_texture_with_downscale(
        image_size,
        original_size,
        spread_scale,
        pixels_per_point,
        deband,
        WGPU_DOWNSCALE_METHOD,
    )
}
