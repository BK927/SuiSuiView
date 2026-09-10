//! Frame coordination for the optional worker: gather the complete active set,
//! then switch every affected draw together in egui's finish_prepare phase.
use super::*;
use crate::app::realtime_sr::background::{
    BackgroundRefiner, RefineEvent, RefineRequest, RefineSnapshot, INPUT_QUIET, MAX_REQUESTS,
};
use crate::app::realtime_sr::RealtimeSrOutput;
use crate::app::SuiSuiViewApp;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::time::{Duration, Instant};

#[derive(Clone, Default)]
pub(in crate::app) struct RefineStatus {
    pub pending: usize,
    pub ready: usize,
    pub skip_reason: Option<&'static str>,
}

pub(in crate::app) fn status(ctx: &egui::Context) -> RefineStatus {
    ctx.data(|data| data.get_temp(egui::Id::new("background-refine-status")))
        .unwrap_or_default()
}

pub(in crate::app) fn note_input(ctx: &egui::Context) {
    let (has_input, rect) = ctx.input(|input| (!input.events.is_empty(), input.screen_rect()));
    ctx.data_mut(|data| {
        let rect_key = egui::Id::new("background-refine-screen");
        if has_input || data.get_temp::<Rect>(rect_key) != Some(rect) {
            data.insert_temp(egui::Id::new("background-refine-input"), Instant::now());
        }
        data.insert_temp(rect_key, rect);
    });
}

struct ReadyOutput {
    // Retain the worker's result owner as well as the pool wrapper: its weak
    // completion record then knows the UI still owns these pixels.
    _output: Arc<RealtimeSrOutput>,
    intermediate: Arc<GpuIntermediateTexture>,
}

struct RefineDraw {
    key: u64,
    slot: u64,
    effects: ViewEffects,
    downscaler: WgpuDownscaleMethod,
    display: GpuDisplayRect,
    opacity: f32,
    linear: bool,
    inspection: Option<(GpuInspection, GpuDisplayRect, u64)>,
}

#[derive(Default)]
pub(super) struct RefineCoordinator {
    worker: Option<BackgroundRefiner>,
    pass: Option<u64>,
    generation: u64,
    revision: Option<u64>,
    keys: Vec<u64>,
    requests: Vec<RefineRequest>,
    draws: Vec<RefineDraw>,
    outputs: HashMap<u64, ReadyOutput>,
    skips: HashMap<u64, &'static str>,
    source_fences: HashMap<u64, Arc<AtomicBool>>,
    unsubmitted_fences: Vec<Arc<AtomicBool>>,
}

impl RefineCoordinator {
    fn begin_frame(&mut self, pass: u64, queue: &wgpu::Queue, ctx: &egui::Context) {
        if self.pass == Some(pass) {
            return;
        }
        self.pass = Some(pass);
        self.requests.clear();
        self.draws.clear();
        // These sources were recorded in the preceding egui frame. Its encoder
        // was submitted before this frame began, so this callback cannot fire
        // before the source-producing work. Never register in the same frame.
        if !self.unsubmitted_fences.is_empty() {
            let fences = std::mem::take(&mut self.unsubmitted_fences);
            let ctx = ctx.clone();
            queue.on_submitted_work_done(move || {
                for ready in fences {
                    ready.store(true, Ordering::Release);
                }
                ctx.request_repaint();
            });
        }
    }
}

