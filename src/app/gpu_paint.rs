use super::{PageCacheKey, SuiSuiViewApp};
use crate::core::effects::ViewEffects;
use crate::core::gpu_effect::{
    output_size_for_effects, params_for_effects, params_for_effects_with_shader_method,
};
#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
use crate::core::perf_trace::{self, PerfField};
use crate::core::state::{DisplayUpscaler, GpuEffectMode};
use eframe::egui::{self, PaintCallbackInfo, Rect};
use egui_wgpu::{CallbackResources, CallbackTrait, ScreenDescriptor};
use lru::LruCache;
use std::borrow::Cow;
use std::hash::{Hash, Hasher};
use std::num::NonZeroUsize;
use std::sync::Arc;
#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
use std::time::{Duration, Instant};
use wgpu::util::DeviceExt;

pub(super) const GPU_SOURCE_TEXTURE_BUDGET_BYTES: usize = 192 * 1024 * 1024;
pub(super) const GPU_INTERMEDIATE_TEXTURE_BUDGET_BYTES: usize = 256 * 1024 * 1024;
const GPU_SOURCE_TEXTURE_CACHE_LIMIT: usize = 32;
const GPU_DRAW_BIND_GROUP_CACHE_LIMIT: usize = 16;
const GPU_INTERMEDIATE_TEXTURE_CACHE_LIMIT: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct GpuPaintSourceKey {
    pub(super) book: u64,
    pub(super) page: PageCacheKey,
    pub(super) upscaled: bool,
    pub(super) generation: u64,
}

pub(super) struct GpuPaintRequest {
    pub(super) rect: Rect,
    pub(super) source_key: GpuPaintSourceKey,
    pub(super) image_size: [usize; 2],
    pub(super) rgba: Arc<[u8]>,
    pub(super) effects: ViewEffects,
    pub(super) display_upscaler: DisplayUpscaler,
    pub(super) opacity: f32,
}

impl SuiSuiViewApp {
    pub(super) fn gpu_paint_book_key(&self) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.book_id.hash(&mut hasher);
        self.opened_path.hash(&mut hasher);
        hasher.finish()
    }

    pub(super) fn active_display_upscaler(&self) -> DisplayUpscaler {
        if !self.gpu_effects_available
            || self.gpu_target_format.is_none()
            || matches!(self.settings.gpu_effect_mode, GpuEffectMode::CpuOnly)
        {
            return DisplayUpscaler::None;
        }
        match self.settings.display_upscaler {
            DisplayUpscaler::None => DisplayUpscaler::None,
            upscaler => upscaler,
        }
    }

    pub(super) fn can_paint_wgsl_effects(&self) -> bool {
        let display_upscaler = self.active_display_upscaler();
        self.gpu_effects_available
            && (self.effects != ViewEffects::default() || display_upscaler != DisplayUpscaler::None)
            && matches!(
                self.settings.gpu_effect_mode,
                GpuEffectMode::Auto | GpuEffectMode::Wgsl
            )
            && self.gpu_target_format.is_some()
    }

    pub(super) fn paint_wgsl_effects(
        &self,
        painter: &egui::Painter,
        request: GpuPaintRequest,
    ) -> bool {
        let Some(target_format) = self.gpu_target_format else {
            return false;
        };
        let callback = GpuEffectCallback {
            source_key: request.source_key,
            image_size: request.image_size,
            rgba: request.rgba,
            effects: request.effects,
            display_upscaler: request.display_upscaler,
            opacity: request.opacity.clamp(0.0, 1.0),
            rect: request.rect,
            target_format,
            draw_id: draw_id(
                request.source_key,
                request.effects,
                request.display_upscaler,
                request.rect,
                request.opacity,
            ),
        };
        painter.add(egui_wgpu::Callback::new_paint_callback(
            request.rect,
            callback,
        ));
        true
    }
}

struct GpuEffectCallback {
    source_key: GpuPaintSourceKey,
    image_size: [usize; 2],
    rgba: Arc<[u8]>,
    effects: ViewEffects,
    display_upscaler: DisplayUpscaler,
    opacity: f32,
    rect: Rect,
    target_format: wgpu::TextureFormat,
    draw_id: u64,
}

