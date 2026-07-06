use super::pools::{
    downscale_intermediate_texture_key, intermediate_texture_key, mip_level_count, mip_size,
    mipmap_intermediate_texture_key, source_texture_content_key, GpuDrawState, GpuIntermediateTexture,
};
use super::{GpuDisplayRect, GpuPaintResources, GpuPaintSourceKey};
use crate::app::realtime_sr::RealtimeSrResources;
use crate::core::effects::ViewEffects;
use crate::core::gpu_effect::{
    output_size_for_effects, params_for_effects, params_for_effects_with_display,
    params_for_effects_with_shader_method, params_for_hardware_mipmap_sample,
    params_for_hardware_mipmap_sample_with_display,
};
#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
use crate::core::perf_trace::{self, PerfField};
use crate::core::state::{WgpuDownscaleMethod, WgpuScalePlan, WgpuUpscaleMethod};
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::Duration;
#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
use std::time::Instant;

impl GpuPaintResources {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn prepare_draw_state(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        source_key: GpuPaintSourceKey,
        source_bind_group: Arc<wgpu::BindGroup>,
        source_size: [usize; 2],
        output_size: [usize; 2],
        effects: ViewEffects,
        wgpu_upscale_method: WgpuUpscaleMethod,
        wgpu_downscale_method: WgpuDownscaleMethod,
        display_rect: GpuDisplayRect,
        opacity: f32,
        ctx: &egui::Context,
    ) -> GpuDrawState {
        let scale_plan = WgpuScalePlan::resolve(
            output_size,
            display_rect.full_size,
            wgpu_upscale_method,
            wgpu_downscale_method,
        );
        let effective_upscaler = scale_plan.effective_upscale_method;
        let effective_downscaler = scale_plan.effective_downscale_method;
        let source_content_key =
            source_texture_content_key(source_key, source_size, output_size, effects);
        self.realtime_sr
            .cancel_inactive_pending_work(effective_upscaler);
        if RealtimeSrResources::is_supported(effective_upscaler) {
            if let Some(draw_state) = self.prepare_realtime_sr_draw_state(
                device,
                encoder,
                source_key,
                source_size,
                output_size,
                effects,
                effective_upscaler,
                wgpu_downscale_method,
                display_rect,
                opacity,
                ctx,
            ) {
                return draw_state;
            }
        }
        if let Some(rcas_method) = effective_upscaler.rcas_shader_method_id() {
            let intermediate_key = intermediate_texture_key(
                source_key,
                source_size,
                output_size,
                effects,
                effective_upscaler,
                display_rect,
            );
            self.ensure_intermediate_texture(device, intermediate_key, display_rect.visible_size);
            let intermediate = self
                .intermediate_textures
                .peek(&intermediate_key)
                .expect("intermediate texture should be cached before rendering")
                .clone();
            let intermediate_bind_group = intermediate.bind_group.clone();
            let intermediate_view = intermediate
                .mip_views
                .first()
                .expect("intermediate textures should expose a renderable mip 0 view");
            let easu_params = params_for_effects_with_display(
                source_size,
                output_size,
                effects,
                effective_upscaler,
                WgpuDownscaleMethod::Bilinear,
                [0, 0],
                display_rect.visible_size,
                display_rect.sample_offset,
                display_rect.full_size,
                1.0,
            );
            let easu_params_bind_group = self.params_bind_group_for(device, easu_params);
            self.render_fullscreen(
                encoder,
                intermediate_view,
                &source_bind_group,
                &easu_params_bind_group,
            );

            let rcas_params = params_for_effects_with_shader_method(
                [
                    display_rect.visible_size[0] as usize,
                    display_rect.visible_size[1] as usize,
                ],
                [
                    display_rect.visible_size[0] as usize,
                    display_rect.visible_size[1] as usize,
                ],
                ViewEffects::default(),
                rcas_method,
                0,
                display_rect.origin,
                display_rect.visible_size,
                opacity,
            );
            let params_bind_group = self.params_bind_group_for(device, rcas_params);
            record_wgpu_upscale_method_render(
                effective_upscaler,
                source_size,
                output_size,
                display_rect.full_size,
                [
                    display_rect.visible_size[0] as usize,
                    display_rect.visible_size[1] as usize,
                ],
                "easu_rcas",
            );
            return GpuDrawState::new(
                intermediate_bind_group,
                params_bind_group,
                vec![intermediate],
            );
        }

        // Unreachable in product after settings sanitize (HardwareMipmapLinear folds
        // to Bilinear); retained for tests and potential future re-exposure.
        if effective_downscaler.is_hardware_mipmap() {
            return self.prepare_hardware_mipmap_draw_state(
                device,
                encoder,
                source_content_key,
                source_bind_group,
                source_size,
                output_size,
                effects,
                display_rect,
                opacity,
            );
        }

        if effective_downscaler.is_pyramid()
            && !display_rect.is_clipped()
            && needs_multi_pass_downscale(output_size, display_rect.full_size)
        {
            return self.prepare_pyramid_downscale_draw_state(
                device,
                encoder,
                source_content_key,
                source_bind_group,
                source_size,
                output_size,
                effects,
                effective_downscaler,
                display_rect.origin,
                display_rect.visible_size,
                opacity,
            );
        }

        if effective_upscaler.shader_method_id() != 0 {
            record_wgpu_upscale_method_render(
                effective_upscaler,
                source_size,
                output_size,
                display_rect.full_size,
                display_rect
                    .visible_size
                    .map(|dimension| dimension as usize),
                "single_pass",
            );
        }
        let params = params_for_effects_with_display(
            source_size,
            output_size,
            effects,
            effective_upscaler,
            effective_downscaler,
            display_rect.origin,
            display_rect.visible_size,
            display_rect.sample_offset,
            display_rect.full_size,
            opacity,
        );
        GpuDrawState::new(
            source_bind_group,
            self.params_bind_group_for(device, params),
            Vec::new(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn prepare_realtime_sr_draw_state(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        source_key: GpuPaintSourceKey,
        source_size: [usize; 2],
        output_size: [usize; 2],
        effects: ViewEffects,
        upscaler: WgpuUpscaleMethod,
        downscaler: WgpuDownscaleMethod,
        display_rect: GpuDisplayRect,
        opacity: f32,
        ctx: &egui::Context,
    ) -> Option<GpuDrawState> {
        let stack_passes = upscaler.fixed_2x_stack_passes(output_size, display_rect.full_size);
        let mut current_size = source_size;
        let mut current_intermediate: Option<Arc<GpuIntermediateTexture>> = None;
        let mut best_ready: Option<Arc<GpuIntermediateTexture>> = None;

        for stage_index in 0..stack_passes {
            let stage_key = realtime_sr_stage_texture_key(
                source_key,
                source_size,
                upscaler,
                stage_index,
                current_size,
                stack_passes,
            );
            if self.should_defer_realtime_sr_first_frame(stage_key, upscaler) {
                self.realtime_sr.warm_up_async(upscaler, device);
                ctx.request_repaint_after(Duration::from_millis(16));
                break;
            }

            let next_intermediate = if stage_index == 0 {
                self.ensure_realtime_sr_stage_texture_from_source(
                    device,
                    encoder,
                    stage_key,
                    source_key,
                    current_size,
                    upscaler,
                )
            } else {
                let input = current_intermediate.as_ref()?;
                self.ensure_realtime_sr_stage_texture_from_view(
                    device,
                    encoder,
                    stage_key,
                    &input._view,
                    current_size,
                    upscaler,
                )
            };
            if self.realtime_sr.has_pending_async_work(upscaler) {
                ctx.request_repaint_after(Duration::from_millis(16));
            }
            let Some(next_intermediate) = next_intermediate else {
                break;
            };

            current_size = next_intermediate.size;
            best_ready = Some(next_intermediate.clone());
            current_intermediate = Some(next_intermediate);
        }

        let intermediate = best_ready?;
        record_wgpu_upscale_method_render(
            upscaler,
            source_size,
            output_size,
            display_rect.full_size,
            intermediate.size,
            if stack_passes > 1 {
                "realtime_sr_stacked"
            } else {
                "realtime_sr"
            },
        );
        Some(self.prepare_realtime_sr_presentation_draw_state(
            device,
            encoder,
            effects,
            downscaler,
            display_rect,
            opacity,
            intermediate,
        ))
    }

    #[allow(clippy::too_many_arguments)]
    fn prepare_realtime_sr_presentation_draw_state(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        effects: ViewEffects,
        downscaler: WgpuDownscaleMethod,
        display_rect: GpuDisplayRect,
        opacity: f32,
        intermediate: Arc<GpuIntermediateTexture>,
    ) -> GpuDrawState {
        let sr_output_size = output_size_for_effects(intermediate.size, effects);
        let post_downscaler =
            post_realtime_sr_downscale_method(sr_output_size, display_rect.full_size, downscaler);
        // Unreachable in product after settings sanitize (HardwareMipmapLinear folds
        // to Bilinear); retained for tests and potential future re-exposure.
        if post_downscaler.is_hardware_mipmap() {
            return self
                .prepare_hardware_mipmap_draw_state(
                    device,
                    encoder,
                    intermediate.content_key,
                    intermediate.bind_group.clone(),
                    intermediate.size,
                    sr_output_size,
                    effects,
                    display_rect,
                    opacity,
                )
                .with_intermediate_pin(intermediate);
        }
        if post_downscaler.is_pyramid()
            && !display_rect.is_clipped()
            && needs_multi_pass_downscale(sr_output_size, display_rect.full_size)
        {
            return self
                .prepare_pyramid_downscale_draw_state(
                    device,
                    encoder,
                    intermediate.content_key,
                    intermediate.bind_group.clone(),
                    intermediate.size,
                    sr_output_size,
                    effects,
                    post_downscaler,
                    display_rect.origin,
                    display_rect.visible_size,
                    opacity,
                )
                .with_intermediate_pin(intermediate);
        }

        let params = params_for_effects_with_display(
            intermediate.size,
            sr_output_size,
            effects,
            WgpuUpscaleMethod::None,
            post_downscaler,
            display_rect.origin,
            display_rect.visible_size,
            display_rect.sample_offset,
            display_rect.full_size,
            opacity,
        );
        GpuDrawState::new(
            intermediate.bind_group.clone(),
            self.params_bind_group_for(device, params),
            vec![intermediate],
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn prepare_pyramid_downscale_draw_state(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        content_key: u64,
        source_bind_group: Arc<wgpu::BindGroup>,
        source_size: [usize; 2],
        output_size: [usize; 2],
        effects: ViewEffects,
        downscaler: WgpuDownscaleMethod,
        origin: [u32; 2],
        target_size: [u32; 2],
        opacity: f32,
    ) -> GpuDrawState {
        let mut pins = Vec::new();
        let mut current_bind_group = source_bind_group.clone();
        let mut current_size = output_size;
        let mut first_stage = true;
        let mut stage_index = 0u32;

        while needs_multi_pass_downscale(current_size, target_size) {
            let stage_size = next_pyramid_stage_size(current_size, target_size);
            if stage_size == current_size.map(|dimension| dimension as u32) {
                break;
            }
            let stage_filter = downscaler.pyramid_stage_filter();
            let intermediate = self.render_downscale_stage(
                device,
                encoder,
                content_key,
                source_size,
                output_size,
                effects,
                downscaler,
                stage_filter,
                &current_bind_group,
                current_size,
                stage_size,
                stage_index,
                first_stage,
            );
            current_bind_group = intermediate.bind_group.clone();
            current_size = intermediate.size;
            pins.push(intermediate);
            first_stage = false;
            stage_index = stage_index.saturating_add(1);
        }

        let final_size = target_size.map(|dimension| dimension.max(1) as usize);
        if current_size != final_size {
            let intermediate = self.render_downscale_stage(
                device,
                encoder,
                content_key,
                source_size,
                output_size,
                effects,
                downscaler,
                downscaler.base_filter(),
                &current_bind_group,
                current_size,
                target_size.map(|dimension| dimension.max(1)),
                stage_index,
                first_stage,
            );
            current_bind_group = intermediate.bind_group.clone();
            current_size = intermediate.size;
            pins.push(intermediate);
        }

        if pins.is_empty() {
            let params = params_for_effects(
                source_size,
                output_size,
                effects,
                WgpuUpscaleMethod::None,
                downscaler.base_filter(),
                origin,
                target_size,
                opacity,
            );
            return GpuDrawState::new(
                source_bind_group,
                self.params_bind_group_for(device, params),
                Vec::new(),
            );
        }

        let params = params_for_effects(
            current_size,
            current_size,
            ViewEffects::default(),
            WgpuUpscaleMethod::None,
            WgpuDownscaleMethod::Bilinear,
            origin,
            target_size,
            opacity,
        );
        GpuDrawState::new(
            current_bind_group,
            self.params_bind_group_for(device, params),
            pins,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn render_downscale_stage(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        content_key: u64,
        source_size: [usize; 2],
        output_size: [usize; 2],
        effects: ViewEffects,
        downscaler: WgpuDownscaleMethod,
        stage_filter: WgpuDownscaleMethod,
        current_bind_group: &wgpu::BindGroup,
        current_size: [usize; 2],
        stage_size: [u32; 2],
        stage_index: u32,
        first_stage: bool,
    ) -> Arc<GpuIntermediateTexture> {
        let stage_size = stage_size.map(|dimension| dimension.max(1));
        let stage_key = downscale_intermediate_texture_key(
            "pyramid",
            content_key,
            downscaler,
            [stage_size[0], stage_size[1]],
            current_size,
            stage_index,
        );
        self.ensure_intermediate_texture(device, stage_key, stage_size);
        let intermediate = self
            .intermediate_textures
            .peek(&stage_key)
            .expect("pyramid stage texture should be cached before rendering")
            .clone();
        let params = if first_stage {
            params_for_effects(
                source_size,
                output_size,
                effects,
                WgpuUpscaleMethod::None,
                stage_filter,
                [0, 0],
                stage_size,
                1.0,
            )
        } else {
            params_for_effects(
                current_size,
                current_size,
                ViewEffects::default(),
                WgpuUpscaleMethod::None,
                stage_filter,
                [0, 0],
                stage_size,
                1.0,
            )
        };
        let params_bind_group = self.params_bind_group_for(device, params);
        let stage_view = intermediate
            .mip_views
            .first()
            .expect("pyramid stage textures should expose a renderable mip 0 view");
        self.render_fullscreen(encoder, stage_view, current_bind_group, &params_bind_group);
        intermediate
    }

    #[allow(clippy::too_many_arguments)]
    fn prepare_hardware_mipmap_draw_state(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        content_key: u64,
        source_bind_group: Arc<wgpu::BindGroup>,
        source_size: [usize; 2],
        output_size: [usize; 2],
        effects: ViewEffects,
        display_rect: GpuDisplayRect,
        opacity: f32,
    ) -> GpuDrawState {
        let mip_levels = mip_level_count(output_size);
        let mip_key = mipmap_intermediate_texture_key(content_key);
        self.ensure_mipmapped_intermediate_texture(
            device,
            mip_key,
            output_size.map(|dimension| dimension.max(1) as u32),
            mip_levels,
        );
        let intermediate = self
            .intermediate_textures
            .peek(&mip_key)
            .expect("mipmapped intermediate texture should be cached before rendering")
            .clone();
        let mip0_params = params_for_effects(
            source_size,
            output_size,
            effects,
            WgpuUpscaleMethod::None,
            WgpuDownscaleMethod::Bilinear,
            [0, 0],
            output_size.map(|dimension| dimension.max(1) as u32),
            1.0,
        );
        let mip0_params_bind_group = self.params_bind_group_for(device, mip0_params);
        self.render_fullscreen(
            encoder,
            &intermediate.mip_views[0],
            &source_bind_group,
            &mip0_params_bind_group,
        );

        for level in 1..mip_levels {
            let prev_size = mip_size(output_size, level - 1);
            let next_size = mip_size(output_size, level);
            let prev_bind_group =
                self.texture_bind_group_for(device, &intermediate.mip_views[level as usize - 1]);
            let params = params_for_hardware_mipmap_sample(
                prev_size,
                [0, 0],
                next_size.map(|dimension| dimension as u32),
                1.0,
                0.0,
            );
            let params_bind_group = self.params_bind_group_for(device, params);
            self.render_fullscreen(
                encoder,
                &intermediate.mip_views[level as usize],
                &prev_bind_group,
                &params_bind_group,
            );
        }

        let lod = downscale_lod(output_size, display_rect.full_size)
            .min(mip_levels.saturating_sub(1) as f32);
        let params = params_for_hardware_mipmap_sample_with_display(
            output_size,
            display_rect.origin,
            display_rect.visible_size,
            display_rect.sample_offset,
            display_rect.full_size,
            opacity,
            lod,
        );
        GpuDrawState::new(
            intermediate.bind_group.clone(),
            self.params_bind_group_for(device, params),
            vec![intermediate],
        )
    }

    fn should_defer_realtime_sr_first_frame(
        &mut self,
        key: u64,
        method: WgpuUpscaleMethod,
    ) -> bool {
        if !defer_initial_realtime_sr_frame(method)
            || self.intermediate_textures.peek(&key).is_some()
        {
            return false;
        }
        if self.deferred_realtime_sr_first_frames.get(&key).is_some() {
            return false;
        }
        self.deferred_realtime_sr_first_frames.push(key, ());
        true
    }

    fn ensure_realtime_sr_stage_texture_from_source(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        key: u64,
        source_key: GpuPaintSourceKey,
        source_size: [usize; 2],
        method: WgpuUpscaleMethod,
    ) -> Option<Arc<GpuIntermediateTexture>> {
        if let Some(intermediate) = self.intermediate_textures.get(&key).cloned() {
            return Some(intermediate);
        }
        let Some(source) = self.source_textures.peek(&source_key) else {
            return None;
        };
        let Some(output) =
            self.realtime_sr
                .render(method, key, device, encoder, &source.view, source_size)
        else {
            return None;
        };
        Some(self.insert_realtime_sr_stage_texture(device, key, method, source_size, output))
    }

    fn ensure_realtime_sr_stage_texture_from_view(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        key: u64,
        source_view: &wgpu::TextureView,
        source_size: [usize; 2],
        method: WgpuUpscaleMethod,
    ) -> Option<Arc<GpuIntermediateTexture>> {
        if let Some(intermediate) = self.intermediate_textures.get(&key).cloned() {
            return Some(intermediate);
        }
        let Some(output) =
            self.realtime_sr
                .render(method, key, device, encoder, source_view, source_size)
        else {
            return None;
        };
        Some(self.insert_realtime_sr_stage_texture(device, key, method, source_size, output))
    }

    fn insert_realtime_sr_stage_texture(
        &mut self,
        device: &wgpu::Device,
        key: u64,
        method: WgpuUpscaleMethod,
        source_size: [usize; 2],
        output: crate::app::realtime_sr::RealtimeSrOutput,
    ) -> Arc<GpuIntermediateTexture> {
        let output_size = output.size;
        let output_byte_size = output.byte_size;
        let bind_group = Arc::new(self.texture_bind_group_for(device, &output.view));
        let mip_views = vec![output.texture.create_view(&super::pools::mip_view_descriptor(0))];
        let intermediate = Arc::new(GpuIntermediateTexture {
            _texture: output.texture,
            _view: output.view,
            mip_views,
            bind_group,
            size: output_size,
            content_key: key,
            byte_size: output_byte_size,
        });
        let evicted_on_insert = if let Some((_old_key, old_texture)) =
            self.intermediate_textures.push(key, intermediate.clone())
        {
            self.intermediate_texture_bytes = self
                .intermediate_texture_bytes
                .saturating_sub(old_texture.byte_size);
            true
        } else {
            false
        };
        self.intermediate_texture_bytes = self
            .intermediate_texture_bytes
            .saturating_add(output_byte_size);
        self.prune_intermediate_textures();
        record_realtime_sr_texture_ready(
            method,
            source_size,
            output_size,
            output_byte_size,
            self.intermediate_textures.len(),
            self.intermediate_texture_bytes,
            evicted_on_insert,
        );
        intermediate
    }

    pub(super) fn render_fullscreen(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        output_view: &wgpu::TextureView,
        texture_bind_group: &wgpu::BindGroup,
        params_bind_group: &wgpu::BindGroup,
    ) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("suisuiview-gpu-upscale-intermediate-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: output_view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&self.intermediate_pipeline);
        pass.set_bind_group(0, texture_bind_group, &[]);
        pass.set_bind_group(1, params_bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
fn record_wgpu_upscale_method_render(
    method: WgpuUpscaleMethod,
    source_size: [usize; 2],
    output_size: [usize; 2],
    target_size: [u32; 2],
    rendered_size: [usize; 2],
    path: &'static str,
) {
    perf_trace::record_duration(
        "wgpu_upscale_method_render",
        Duration::ZERO,
        &[
            PerfField::Str("method", method.token()),
            PerfField::Str("path", path),
            PerfField::Usize("source_width", source_size[0]),
            PerfField::Usize("source_height", source_size[1]),
            PerfField::Usize("output_width", output_size[0]),
            PerfField::Usize("output_height", output_size[1]),
            PerfField::U32("target_width", target_size[0]),
            PerfField::U32("target_height", target_size[1]),
            PerfField::Usize("rendered_width", rendered_size[0]),
            PerfField::Usize("rendered_height", rendered_size[1]),
        ],
    );
}

#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
fn record_realtime_sr_texture_ready(
    method: WgpuUpscaleMethod,
    source_size: [usize; 2],
    output_size: [usize; 2],
    output_byte_size: usize,
    cache_entries: usize,
    cache_bytes: usize,
    evicted_on_insert: bool,
) {
    perf_trace::record_duration(
        "realtime_sr_texture_ready",
        Duration::ZERO,
        &[
            PerfField::Str("method", method.token()),
            PerfField::Usize("source_width", source_size[0]),
            PerfField::Usize("source_height", source_size[1]),
            PerfField::Usize("output_width", output_size[0]),
            PerfField::Usize("output_height", output_size[1]),
            PerfField::Usize("output_bytes", output_byte_size),
            PerfField::Usize("cache_entries", cache_entries),
            PerfField::Usize("cache_bytes", cache_bytes),
            PerfField::Bool("evicted_on_insert", evicted_on_insert),
        ],
    );
}

#[cfg(not(any(feature = "perf-dev", feature = "perf-diagnostics")))]
fn record_realtime_sr_texture_ready(
    _method: WgpuUpscaleMethod,
    _source_size: [usize; 2],
    _output_size: [usize; 2],
    _output_byte_size: usize,
    _cache_entries: usize,
    _cache_bytes: usize,
    _evicted_on_insert: bool,
) {
}

#[cfg(not(any(feature = "perf-dev", feature = "perf-diagnostics")))]
fn record_wgpu_upscale_method_render(
    _method: WgpuUpscaleMethod,
    _source_size: [usize; 2],
    _output_size: [usize; 2],
    _target_size: [u32; 2],
    _rendered_size: [usize; 2],
    _path: &'static str,
) {
}

pub(super) fn needs_multi_pass_downscale(source_size: [usize; 2], target_size: [u32; 2]) -> bool {
    downscale_ratio(source_size, target_size) > 2.0
}

fn downscale_ratio(source_size: [usize; 2], target_size: [u32; 2]) -> f32 {
    let target_width = target_size[0].max(1) as f32;
    let target_height = target_size[1].max(1) as f32;
    ((source_size[0].max(1) as f32) / target_width)
        .max((source_size[1].max(1) as f32) / target_height)
}

fn downscale_lod(source_size: [usize; 2], target_size: [u32; 2]) -> f32 {
    downscale_ratio(source_size, target_size).max(1.0).log2()
}

pub(super) fn next_pyramid_stage_size(current_size: [usize; 2], target_size: [u32; 2]) -> [u32; 2] {
    [
        next_pyramid_stage_dimension(current_size[0], target_size[0]),
        next_pyramid_stage_dimension(current_size[1], target_size[1]),
    ]
}

fn next_pyramid_stage_dimension(current: usize, target: u32) -> u32 {
    let current = current.max(1);
    let target = target.max(1) as usize;
    if target >= current {
        current as u32
    } else if current > target.saturating_mul(2) {
        ((current + 1) / 2).max(target) as u32
    } else {
        target as u32
    }
}

pub(super) fn create_effect_pipeline_timed(
    device: &wgpu::Device,
    shader: &wgpu::ShaderModule,
    pipeline_layout: &wgpu::PipelineLayout,
    target_format: wgpu::TextureFormat,
    label: &'static str,
) -> wgpu::RenderPipeline {
    #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
    let started = Instant::now();
    let pipeline = create_effect_pipeline(device, shader, pipeline_layout, target_format, label);
    #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
    perf_trace::record_duration(
        "gpu_effect_pipeline_create",
        started.elapsed(),
        &[
            PerfField::Str("label", label),
            PerfField::Str(
                "target_format",
                super::pools::texture_format_label(target_format),
            ),
        ],
    );
    pipeline
}

fn create_effect_pipeline(
    device: &wgpu::Device,
    shader: &wgpu::ShaderModule,
    pipeline_layout: &wgpu::PipelineLayout,
    target_format: wgpu::TextureFormat,
    label: &'static str,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(pipeline_layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format: target_format,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    })
}

pub(super) fn defer_initial_realtime_sr_frame(method: WgpuUpscaleMethod) -> bool {
    method.is_artcnn() || matches!(method, WgpuUpscaleMethod::WgslSrLabSpanX2)
}

pub(super) fn post_realtime_sr_downscale_method(
    output_size: [usize; 2],
    target_size: [u32; 2],
    requested_downscaler: WgpuDownscaleMethod,
) -> WgpuDownscaleMethod {
    requested_downscaler.resolve_for_downscale(output_size, target_size)
}

pub(super) fn realtime_sr_stage_texture_key(
    source_key: GpuPaintSourceKey,
    base_source_size: [usize; 2],
    wgpu_upscale_method: WgpuUpscaleMethod,
    stage_index: usize,
    input_size: [usize; 2],
    stack_passes: usize,
) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    "realtime_sr_stage".hash(&mut hasher);
    source_key.hash(&mut hasher);
    base_source_size.hash(&mut hasher);
    wgpu_upscale_method.token().hash(&mut hasher);
    stage_index.hash(&mut hasher);
    input_size.hash(&mut hasher);
    stack_passes.hash(&mut hasher);
    hasher.finish()
}