impl GpuPaintResources {
    pub(super) fn finish_normal_sr_frame(&mut self, ctx: &egui::Context) {
        if let Some(active) = self
            .realtime_sr
            .finish_active_frame(ctx.cumulative_pass_nr())
        {
            ctx.data_mut(|data| data.insert_temp(egui::Id::new("normal-span-was-active"), active));
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn enqueue_background_refine(
        &mut self,
        queue: &wgpu::Queue,
        slot: u64,
        source_key: GpuPaintSourceKey,
        source_size: [usize; 2],
        output_size: [usize; 2],
        effects: ViewEffects,
        method: WgpuUpscaleMethod,
        downscaler: WgpuDownscaleMethod,
        display: GpuDisplayRect,
        opacity: f32,
        deband: ResolvedDeband,
        debanded: Option<&Arc<GpuIntermediateTexture>>,
        ctx: &egui::Context,
    ) {
        self.refine.begin_frame(self.current_pass, queue, ctx);
        let stack_passes = method.fixed_2x_stack_passes(output_size, display.full_size);
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        "background-refine-result".hash(&mut hasher);
        source_key.hash(&mut hasher);
        source_size.hash(&mut hasher);
        method.hash(&mut hasher);
        deband.hash(&mut hasher);
        stack_passes.hash(&mut hasher);
        let key = hasher.finish();
        let (texture, view, bytes) = if let Some(source) = debanded {
            (
                source._texture.clone(),
                source._view.clone(),
                source.byte_size,
            )
        } else if let Some(source) = self.source_textures.peek(&source_key) {
            (
                source._texture.clone(),
                source.view.clone(),
                source.byte_size,
            )
        } else {
            return;
        };
        let mut allocation_hash = std::collections::hash_map::DefaultHasher::new();
        texture.hash(&mut allocation_hash);
        let allocation = allocation_hash.finish();
        let ready = self
            .refine
            .source_fences
            .entry(allocation)
            .or_insert_with(|| {
                let ready = Arc::new(AtomicBool::new(false));
                self.refine.unsubmitted_fences.push(ready.clone());
                // Ensure a following frame registers the previous submit fence.
                ctx.request_repaint_after(Duration::from_millis(16));
                ready
            })
            .clone();
        if !self
            .refine
            .requests
            .iter()
            .any(|request| request.key == key)
        {
            self.refine.requests.push(RefineRequest {
                key,
                source_key: allocation,
                method,
                _source_texture: texture,
                source_view: view,
                source_size,
                source_byte_size: bytes,
                stack_passes,
                source_ready: ready,
            });
        }
        self.refine.draws.push(RefineDraw {
            key,
            slot,
            effects,
            downscaler,
            display,
            opacity,
            linear: self.request_linear_downscale,
            inspection: self.request_inspection,
        });
    }

    fn finish_refine_frame(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        ctx: &egui::Context,
    ) {
        self.refine
            .begin_frame(ctx.cumulative_pass_nr(), queue, ctx);
        let mut keys: Vec<_> = self
            .refine
            .requests
            .iter()
            .map(|request| request.key)
            .collect();
        keys.sort_unstable();
        // Result identity only describes SR pixels. Cancellation identity also
        // includes the complete requested presentation policy, so an old job
        // cannot publish after a settings/display change even when SR could be
        // reused once the new request is established.
        let mut presentation: Vec<_> = self
            .refine
            .draws
            .iter()
            .map(|draw| {
                let mut hash = std::collections::hash_map::DefaultHasher::new();
                draw.key.hash(&mut hash);
                draw.effects.hash(&mut hash);
                draw.downscaler.hash(&mut hash);
                draw.linear.hash(&mut hash);
                draw.display.full_size.hash(&mut hash);
                hash.finish()
            })
            .collect();
        presentation.sort_unstable();
        presentation.dedup();
        let mut revision = std::collections::hash_map::DefaultHasher::new();
        keys.hash(&mut revision);
        presentation.hash(&mut revision);
        let revision = revision.finish();
        if self.refine.revision != Some(revision) {
            self.refine.generation = self.refine.generation.wrapping_add(1);
            self.refine.revision = Some(revision);
            self.refine.skips.clear();
        }
        self.refine.keys = keys;
        self.refine
            .outputs
            .retain(|key, _| self.refine.keys.contains(key));
        let active_allocations: HashSet<_> = self
            .refine
            .requests
            .iter()
            .map(|request| request.source_key)
            .collect();
        self.refine
            .source_fences
            .retain(|key, _| active_allocations.contains(key));
        if self.refine.worker.is_none() && !self.refine.requests.is_empty() {
            self.refine.worker = Some(BackgroundRefiner::new(
                device.clone(),
                queue.clone(),
                ctx.clone(),
            ));
        }
        // Keep the same sleeping worker after cancellation. Repeated off/on
        // changes must not create overlapping detached GPU submitters.
        let events = self
            .refine
            .worker
            .as_ref()
            .map(|worker| worker.drain())
            .unwrap_or_default();
        for event in events {
            match event {
                RefineEvent::Ready {
                    generation,
                    key,
                    output,
                } if generation == self.refine.generation && self.refine.keys.contains(&key) => {
                    let bind_group = Arc::new(self.texture_bind_group_for(device, &output.view));
                    let intermediate = Arc::new(GpuIntermediateTexture {
                        _allocation: accounting::TextureAllocation::new(output.byte_size),
                        _texture: output.texture.clone(),
                        _view: output.view.clone(),
                        mip_views: vec![output.texture.create_view(&pools::mip_view_descriptor(0))],
                        bind_group,
                        size: output.size,
                        content_key: key,
                        byte_size: output.byte_size,
                        rendered: AtomicBool::new(true),
                        last_used_pass: AtomicU64::new(self.current_pass),
                    });
                    self.refine.outputs.insert(
                        key,
                        ReadyOutput {
                            _output: output,
                            intermediate,
                        },
                    );
                }
                RefineEvent::Skipped {
                    generation,
                    key,
                    reason,
                } if generation == self.refine.generation && self.refine.keys.contains(&key) => {
                    self.refine.skips.insert(key, reason);
                }
                _ => {}
            }
        }
        let mut input_keys = HashSet::new();
        let input_bytes: usize = self
            .refine
            .requests
            .iter()
            .filter(|r| input_keys.insert(r.source_key))
            .map(|r| r.source_byte_size)
            .sum();
        let output_bytes: usize = self
            .refine
            .outputs
            .values()
            .map(|r| r.intermediate.byte_size)
            .sum();
        let foreground_bytes = self
            .source_texture_bytes
            .saturating_add(accounting::intermediate_bytes());
        let unrelated_bytes =
            foreground_bytes.saturating_sub(input_bytes.saturating_add(output_bytes));
        let budget_bytes = self
            .source_texture_budget_bytes
            .saturating_add(self.intermediate_texture_budget_bytes)
            .saturating_sub(unrelated_bytes);
        let quiet_until = ctx
            .data(|data| data.get_temp::<Instant>(egui::Id::new("background-refine-input")))
            .unwrap_or_else(Instant::now)
            + INPUT_QUIET;
        if let Some(worker) = &self.refine.worker {
            if let Some(reason) = worker.failure_reason() {
                for key in &self.refine.keys {
                    self.refine.skips.insert(*key, reason);
                }
            }
            let requests = if self.refine.requests.len() > MAX_REQUESTS {
                for key in &self.refine.keys {
                    self.refine.skips.insert(*key, "request_limit");
                }
                Vec::new()
            } else {
                self.refine.requests.clone()
            };
            if !worker.update(RefineSnapshot {
                generation: self.refine.generation,
                requests,
                quiet_until,
                budget_bytes,
            }) {
                ctx.request_repaint_after(Duration::from_millis(16));
            }
        }
        let all_ready = !self.refine.keys.is_empty()
            && self
                .refine
                .keys
                .iter()
                .all(|key| self.refine.outputs.contains_key(key));
        if all_ready {
            for intent in std::mem::take(&mut self.refine.draws) {
                let intermediate = self.refine.outputs[&intent.key].intermediate.clone();
                intermediate
                    .last_used_pass
                    .store(self.current_pass, Ordering::Relaxed);
                self.request_linear_downscale = intent.linear;
                let draw = self.prepare_realtime_sr_presentation_draw_state(
                    device,
                    queue,
                    intent.slot,
                    encoder,
                    intent.effects,
                    intent.downscaler,
                    intent.display,
                    intent.opacity,
                    intermediate,
                );
                if let Some((inspection, destination, draw_id)) = intent.inspection {
                    let draw = self.prepare_inspection_draw(
                        device,
                        queue,
                        encoder,
                        draw_id,
                        draw,
                        inspection,
                        destination,
                    );
                    self.insert_draw_state(draw_id, draw);
                } else {
                    self.insert_draw_state(intent.slot, draw);
                }
            }
        }
        let status = RefineStatus {
            pending: self
                .refine
                .keys
                .len()
                .saturating_sub(self.refine.outputs.len()),
            ready: self.refine.outputs.len(),
            skip_reason: self.refine.skips.values().copied().next(),
        };
        self.display_scale_capture.publish(ctx);
        ctx.data_mut(|data| data.insert_temp(egui::Id::new("background-refine-status"), status));
    }
}

struct RefineFrameFinish {
    ctx: egui::Context,
}
impl CallbackTrait for RefineFrameFinish {
    fn finish_prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        resources: &mut CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        if let Some(resources) = resources.get_mut::<GpuPaintResources>() {
            resources.finish_normal_sr_frame(&self.ctx);
            if resources.refine.worker.is_some()
                || self
                    .ctx
                    .data(|data| {
                        data.get_temp::<bool>(egui::Id::new("background-refine-was-active"))
                    })
                    .unwrap_or(false)
            {
                resources.finish_refine_frame(device, queue, encoder, &self.ctx);
            }
        }
        Vec::new()
    }
    fn paint(
        &self,
        _: PaintCallbackInfo,
        _: &mut wgpu::RenderPass<'static>,
        _: &CallbackResources,
    ) {
    }
}

impl SuiSuiViewApp {
    pub(in crate::app) fn paint_refine_frame_finish(&self, painter: &egui::Painter, rect: Rect) {
        let enabled = self.settings.background_refine
            && self.settings.wgpu_upscale_method != WgpuUpscaleMethod::Auto;
        let was_active_key = egui::Id::new("background-refine-was-active");
        let (was_active, normal_span) = painter.ctx().data(|data| {
            (
                data.get_temp::<bool>(was_active_key).unwrap_or(false),
                data.get_temp::<bool>(egui::Id::new("normal-span-was-active"))
                    .unwrap_or(false),
            )
        });
        // Default-off adds no GPU callback and never mutates egui temporary
        // state. One final callback after disabling cancels the previous set.
        if !enabled && !was_active && !normal_span {
            return;
        }
        if enabled || was_active {
            painter
                .ctx()
                .data_mut(|data| data.insert_temp(was_active_key, enabled));
        }
        if self.gpu_target_format.is_some() {
            painter.add(egui_wgpu::Callback::new_paint_callback(
                rect,
                RefineFrameFinish {
                    ctx: painter.ctx().clone(),
                },
            ));
        }
    }
}
