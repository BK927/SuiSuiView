use super::gpu_paint::{GpuPaintRequest, GpuPaintSourceKey};
use super::perf;
use super::{
    gpu_visual_needs_wgsl, rect_target_size, ui, PageCacheKey, SuiSuiViewApp, TextureCacheKey,
    TextureEntry, BYTES_PER_RGBA_PIXEL,
};
use crate::core::effects::{
    apply_effects_to_image, compose_images_horizontally, transformed_page_size, ViewEffects,
};
use crate::core::state::PageTransitionStyle;
use crate::core::worker::MAX_TARGET_LONG_EDGE;
use eframe::egui::{
    self, Align2, Color32, ColorImage, FontId, ImageData, Pos2, Rect, Sense, Stroke, StrokeKind,
    Vec2,
};
use std::sync::Arc;
#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
use std::time::Instant;

mod interaction;
mod model;
mod paint_helpers;
mod transition;

#[cfg(test)]
pub(in crate::app) use model::relative_difference;
pub(in crate::app) use model::{
    double_spread_indices, ordered_spread_indices, page_visual_size,
    smart_spread_indices_for_metrics, worker_center_page_for_mode, CurrentViewState, PageMetrics,
    PageRenderInfo, PageVisual, Transition, ViewMode,
};
pub(in crate::app) use paint_helpers::texture_options_for_sampling;
pub(in crate::app) use transition::{
    paint_book_flip_shadow, transition_paint_params, transition_screen_sign,
};

const TRANSITION_MS: f32 = 120.0;
const SPREAD_GAP_POINTS: f32 = 14.0;
const TARGET_EDGE_HYSTERESIS: u32 = 512;

struct SpreadPaint<'a> {
    viewport: Rect,
    indices: &'a [usize],
    target_long_edge: u32,
    offset: Vec2,
    scale: Vec2,
    alpha: f32,
}

impl SuiSuiViewApp {
    pub(in crate::app) fn effected_page_image(
        &self,
        index: usize,
        target_long_edge: u32,
    ) -> Option<ColorImage> {
        let key = PageCacheKey {
            index,
            target_long_edge,
            decode: self.decode_options(),
        };
        let best_key = self.best_page_key(key)?;
        let page = self.decoded_pages.peek(&best_key)?;
        Some(apply_effects_to_image(&page.color_image(), self.effects))
    }

    pub(in crate::app) fn compose_spread_image(
        &self,
        indices: &[usize],
        target_long_edge: u32,
    ) -> Option<ColorImage> {
        let images = indices
            .iter()
            .map(|index| self.effected_page_image(*index, target_long_edge))
            .collect::<Option<Vec<_>>>()?;
        compose_images_horizontally(&images, SPREAD_GAP_POINTS as usize)
    }

    pub(in crate::app) fn spread_indices(&self) -> Vec<usize> {
        self.spread_indices_for(self.current_page)
    }

    pub(in crate::app) fn spread_indices_for(&self, page: usize) -> Vec<usize> {
        ordered_spread_indices(
            self.spread_indices_for_unordered(page),
            self.view_mode,
            self.reading_direction,
        )
    }

    pub(in crate::app) fn spread_indices_for_unordered(&self, page: usize) -> Vec<usize> {
        let Some(source) = self.source.as_ref() else {
            return Vec::new();
        };
        let page_count = source.page_count();
        if page_count == 0 {
            return Vec::new();
        }

        let page = page.min(page_count - 1);
        match self.view_mode {
            ViewMode::Single => vec![page],
            ViewMode::DoubleLeftToRight | ViewMode::DoubleRightToLeft => {
                double_spread_indices(page, page_count)
            }
            ViewMode::SmartDoubleLeftToRight | ViewMode::SmartDoubleRightToLeft => {
                self.smart_spread_indices_for(page, page_count)
            }
        }
    }

    pub(in crate::app) fn smart_spread_indices_for(
        &self,
        page: usize,
        page_count: usize,
    ) -> Vec<usize> {
        smart_spread_indices_for_metrics(page, page_count, &self.page_metrics)
    }

    pub(in crate::app) fn visible_page_count(&self) -> usize {
        self.view_mode.step()
    }

    pub(in crate::app) fn worker_center_page(&self) -> usize {
        worker_center_page_for_mode(self.current_page, self.view_mode)
    }

