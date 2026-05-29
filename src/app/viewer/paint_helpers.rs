use crate::app::gpu_paint::{GpuPaintRequest, GpuPaintSourceKey};
use crate::app::{
    gpu_visual_needs_wgsl, rect_target_size, SuiSuiViewApp, TextureCacheKey, TextureEntry,
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
        tint: Color32,
    ) -> bool {
        let target_size = rect_target_size(request.rect);
        if !gpu_visual_needs_wgsl(
            request.image_size,
            target_size,
            request.effects,
            request.display_upscaler,
        ) {
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
            upscaled: source_key.upscaled,
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
                "page-{}-{}-{}-{:?}",
                source_key.page.index,
                source_key.page.target_long_edge,
                if source_key.upscaled { "ai" } else { "base" },
                effects
            ),
            ImageData::Color(image),
            TextureOptions::LINEAR,
        );
        self.textures.put(
            texture_key,
            TextureEntry {
                texture: texture.clone(),
                byte_size: texture_byte_size,
            },
        );
        self.prune_texture_cache();
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        self.record_cache_snapshot("gpu_fallback_texture_upload");
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

    pub(in crate::app) fn scale_for(&self, viewport: Vec2, natural: Vec2) -> f32 {
        let safe = Vec2::new(natural.x.max(1.0), natural.y.max(1.0));
        match self.fit_mode {
            FitMode::FitPage => (viewport.x / safe.x).min(viewport.y / safe.y),
            FitMode::FitWidth => viewport.x / safe.x,
            FitMode::FitHeight => viewport.y / safe.y,
            FitMode::Original => 1.0,
            FitMode::Manual => self.manual_zoom,
        }
        .clamp(0.02, 16.0)
    }
}
