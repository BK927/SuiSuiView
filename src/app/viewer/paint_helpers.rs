use crate::app::gpu_paint::{GpuPaintRequest, GpuPaintSourceKey};
use crate::app::{
    gpu_visual_needs_wgsl, rect_target_size, SuiSuiViewApp, TextureCacheKey, TextureEntry,
    TextureSampling,
};
use crate::core::effects::ViewEffects;
use crate::core::state::{FitMode, LargeImageAnchor};
use eframe::egui::{
    self, Align2, Color32, ColorImage, FontId, ImageData, Pos2, Rect, Stroke, StrokeKind,
    TextureHandle, TextureOptions, Vec2,
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
            )
        {
            let texture = self.texture_for_gpu_fallback(
                ctx,
                request.source_key,
                request.image_size,
                &request.rgba,
                request.effects,
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
        rgba: &[u8],
        effects: ViewEffects,
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

        let image = Arc::new(ColorImage::from_rgba_unmultiplied(image_size, rgba));
        let texture_byte_size = rgba.len();
        let texture = ctx.load_texture(
            format!(
                "page-{}-{}-{:?}",
                source_key.page.index, source_key.page.target_long_edge, effects
            ),
            ImageData::Color(image),
            texture_options_for_sampling(texture_key.sampling),
        );
        ctx.request_repaint_after(super::super::TEXTURE_PRESENT_REPAINT_DELAY);
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
}

fn source_pixel_scale(pixels_per_point: f32) -> f32 {
    1.0 / pixels_per_point.max(0.1)
}

pub(in crate::app) fn texture_options_for_sampling(sampling: TextureSampling) -> TextureOptions {
    match sampling {
        TextureSampling::Linear => TextureOptions::LINEAR,
        TextureSampling::Nearest => TextureOptions::NEAREST,
    }
}

#[cfg(test)]
mod tests {
    use super::{source_pixel_scale, texture_options_for_sampling};
    use crate::app::TextureSampling;
    use eframe::egui::TextureOptions;

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