    fn page_visual(
        &mut self,
        ctx: &egui::Context,
        index: usize,
        target_long_edge: u32,
    ) -> PageVisual {
        if let Some(error) = self.page_errors.get(&index) {
            return PageVisual::Failed {
                index,
                message: error.clone(),
            };
        }

        let key = PageCacheKey {
            index,
            target_long_edge,
            decode: self.decode_options(),
        };
        if let Some(visual) = self.original_texture_only_visual(key) {
            return visual;
        }
        let Some(best_key) = self.best_page_key(key) else {
            return PageVisual::Loading { index };
        };
        let use_wgsl_effects = self.can_paint_wgsl_effects();
        let sampling = self.texture_sampling_for_page_key(best_key);
        let texture_key = TextureCacheKey {
            page: best_key,
            effects: self.effects,
            sampling,
        };

        if !use_wgsl_effects {
            if let Some(texture) = self
                .textures
                .get(&texture_key)
                .map(|entry| entry.texture.clone())
            {
                let page = self.decoded_pages.get(&best_key);
                if let Some(page) = page {
                    let size = transformed_page_size(
                        page.original_width as f32,
                        page.original_height as f32,
                        self.effects.transform,
                    );
                    #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
                    perf::record_open_to_first_visible_if_pending(
                        &mut self.open_to_first_visible_trace,
                        self.book_id.as_deref(),
                        index,
                        best_key.target_long_edge,
                        false,
                    );
                    return PageVisual::Ready {
                        texture,
                        size,
                        render_info: Some(PageRenderInfo::from_page(index, best_key, page)),
                    };
                }
            }
        }

        let page = self
            .decoded_pages
            .get(&best_key)
            .cloned()
            .expect("best page key should exist in decoded cache");
        if use_wgsl_effects {
            let wgpu_upscale_method =
                self.content_aware_wgpu_upscale_method(best_key, self.active_wgpu_upscale_method());
            #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
            perf::record_open_to_first_visible_if_pending(
                &mut self.open_to_first_visible_trace,
                self.book_id.as_deref(),
                index,
                best_key.target_long_edge,
                true,
            );
            return PageVisual::ReadyGpu {
                source_key: GpuPaintSourceKey {
                    book: self.gpu_paint_book_key(),
                    page: best_key,
                },
                image_size: page.image_size(),
                rgba: page.rgba.clone(),
                size: transformed_page_size(
                    page.original_width as f32,
                    page.original_height as f32,
                    self.effects.transform,
                ),
                effects: self.effects,
                wgpu_upscale_method,
                wgpu_downscale_method: self.settings.wgpu_downscale_method,
                render_info: PageRenderInfo::from_page(index, best_key, &page),
            };
        }
        let image = if self.effects == ViewEffects::default() {
            Arc::new(page.color_image())
        } else {
            #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
            let effects_started = Instant::now();
            let image = Arc::new(apply_effects_to_image(&page.color_image(), self.effects));
            #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
            perf::record_page_effects_cpu(effects_started, index, best_key.target_long_edge);
            image
        };

        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        let texture_started = Instant::now();
        let texture_byte_size = image
            .size
            .iter()
            .copied()
            .product::<usize>()
            .saturating_mul(BYTES_PER_RGBA_PIXEL);
        let texture = ctx.load_texture(
            format!(
                "page-{index}-{}-{:?}",
                best_key.target_long_edge, self.effects
            ),
            ImageData::Color(image),
            texture_options_for_sampling(sampling),
        );
        ctx.request_repaint_after(super::TEXTURE_PRESENT_REPAINT_DELAY);
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        perf::record_texture_load(texture_started, index, best_key.target_long_edge);
        self.textures.put(
            texture_key,
            TextureEntry {
                texture: texture.clone(),
                byte_size: texture_byte_size,
            },
        );
        self.prune_texture_cache();
        let dropped_original = self.drop_original_after_texture_upload_if_enabled(best_key);
        #[cfg(not(any(feature = "perf-dev", feature = "perf-diagnostics")))]
        let _ = dropped_original;
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        self.record_cache_snapshot(if dropped_original {
            "original_texture_only_drop"
        } else {
            "texture_upload"
        });

        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        perf::record_open_to_first_visible_if_pending(
            &mut self.open_to_first_visible_trace,
            self.book_id.as_deref(),
            index,
            best_key.target_long_edge,
            false,
        );
        PageVisual::Ready {
            texture,
            size: transformed_page_size(
                page.original_width as f32,
                page.original_height as f32,
                self.effects.transform,
            ),
            render_info: Some(PageRenderInfo::from_page(index, best_key, &page)),
        }
    }

