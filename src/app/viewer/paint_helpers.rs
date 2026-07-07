use crate::app::gpu_paint::{GpuPaintRequest, GpuPaintSourceKey};
use crate::app::{
    gpu_visual_needs_wgsl, rect_target_size, SuiSuiViewApp, TextureCacheKey, TextureEntry,
    TextureSampling, BYTES_PER_RGBA_PIXEL,
};
use crate::core::effects::ViewEffects;
use crate::core::state::{FitMode, LargeImageAnchor};
use crate::core::worker::PagePixels;
use egui::{
    self, Align2, Color32, FontId, ImageData, Pos2, Rect, Stroke, StrokeKind, TextureHandle,
    TextureOptions, Vec2,
};
use std::sync::Arc;

impl SuiSuiViewApp {
    pub(in crate::app) fn paint_placeholder(
        &self,
        painter: &egui::Painter,
        rect: Rect,
        text: &str,
        color: Color32,
        tint: Color32,
    ) {
        let stroke = Stroke::new(1.0, color.gamma_multiply(tint.a() as f32 / 255.0));
        painter.rect_stroke(rect, 2.0, stroke, StrokeKind::Inside);
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            text,
            FontId::proportional(16.0),
            color.gamma_multiply(tint.a() as f32 / 255.0),
        );
    }

    pub(super) fn paint_ready_gpu_visual(
        &mut self,
        ctx: &egui::Context,
        painter: &egui::Painter,
        request: GpuPaintRequest,
        force_texture_fallback: bool,
        tint: Color32,
    ) -> bool {
        let target_size = rect_target_size(request.rect, ctx.pixels_per_point());
        if force_texture_fallback
            || !gpu_visual_needs_wgsl(
                request.image_size,
                target_size,
                request.effects,
                request.wgpu_upscale_method,
                request.wgpu_downscale_method,
                self.settings.fixed_2x_sr_min_scale(),
            )
        {
            let texture = self.texture_for_gpu_fallback(
                ctx,
                request.source_key,
                request.image_size,
                &request.pixels,
                request.effects,
                !force_texture_fallback,
            );
            painter.image(
                texture.id(),
                request.rect,
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                tint,
            );
            return true;
        }

        self.paint_wgsl_effects(painter, request)
    }

    fn texture_for_gpu_fallback(
        &mut self,
        ctx: &egui::Context,
        source_key: GpuPaintSourceKey,
        image_size: [usize; 2],
        pixels: &PagePixels,
        effects: ViewEffects,
        request_present_repaint: bool,
    ) -> TextureHandle {
        let texture_key = TextureCacheKey {
            page: source_key.page,
            effects,
            sampling: self.texture_sampling_for_page_key(source_key.page),
        };
        if let Some(texture) = self
            .textures
            .get(&texture_key)
            .map(|entry| entry.texture.clone())
        {
            return texture;
        }

        // egui textures are RGBA. Build the ColorImage directly from whatever the page retained
        // (luma builds gray Color32s with no intermediate RGBA Vec) and account the RGBA footprint.
        let image = Arc::new(pixels.to_color_image(image_size));
        let texture_byte_size = image_size
            .iter()
            .copied()
            .product::<usize>()
            .saturating_mul(BYTES_PER_RGBA_PIXEL);
        let texture = ctx.load_texture(
            format!(
                "page-{}-{}-{:?}",
                source_key.page.page_id.0, source_key.page.target_long_edge, effects
            ),
            ImageData::Color(image),
            texture_options_for_sampling(texture_key.sampling),
        );
        if request_present_repaint {
            ctx.request_repaint_after(super::super::TEXTURE_PRESENT_REPAINT_DELAY);
        }
        self.textures.put(
            texture_key,
            TextureEntry {
                texture: texture.clone(),
                byte_size: texture_byte_size,
            },
        );
        self.prune_texture_cache();
        let dropped_original = self.drop_original_after_texture_upload_if_enabled(source_key.page);
        #[cfg(not(any(feature = "perf-dev", feature = "perf-diagnostics")))]
        let _ = dropped_original;
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        self.record_cache_snapshot(if dropped_original {
            "original_texture_only_drop"
        } else {
            "gpu_fallback_texture_upload"
        });
        texture
    }

    pub(in crate::app) fn spread_origin(
        &self,
        viewport: Rect,
        spread_size: Vec2,
        offset: Vec2,
    ) -> Pos2 {
        let centered_x = viewport.center().x - spread_size.x * 0.5;
        let centered_y = viewport.center().y - spread_size.y * 0.5;
        let x = if spread_size.x > viewport.width()
            && self.settings.large_image_anchor == LargeImageAnchor::TopLeft
        {
            viewport.left()
        } else {
            centered_x
        };
        let y = if spread_size.y > viewport.height()
            && matches!(
                self.settings.large_image_anchor,
                LargeImageAnchor::Top | LargeImageAnchor::TopLeft
            ) {
            viewport.top()
        } else {
            centered_y
        };

        Pos2::new(x + self.pan.x + offset.x, y + self.pan.y + offset.y)
    }

    pub(in crate::app) fn scale_for(
        &self,
        viewport: Vec2,
        natural: Vec2,
        pixels_per_point: f32,
    ) -> f32 {
        let safe = Vec2::new(natural.x.max(1.0), natural.y.max(1.0));
        match self.fit_mode {
            FitMode::FitPage => (viewport.x / safe.x).min(viewport.y / safe.y),
            FitMode::FitWidth => viewport.x / safe.x,
            FitMode::FitHeight => viewport.y / safe.y,
            FitMode::Original => source_pixel_scale(pixels_per_point),
            FitMode::Manual => self.manual_zoom * source_pixel_scale(pixels_per_point),
        }
        .clamp(0.02, 16.0)
    }

    /// Overlay a 1px grid on original-pixel boundaries once the page is magnified past the user
    /// threshold. `original_size` is the transformed (rotation-aware) original pixel size, so a
    /// 90° rotation that swaps width/height is honored. Lines are clipped to the visible window
    /// and land exactly on integer original-pixel boundaries.
    pub(in crate::app) fn paint_pixel_grid(
        &self,
        painter: &egui::Painter,
        rect: Rect,
        original_size: Vec2,
        viewport: Rect,
        pixels_per_point: f32,
    ) {
        if !self.settings.pixel_grid_enabled {
            return;
        }
        let min_zoom = self.settings.pixel_grid_min_zoom_pct as f32 / 100.0;
        if pixel_grid_spacing(rect.width() * pixels_per_point, original_size.x, min_zoom).is_none()
        {
            return;
        }
        let clip = viewport.intersect(rect);
        if clip.width() <= 0.0 || clip.height() <= 0.0 {
            return;
        }
        // 1 physical pixel wide; a translucent black boundary reads against light and dark pages.
        let stroke = Stroke::new(
            1.0 / pixels_per_point.max(0.1),
            Color32::from_black_alpha(96),
        );

        let x_step = rect.width() / original_size.x;
        let first_col = ((clip.left() - rect.left()) / x_step)
            .floor()
            .clamp(0.0, original_size.x) as u32;
        let last_col = ((clip.right() - rect.left()) / x_step)
            .ceil()
            .clamp(0.0, original_size.x) as u32;
        for col in first_col..=last_col {
            let x = rect.left() + col as f32 * x_step;
            if x >= clip.left() && x <= clip.right() {
                painter.line_segment(
                    [Pos2::new(x, clip.top()), Pos2::new(x, clip.bottom())],
                    stroke,
                );
            }
        }

        let y_step = rect.height() / original_size.y;
        let first_row = ((clip.top() - rect.top()) / y_step)
            .floor()
            .clamp(0.0, original_size.y) as u32;
        let last_row = ((clip.bottom() - rect.top()) / y_step)
            .ceil()
            .clamp(0.0, original_size.y) as u32;
        for row in first_row..=last_row {
            let y = rect.top() + row as f32 * y_step;
            if y >= clip.top() && y <= clip.bottom() {
                painter.line_segment(
                    [Pos2::new(clip.left(), y), Pos2::new(clip.right(), y)],
                    stroke,
                );
            }
        }
    }
}