impl CallbackTrait for GpuEffectCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        screen_descriptor: &ScreenDescriptor,
        egui_encoder: &mut wgpu::CommandEncoder,
        callback_resources: &mut CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        if callback_resources.get::<GpuPaintResources>().is_none() {
            callback_resources.insert(GpuPaintResources::new(device, self.target_format));
        }
        let resources = callback_resources
            .get_mut::<GpuPaintResources>()
            .expect("GPU paint resources should be inserted before use");
        if resources.target_format != self.target_format {
            *resources = GpuPaintResources::new(device, self.target_format);
        }
        resources.ensure_source_texture(
            device,
            queue,
            self.source_key,
            self.image_size,
            &self.rgba,
        );

        let output_size = output_size_for_effects(self.image_size, self.effects);
        let (origin, target_size) = viewport_rect(self.rect, screen_descriptor);
        if let Some(source_view) = resources
            .source_textures
            .peek(&self.source_key)
            .map(|source| source.view.clone())
        {
            let draw_state = resources.prepare_draw_state(
                device,
                egui_encoder,
                self.source_key,
                &source_view,
                self.image_size,
                output_size,
                self.effects,
                self.display_upscaler,
                origin,
                target_size,
                self.opacity,
            );
            resources.insert_draw_state(self.draw_id, draw_state);
        }
        Vec::new()
    }

    fn paint(
        &self,
        _info: PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        callback_resources: &CallbackResources,
    ) {
        let Some(resources) = callback_resources.get::<GpuPaintResources>() else {
            return;
        };
        let Some(draw_state) = resources.draw_bind_groups.peek(&self.draw_id) else {
            return;
        };
        render_pass.set_pipeline(&resources.pipeline);
        render_pass.set_bind_group(0, &draw_state.bind_group, &[]);
        render_pass.draw(0..3, 0..1);
    }
}

struct GpuPaintResources {
    target_format: wgpu::TextureFormat,
    bind_group_layout: wgpu::BindGroupLayout,
    pipeline: wgpu::RenderPipeline,
    source_textures: LruCache<GpuPaintSourceKey, GpuSourceTexture>,
    source_texture_bytes: usize,
    draw_bind_groups: LruCache<u64, GpuDrawState>,
    draw_state_intermediate_bytes: usize,
    intermediate_textures: LruCache<u64, Arc<GpuIntermediateTexture>>,
    intermediate_texture_bytes: usize,
}

struct GpuSourceTexture {
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
    byte_size: usize,
}

struct GpuDrawState {
    bind_group: wgpu::BindGroup,
    _intermediate_pin: Option<Arc<GpuIntermediateTexture>>,
    intermediate_byte_size: usize,
}

struct GpuIntermediateTexture {
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
    byte_size: usize,
}

impl GpuDrawState {
    fn new(
        bind_group: wgpu::BindGroup,
        intermediate_pin: Option<Arc<GpuIntermediateTexture>>,
    ) -> Self {
        let intermediate_byte_size = intermediate_pin
            .as_ref()
            .map_or(0, |texture| texture.byte_size);
        Self {
            bind_group,
            _intermediate_pin: intermediate_pin,
            intermediate_byte_size,
        }
    }
}