    fn original_texture_only_visual(&mut self, requested: PageCacheKey) -> Option<PageVisual> {
        if !perf::original_texture_only_enabled()
            || !self
                .current_prepared_target_intent()
                .is_original_inspection()
            || requested.target_long_edge <= MAX_TARGET_LONG_EDGE
        {
            return None;
        }
        let texture_key = TextureCacheKey {
            page: requested,
            effects: self.effects,
            sampling: self.texture_sampling_for_page_key(requested),
        };
        let texture = self
            .textures
            .get(&texture_key)
            .map(|entry| entry.texture.clone())?;
        let metrics = self.page_metrics.get(&requested.index).copied()?;
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        perf::record_open_to_first_visible_if_pending(
            &mut self.open_to_first_visible_trace,
            self.book_id.as_deref(),
            requested.index,
            requested.target_long_edge,
            false,
        );
        Some(PageVisual::Ready {
            texture,
            size: transformed_page_size(metrics.width, metrics.height, self.effects.transform),
            render_info: None,
        })
    }

    pub(in crate::app) fn show_viewer(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let available = ui.available_size();
        let (rect, response) = ui.allocate_exact_size(available, Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        self.paint_pending_gpu_original_inspection_cleanup(&painter, rect);

        painter.rect_filled(rect, 0.0, ui::theme::VIEWER_BG);
        if self.settings.show_main_border {
            painter.rect_stroke(
                rect.shrink(0.5),
                0.0,
                Stroke::new(1.0, ui::theme::SUBTLE_STROKE),
                StrokeKind::Inside,
            );
        }
        self.show_context_menu(ctx, &response);

        if self.source.is_none() {
            self.current_view_state = None;
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                self.i18n().text("viewer.empty"),
                FontId::proportional(22.0),
                ui::theme::TEXT_PRIMARY,
            );
            return;
        }

        self.update_target_long_edge(ctx, rect.size());
        self.handle_viewer_pointer(ui, &response);

        if self.debug_compare.enabled {
            self.current_view_state = None;
            self.transition = None;
            self.paint_debug_compare(ctx, &painter, rect);
            return;
        }

        let current_indices = self.spread_indices();
        if let Some(transition) = self.transition.take() {
            let elapsed_ms = transition.started_at.elapsed().as_secs_f32() * 1000.0;
            let t = (elapsed_ms / TRANSITION_MS).clamp(0.0, 1.0);
            let paint = transition_paint_params(transition.style, t, transition.screen_sign, rect);
            let current_target_long_edge = self.target_long_edge;

            self.paint_spread(
                ctx,
                &painter,
                SpreadPaint {
                    viewport: rect,
                    indices: &transition.from_indices,
                    target_long_edge: transition.target_long_edge,
                    offset: paint.from_offset,
                    scale: paint.from_scale,
                    alpha: paint.from_alpha,
                },
            );
            if transition.style == PageTransitionStyle::BookFlip2d {
                paint_book_flip_shadow(&painter, rect, transition.screen_sign, t);
            }
            self.paint_spread(
                ctx,
                &painter,
                SpreadPaint {
                    viewport: rect,
                    indices: &current_indices,
                    target_long_edge: current_target_long_edge,
                    offset: paint.to_offset,
                    scale: paint.to_scale,
                    alpha: paint.to_alpha,
                },
            );

            if t < 1.0 {
                self.transition = Some(transition);
            }
        } else {
            self.paint_spread(
                ctx,
                &painter,
                SpreadPaint {
                    viewport: rect,
                    indices: &current_indices,
                    target_long_edge: self.target_long_edge,
                    offset: Vec2::ZERO,
                    scale: Vec2::splat(1.0),
                    alpha: 1.0,
                },
            );
        }
        self.paint_filename_overlay(ctx, &painter, rect);
        self.paint_page_arrows(ctx, &painter, rect);
    }

    fn paint_filename_overlay(&self, ctx: &egui::Context, painter: &egui::Painter, rect: Rect) {
        if !self.settings.show_filename_overlay || self.settings.top_bar_pinned {
            return;
        }
        if self.top_bar_is_visible(ctx) {
            return;
        }
        let Some(source) = self.source.as_ref() else {
            return;
        };
        let text = source
            .page_display_path(self.current_page)
            .or_else(|| source.page_name(self.current_page).map(ToOwned::to_owned))
            .unwrap_or_else(|| source.title().to_owned());
        let font = FontId::proportional(13.0);
        let galley = painter.layout_no_wrap(text, font, ui::theme::TEXT_PRIMARY);
        let max_width = (rect.width() - 36.0).max(80.0);
        let overlay_rect = Rect::from_min_size(
            rect.min + egui::vec2(14.0, 12.0),
            egui::vec2(galley.size().x.min(max_width) + 18.0, 30.0),
        );
        painter.rect_filled(
            overlay_rect,
            6.0,
            Color32::from_rgba_unmultiplied(14, 16, 20, 208),
        );
        let clipped = painter.with_clip_rect(overlay_rect.shrink2(egui::vec2(9.0, 0.0)));
        clipped.galley(
            egui::pos2(
                overlay_rect.left() + 9.0,
                overlay_rect.center().y - galley.size().y * 0.5,
            ),
            galley,
            ui::theme::TEXT_PRIMARY,
        );
    }