fn source_pixel_scale(pixels_per_point: f32) -> f32 {
    1.0 / pixels_per_point.max(0.1)
}

/// On-screen spacing (physical pixels per original image pixel) when the page is magnified at or
/// beyond `min_zoom` (also physical pixels per original pixel); `None` below threshold or for
/// degenerate input.
pub(in crate::app) fn pixel_grid_spacing(
    rect_width_px: f32,
    original_width: f32,
    min_zoom: f32,
) -> Option<f32> {
    if original_width <= 0.0 || rect_width_px <= 0.0 {
        return None;
    }
    let spacing = rect_width_px / original_width;
    (spacing >= min_zoom).then_some(spacing)
}

pub(in crate::app) fn texture_options_for_sampling(sampling: TextureSampling) -> TextureOptions {
    match sampling {
        TextureSampling::Linear => TextureOptions::LINEAR,
        TextureSampling::Nearest => TextureOptions::NEAREST,
    }
}

#[cfg(test)]
mod tests {
    use super::{pixel_grid_spacing, source_pixel_scale, texture_options_for_sampling};
    use crate::app::TextureSampling;
    use egui::TextureOptions;

    #[test]
    fn pixel_grid_spacing_reports_at_and_above_threshold() {
        // 1600 physical px across 200 original px = 8.0x magnification == 800% threshold.
        assert_eq!(pixel_grid_spacing(1600.0, 200.0, 8.0), Some(8.0));
        assert_eq!(pixel_grid_spacing(2000.0, 200.0, 8.0), Some(10.0));
    }

    #[test]
    fn pixel_grid_spacing_below_threshold_is_none() {
        assert_eq!(pixel_grid_spacing(1400.0, 200.0, 8.0), None);
    }

    #[test]
    fn pixel_grid_spacing_rejects_degenerate_inputs() {
        assert_eq!(pixel_grid_spacing(1600.0, 0.0, 8.0), None);
        assert_eq!(pixel_grid_spacing(1600.0, -10.0, 8.0), None);
        assert_eq!(pixel_grid_spacing(0.0, 200.0, 8.0), None);
    }

    #[test]
    fn source_pixel_scale_maps_one_image_pixel_to_one_physical_pixel() {
        assert_eq!(source_pixel_scale(1.25), 0.8);
        assert_eq!(source_pixel_scale(1.0), 1.0);
    }

    #[test]
    fn original_inspection_texture_uses_nearest_sampler() {
        assert_eq!(
            texture_options_for_sampling(TextureSampling::Nearest),
            TextureOptions::NEAREST
        );
        assert_eq!(
            texture_options_for_sampling(TextureSampling::Linear),
            TextureOptions::LINEAR
        );
    }
}
