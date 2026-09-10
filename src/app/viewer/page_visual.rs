//! Decoded-page selection and the final texture/source supplied to the viewer.
use super::*;

impl SuiSuiViewApp {
    pub(super) fn page_visual(
        &mut self,
        ctx: &egui::Context,
        index: usize,
        target_long_edge: u32,
    ) -> PageVisual {
        let Some(key) = self.page_key_at(index, target_long_edge) else {
            return PageVisual::Loading { index, size: None };
        };
        if let Some(visual) = self.original_texture_only_visual(index, key) {
            return visual;
        }
        let Some(best_key) = self.best_page_key(key) else {
            if self.debug_compare.enabled {
                self.request_debug_compare_page(key);
            }
            if let Some(error) = self.page_errors.get(&key) {
                return PageVisual::Failed {
                    index,
                    size: None,
                    message: error.clone(),
                };
            }
            return PageVisual::Loading { index, size: None };
        };
        let use_wgsl_effects = self.can_paint_wgsl_effects();
        let sampling = self.texture_sampling_for_page_key(best_key);
        let texture_key = TextureCacheKey {
            page: best_key,
            effects: self.display_effects(),
            sampling,
        };

        if !use_wgsl_effects {
            if let Some(texture) = self
                .textures
                .get(&texture_key)
                .map(|entry| entry.texture.clone())
            {
                let page = self.decoded_pages.peek(&best_key);
                if let Some(page) = page {
                    let size = transformed_page_size(
                        page.original_width as f32,
                        page.original_height as f32,
                        self.display_effects().transform,
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
            let (wgpu_upscale_method, wgpu_upscale_origin) =
                self.book_aware_wgpu_upscale_method(self.active_wgpu_upscale_method());
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
                pixels: page.pixels.clone(),
                size: transformed_page_size(
                    page.original_width as f32,
                    page.original_height as f32,
                    self.display_effects().transform,
                ),
                effects: self.display_effects(),
                wgpu_upscale_method,
                wgpu_upscale_origin,
                wgpu_downscale_method: self.settings.expert_downscale.gpu.method(),
                render_info: PageRenderInfo::from_page(index, best_key, &page),
            };
        }
        if self.display_effects() != ViewEffects::default() {
            // Preparation changes pixels only. The known transformed geometry
            // must remain stable while its CPU texture is pending or unavailable.
            let size = transformed_page_size(
                page.original_width as f32,
                page.original_height as f32,
                texture_key.effects.transform,
            );
            if let Err(message) =
                self.request_cpu_adjustment(texture_key, page.pixels.clone(), page.image_size())
            {
                return PageVisual::Failed {
                    index,
                    size: Some(size),
                    message,
                };
            }
            // A control drag keeps this page's last complete rendering visible.
            // Never borrow another page, decode target, orientation or sampling mode.
            let previous = self
                .textures
                .iter()
                .find(|(key, _)| {
                    key.page == texture_key.page
                        && key.effects.transform == texture_key.effects.transform
                        && key.sampling == texture_key.sampling
                })
                .map(|(_, entry)| entry.texture.clone());
            if let Some(texture) = previous {
                return PageVisual::Ready {
                    texture,
                    size,
                    render_info: Some(PageRenderInfo::from_page(index, best_key, &page)),
                };
            }
            return PageVisual::Loading {
                index,
                size: Some(size),
            };
        }
        let image = Arc::new(page.color_image());

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
                best_key.target_long_edge,
                self.display_effects()
            ),
            ImageData::Color(image),
            texture_options_for_sampling(sampling),
        );
        ctx.request_repaint_after(crate::app::TEXTURE_PRESENT_REPAINT_DELAY);
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
                self.display_effects().transform,
            ),
            render_info: Some(PageRenderInfo::from_page(index, best_key, &page)),
        }
    }

    pub(super) fn original_texture_only_visual(
        &mut self,
        index: usize,
        requested: PageCacheKey,
    ) -> Option<PageVisual> {
        if self.debug_compare.enabled
            || !perf::original_texture_only_enabled()
            || !self
                .current_prepared_target_intent()
                .is_original_inspection()
            || requested.target_long_edge <= MAX_TARGET_LONG_EDGE
        {
            return None;
        }
        let texture_key = TextureCacheKey {
            page: requested,
            effects: self.display_effects(),
            sampling: self.texture_sampling_for_page_key(requested),
        };
        let texture = self
            .textures
            .get(&texture_key)
            .map(|entry| entry.texture.clone())?;
        let metrics = self.page_metrics.get(&requested.page_id).copied()?;
        #[cfg(not(any(feature = "perf-dev", feature = "perf-diagnostics")))]
        let _ = index;
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        perf::record_open_to_first_visible_if_pending(
            &mut self.open_to_first_visible_trace,
            self.book_id.as_deref(),
            index,
            requested.target_long_edge,
            false,
        );
        Some(PageVisual::Ready {
            texture,
            size: transformed_page_size(
                metrics.width,
                metrics.height,
                self.display_effects().transform,
            ),
            render_info: None,
        })
    }
}