impl GpuPaintResources {
    fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("suisuiview-gpu-effect-shader"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(include_str!(
                "../core/gpu_effect.wgsl"
            ))),
        });
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("suisuiview-gpu-effect-layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("suisuiview-gpu-effect-pipeline-layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("suisuiview-gpu-effect-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
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
        });
        Self {
            target_format,
            bind_group_layout,
            pipeline,
            source_textures: LruCache::new(
                NonZeroUsize::new(GPU_SOURCE_TEXTURE_CACHE_LIMIT).unwrap(),
            ),
            source_texture_bytes: 0,
            draw_bind_groups: LruCache::new(
                NonZeroUsize::new(GPU_DRAW_BIND_GROUP_CACHE_LIMIT).unwrap(),
            ),
            draw_state_intermediate_bytes: 0,
            intermediate_textures: LruCache::new(
                NonZeroUsize::new(GPU_INTERMEDIATE_TEXTURE_CACHE_LIMIT).unwrap(),
            ),
            intermediate_texture_bytes: 0,
        }
    }

    fn ensure_source_texture(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        key: GpuPaintSourceKey,
        image_size: [usize; 2],
        rgba: &[u8],
    ) {
        if self.source_textures.get(&key).is_some() {
            return;
        }
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        let upload_started = Instant::now();
        let [width, height] = image_size;
        let byte_size = width.saturating_mul(height).saturating_mul(4);
        if rgba.len() != byte_size {
            return;
        }
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("suisuiview-gpu-effect-source"),
            size: wgpu::Extent3d {
                width: width as u32,
                height: height as u32,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some((width * 4) as u32),
                rows_per_image: Some(height as u32),
            },
            wgpu::Extent3d {
                width: width as u32,
                height: height as u32,
                depth_or_array_layers: 1,
            },
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        if let Some((_old_key, old_texture)) = self.source_textures.push(
            key,
            GpuSourceTexture {
                _texture: texture,
                view,
                byte_size,
            },
        ) {
            self.source_texture_bytes = self
                .source_texture_bytes
                .saturating_sub(old_texture.byte_size);
        }
        self.source_texture_bytes = self.source_texture_bytes.saturating_add(byte_size);
        self.prune_source_textures();
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        perf_trace::record_duration_if_at_least(
            "gpu_texture_upload",
            upload_started.elapsed(),
            Duration::from_millis(16),
            &[
                PerfField::Usize("width", width),
                PerfField::Usize("height", height),
                PerfField::Bool("upscaled", key.upscaled),
            ],
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn prepare_draw_state(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        source_key: GpuPaintSourceKey,
        source_view: &wgpu::TextureView,
        source_size: [usize; 2],
        output_size: [usize; 2],
        effects: ViewEffects,
        display_upscaler: DisplayUpscaler,
        origin: [u32; 2],
        target_size: [u32; 2],
        opacity: f32,
    ) -> GpuDrawState {
        let effective_upscaler = display_upscaler
            .resolve_for_render(output_size, target_size)
            .unwrap_or(DisplayUpscaler::None);
        if let Some(rcas_method) = effective_upscaler.rcas_shader_method_id() {
            let intermediate_key = intermediate_texture_key(
                source_key,
                source_size,
                output_size,
                effects,
                effective_upscaler,
                target_size,
            );
            self.ensure_intermediate_texture(device, intermediate_key, target_size);
            let intermediate = self
                .intermediate_textures
                .peek(&intermediate_key)
                .expect("intermediate texture should be cached before rendering")
                .clone();
            let intermediate_view = &intermediate.view;
            let easu_params = params_for_effects(
                source_size,
                output_size,
                effects,
                effective_upscaler,
                [0, 0],
                target_size,
                1.0,
            );
            let easu_bind_group = self.bind_group_for(device, source_view, easu_params);
            self.render_fullscreen(encoder, intermediate_view, &easu_bind_group);

            let rcas_params = params_for_effects_with_shader_method(
                [target_size[0] as usize, target_size[1] as usize],
                [target_size[0] as usize, target_size[1] as usize],
                ViewEffects::default(),
                rcas_method,
                origin,
                target_size,
                opacity,
            );
            let bind_group = self.bind_group_for(device, intermediate_view, rcas_params);
            return GpuDrawState::new(bind_group, Some(intermediate));
        }

        let params = params_for_effects(
            source_size,
            output_size,
            effects,
            effective_upscaler,
            origin,
            target_size,
            opacity,
        );
        GpuDrawState::new(self.bind_group_for(device, source_view, params), None)
    }

    fn ensure_intermediate_texture(
        &mut self,
        device: &wgpu::Device,
        key: u64,
        target_size: [u32; 2],
    ) {
        if self.intermediate_textures.get(&key).is_some() {
            return;
        }
        let byte_size = (target_size[0] as usize)
            .saturating_mul(target_size[1] as usize)
            .saturating_mul(4);
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("suisuiview-gpu-upscale-intermediate"),
            size: wgpu::Extent3d {
                width: target_size[0],
                height: target_size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        if let Some((_old_key, old_texture)) = self.intermediate_textures.push(
            key,
            Arc::new(GpuIntermediateTexture {
                _texture: texture,
                view,
                byte_size,
            }),
        ) {
            self.intermediate_texture_bytes = self
                .intermediate_texture_bytes
                .saturating_sub(old_texture.byte_size);
        }
        self.intermediate_texture_bytes = self.intermediate_texture_bytes.saturating_add(byte_size);
        self.prune_intermediate_textures();
    }

    fn insert_draw_state(&mut self, key: u64, draw_state: GpuDrawState) {
        let byte_size = draw_state.intermediate_byte_size;
        if let Some((_old_key, old_state)) = self.draw_bind_groups.push(key, draw_state) {
            self.draw_state_intermediate_bytes = self
                .draw_state_intermediate_bytes
                .saturating_sub(old_state.intermediate_byte_size);
        }
        self.draw_state_intermediate_bytes =
            self.draw_state_intermediate_bytes.saturating_add(byte_size);
        self.prune_draw_states();
    }

    fn bind_group_for(
        &self,
        device: &wgpu::Device,
        source_view: &wgpu::TextureView,
        params: crate::core::gpu_effect::EffectParams,
    ) -> wgpu::BindGroup {
        let params_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("suisuiview-gpu-effect-params"),
            contents: bytemuck::bytes_of(&params),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("suisuiview-gpu-effect-bind-group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(source_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: params_buffer.as_entire_binding(),
                },
            ],
        })
    }

    fn render_fullscreen(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        output_view: &wgpu::TextureView,
        bind_group: &wgpu::BindGroup,
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
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, bind_group, &[]);
        pass.draw(0..3, 0..1);
    }

    fn prune_source_textures(&mut self) {
        while self.source_texture_bytes > GPU_SOURCE_TEXTURE_BUDGET_BYTES
            && self.source_textures.len() > 1
        {
            let Some((_key, texture)) = self.source_textures.pop_lru() else {
                break;
            };
            self.source_texture_bytes = self.source_texture_bytes.saturating_sub(texture.byte_size);
        }
    }

    fn prune_intermediate_textures(&mut self) {
        while self.intermediate_texture_bytes > GPU_INTERMEDIATE_TEXTURE_BUDGET_BYTES
            && self.intermediate_textures.len() > 1
        {
            let Some((_key, texture)) = self.intermediate_textures.pop_lru() else {
                break;
            };
            self.intermediate_texture_bytes = self
                .intermediate_texture_bytes
                .saturating_sub(texture.byte_size);
        }
    }

    fn prune_draw_states(&mut self) {
        while self.draw_state_intermediate_bytes > GPU_INTERMEDIATE_TEXTURE_BUDGET_BYTES
            && self.draw_bind_groups.len() > 1
        {
            let Some((_key, draw_state)) = self.draw_bind_groups.pop_lru() else {
                break;
            };
            self.draw_state_intermediate_bytes = self
                .draw_state_intermediate_bytes
                .saturating_sub(draw_state.intermediate_byte_size);
        }
    }
}