    fn paint_spread(
        &mut self,
        ctx: &egui::Context,
        painter: &egui::Painter,
        request: SpreadPaint<'_>,
    ) {
        if request.indices.is_empty() {
            return;
        }

        let mut pages = Vec::with_capacity(request.indices.len());
        for index in request.indices {
            let visual = self.page_visual(ctx, *index, request.target_long_edge);
            let size = page_visual_size(&visual);
            pages.push((visual, size));
        }

        let gap = if pages.len() > 1 {
            SPREAD_GAP_POINTS
        } else {
            0.0
        };
        let natural_width = pages.iter().map(|(_visual, size)| size.x).sum::<f32>()
            + gap * pages.len().saturating_sub(1) as f32;
        let natural_height = pages
            .iter()
            .map(|(_visual, size)| size.y)
            .fold(1.0_f32, |left, right| left.max(right));
        let scale = self.scale_for(
            request.viewport.size(),
            Vec2::new(natural_width, natural_height),
            ctx.pixels_per_point(),
        );
        let spread_width = natural_width * scale * request.scale.x;
        let spread_height = natural_height * scale * request.scale.y;
        let mut cursor = self.spread_origin(
            request.viewport,
            Vec2::new(spread_width, spread_height),
            request.offset,
        );
        let tint = Color32::from_white_alpha((request.alpha.clamp(0.0, 1.0) * 255.0) as u8);

        for (visual, size) in pages {
            let page_size = Vec2::new(
                size.x * scale * request.scale.x,
                size.y * scale * request.scale.y,
            );
            let top = cursor.y + (spread_height - page_size.y) * 0.5;
            let page_rect = Rect::from_min_size(Pos2::new(cursor.x, top), page_size);

            match visual {
                PageVisual::Ready {
                    texture,
                    render_info,
                    ..
                } => {
                    if let Some(render_info) = render_info {
                        let target_intent =
                            self.prepared_target_intent_for_target(render_info.target_long_edge);
                        self.record_current_view_state(CurrentViewState::from_cpu(
                            render_info,
                            target_intent,
                        ));
                    }
                    painter.image(
                        texture.id(),
                        page_rect,
                        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                        tint,
                    );
                }
                PageVisual::ReadyGpu {
                    source_key,
                    image_size,
                    rgba,
                    effects,
                    wgpu_upscale_method,
                    wgpu_downscale_method,
                    render_info,
                    ..
                } => {
                    let target_size = rect_target_size(page_rect, ctx.pixels_per_point());
                    let active_wgsl = gpu_visual_needs_wgsl(
                        image_size,
                        target_size,
                        effects,
                        wgpu_upscale_method,
                        wgpu_downscale_method,
                    );
                    let target_intent =
                        self.prepared_target_intent_for_target(render_info.target_long_edge);
                    self.record_current_view_state(CurrentViewState::from_gpu(
                        render_info,
                        image_size,
                        effects,
                        target_size,
                        wgpu_upscale_method,
                        wgpu_downscale_method,
                        active_wgsl,
                        target_intent,
                    ));
                    if !self.paint_ready_gpu_visual(
                        ctx,
                        painter,
                        GpuPaintRequest {
                            rect: page_rect,
                            source_key,
                            image_size,
                            rgba,
                            effects,
                            wgpu_upscale_method,
                            wgpu_downscale_method,
                            opacity: request.alpha,
                        },
                        tint,
                    ) {
                        self.paint_placeholder(
                            painter,
                            page_rect,
                            "GPU effect fallback pending",
                            Color32::from_gray(120),
                            tint,
                        );
                    }
                }
                PageVisual::Loading { index } => {
                    self.clear_current_view_state_for(index);
                    self.paint_placeholder(
                        painter,
                        page_rect,
                        &format!("Loading page {}", index + 1),
                        Color32::from_gray(120),
                        tint,
                    );
                }
                PageVisual::Failed { index, message } => {
                    self.clear_current_view_state_for(index);
                    self.paint_placeholder(
                        painter,
                        page_rect,
                        &format!("Page {} failed\n{}", index + 1, message),
                        Color32::from_rgb(180, 80, 80),
                        tint,
                    );
                }
            }

            cursor.x += page_size.x + gap * scale;
        }
    }

    fn record_current_view_state(&mut self, state: CurrentViewState) {
        if state.page_index == self.current_page {
            self.current_view_state = Some(state);
        }
    }

    fn clear_current_view_state_for(&mut self, index: usize) {
        if index == self.current_page {
            self.current_view_state = None;
        }
    }
}