fn draw_id(
    source_key: GpuPaintSourceKey,
    effects: ViewEffects,
    display_upscaler: DisplayUpscaler,
    rect: Rect,
    opacity: f32,
) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    source_key.hash(&mut hasher);
    effects.hash(&mut hasher);
    display_upscaler.token().hash(&mut hasher);
    rect.min.x.to_bits().hash(&mut hasher);
    rect.min.y.to_bits().hash(&mut hasher);
    rect.max.x.to_bits().hash(&mut hasher);
    rect.max.y.to_bits().hash(&mut hasher);
    opacity.to_bits().hash(&mut hasher);
    hasher.finish()
}

fn viewport_rect(rect: Rect, screen_descriptor: &ScreenDescriptor) -> ([u32; 2], [u32; 2]) {
    let screen_width = screen_descriptor.size_in_pixels[0] as i32;
    let screen_height = screen_descriptor.size_in_pixels[1] as i32;
    let left = (screen_descriptor.pixels_per_point * rect.min.x)
        .round()
        .clamp(0.0, screen_width as f32) as u32;
    let top = (screen_descriptor.pixels_per_point * rect.min.y)
        .round()
        .clamp(0.0, screen_height as f32) as u32;
    let right_raw = (screen_descriptor.pixels_per_point * rect.max.x)
        .round()
        .clamp(0.0, screen_width as f32) as u32;
    let bottom_raw = (screen_descriptor.pixels_per_point * rect.max.y)
        .round()
        .clamp(0.0, screen_height as f32) as u32;
    let right = right_raw
        .max(left.saturating_add(1))
        .min(screen_width as u32);
    let bottom = bottom_raw
        .max(top.saturating_add(1))
        .min(screen_height as u32);
    (
        [left, top],
        [
            right.saturating_sub(left).max(1),
            bottom.saturating_sub(top).max(1),
        ],
    )
}

fn intermediate_texture_key(
    source_key: GpuPaintSourceKey,
    source_size: [usize; 2],
    output_size: [usize; 2],
    effects: ViewEffects,
    display_upscaler: DisplayUpscaler,
    target_size: [u32; 2],
) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    source_key.hash(&mut hasher);
    source_size.hash(&mut hasher);
    output_size.hash(&mut hasher);
    effects.hash(&mut hasher);
    display_upscaler.token().hash(&mut hasher);
    target_size.hash(&mut hasher);
    hasher.finish()
}
