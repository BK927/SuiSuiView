use super::{
    GpuDisplayRect, GpuPaintResources, GpuPaintSourceKey, GPU_DRAW_BIND_GROUP_CACHE_LIMIT,
    GPU_DRAW_STATE_BYTES_LIVE, GPU_INTERMEDIATE_TEXTURE_BUDGET_BYTES,
    GPU_INTERMEDIATE_TEXTURE_BYTES_LIVE, GPU_INTERMEDIATE_TEXTURE_CACHE_LIMIT,
    GPU_REALTIME_SR_DEFER_CACHE_LIMIT, GPU_SOURCE_TEXTURE_BUDGET_BYTES,
    GPU_SOURCE_TEXTURE_BYTES_LIVE, GPU_SOURCE_TEXTURE_CACHE_LIMIT,
};
use crate::app::realtime_sr::RealtimeSrResources;
use crate::core::effects::ViewEffects;
#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
use crate::core::perf_trace::{self, PerfField};
use crate::core::state::{WgpuDownscaleMethod, WgpuUpscaleMethod};
use crate::core::worker::PagePixels;
use lru::LruCache;
use std::borrow::Cow;
use std::hash::{Hash, Hasher};
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
use std::time::Duration;
#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
use std::time::Instant;
use wgpu::util::DeviceExt;

pub(super) struct GpuSourceTexture {
    _texture: wgpu::Texture,
    pub(super) view: wgpu::TextureView,
    pub(super) bind_group: Arc<wgpu::BindGroup>,
    byte_size: usize,
}

pub(super) struct GpuDrawState {
    pub(super) texture_bind_group: Arc<wgpu::BindGroup>,
    pub(super) params_bind_group: wgpu::BindGroup,
    _intermediate_pins: Vec<Arc<GpuIntermediateTexture>>,
    pub(super) intermediate_byte_size: usize,
    /// egui pass number that inserted this state; entries from the CURRENT pass
    /// are never pruned (see `prune_draw_states`).
    pub(super) inserted_pass: u64,
}

pub(super) struct GpuIntermediateTexture {
    pub(super) _texture: wgpu::Texture,
    pub(super) _view: wgpu::TextureView,
    pub(super) mip_views: Vec<wgpu::TextureView>,
    pub(super) bind_group: Arc<wgpu::BindGroup>,
    pub(super) size: [usize; 2],
    pub(super) content_key: u64,
    pub(super) byte_size: usize,
    /// `false` until the pass(es) that fill this texture have been recorded into a frame encoder;
    /// set `true` once recorded so later prepares reuse the cached contents instead of re-recording
    /// the (identical, key-determined) passes. A fresh allocation after LRU eviction starts `false`,
    /// so an evicted-and-recreated texture is naturally re-rendered.
    pub(super) rendered: AtomicBool,
    /// egui pass number that last created or reused this texture; entries from
    /// the CURRENT pass are never pruned (see `prune_intermediate_textures`).
    pub(super) last_used_pass: AtomicU64,
}

impl GpuDrawState {
    pub(super) fn new(
        texture_bind_group: Arc<wgpu::BindGroup>,
        params_bind_group: wgpu::BindGroup,
        intermediate_pins: Vec<Arc<GpuIntermediateTexture>>,
    ) -> Self {
        let intermediate_byte_size = intermediate_pins
            .iter()
            .map(|texture| texture.byte_size)
            .sum();
        Self {
            texture_bind_group,
            params_bind_group,
            _intermediate_pins: intermediate_pins,
            intermediate_byte_size,
            inserted_pass: 0,
        }
    }

    pub(super) fn with_intermediate_pin(
        mut self,
        intermediate: Arc<GpuIntermediateTexture>,
    ) -> Self {
        self.intermediate_byte_size = self
            .intermediate_byte_size
            .saturating_add(intermediate.byte_size);
        self._intermediate_pins.push(intermediate);
        self
    }
}

impl GpuPaintResources {
    pub(super) fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Self {
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        let started = Instant::now();
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("suisuiview-gpu-effect-shader"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(include_str!(
                "../../core/gpu_effect.wgsl"
            ))),
        });
        let texture_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("suisuiview-gpu-effect-texture-layout"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });
        let params_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("suisuiview-gpu-effect-params-layout"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("suisuiview-gpu-effect-pipeline-layout"),
            bind_group_layouts: &[&texture_bind_group_layout, &params_bind_group_layout],
            push_constant_ranges: &[],
        });
        let pipeline = super::passes::create_effect_pipeline_timed(
            device,
            &shader,
            &pipeline_layout,
            target_format,
            "suisuiview-gpu-effect-pipeline",
        );
        let intermediate_pipeline = super::passes::create_effect_pipeline_timed(
            device,
            &shader,
            &pipeline_layout,
            INTERMEDIATE_TEXTURE_FORMAT,
            "suisuiview-gpu-effect-intermediate-pipeline",
        );
        let deband_pipeline = super::deband::create_deband_pipeline(device, &pipeline_layout);
        let texture_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("suisuiview-gpu-effect-linear-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let resources = Self {
            target_format,
            texture_bind_group_layout,
            params_bind_group_layout,
            texture_sampler,
            pipeline,
            intermediate_pipeline,
            deband_pipeline,
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
            source_texture_budget_bytes: GPU_SOURCE_TEXTURE_BUDGET_BYTES,
            intermediate_texture_budget_bytes: GPU_INTERMEDIATE_TEXTURE_BUDGET_BYTES,
            current_pass: 0,
            deferred_realtime_sr_first_frames: LruCache::new(
                NonZeroUsize::new(GPU_REALTIME_SR_DEFER_CACHE_LIMIT).unwrap(),
            ),
            realtime_sr: RealtimeSrResources::new(),
            last_refine_log: None,
        };
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        perf_trace::record_duration(
            "gpu_paint_resources_create",
            started.elapsed(),
            &[PerfField::Str(
                "target_format",
                texture_format_label(target_format),
            )],
        );
        // Reset the read-only mirrors so a recreation (e.g. target-format change) does not leave
        // stale byte counts visible to the UI thread.
        resources.publish_gpu_pool_bytes();
        resources
    }

    pub(super) fn ensure_source_texture(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        key: GpuPaintSourceKey,
        image_size: [usize; 2],
        pixels: &PagePixels,
    ) -> bool {
        if self.source_textures.get(&key).is_some() {
            return false;
        }
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        let upload_started = Instant::now();
        let [width, height] = image_size;
        let byte_size = width.saturating_mul(height).saturating_mul(4);
        // VRAM is always RGBA. Expand luma -> RGBA here, after the LRU-miss check, so the cost is
        // paid at most once per source texture (per-frame repaints hit the early return above). For
        // RGBA pages `to_rgba_vec` just clones the retained buffer.
        let rgba = pixels.to_rgba_vec(width, height);
        if rgba.len() != byte_size {
            return false;
        }
        let rgba = rgba.as_slice();
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
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        let write_started = Instant::now();
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
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        record_gpu_texture_upload_stage("gpu_texture_write", write_started, width, height);
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        let bind_group_started = Instant::now();
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = Arc::new(self.texture_bind_group_for(device, &view));
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        record_gpu_texture_upload_stage(
            "gpu_texture_bind_group",
            bind_group_started,
            width,
            height,
        );
        if let Some((_old_key, old_texture)) = self.source_textures.push(
            key,
            GpuSourceTexture {
                _texture: texture,
                view,
                bind_group,
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
        perf_trace::record_duration(
            "gpu_texture_upload",
            upload_started.elapsed(),
            &[
                PerfField::Usize("width", width),
                PerfField::Usize("height", height),
            ],
        );
        true
    }

    pub(super) fn ensure_intermediate_texture(
        &mut self,
        device: &wgpu::Device,
        key: u64,
        target_size: [u32; 2],
    ) {
        if let Some(texture) = self.intermediate_textures.get(&key) {
            texture
                .last_used_pass
                .store(self.current_pass, Ordering::Relaxed);
            return;
        }
        let byte_size = texture_byte_size(target_size, 1);
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
            format: INTERMEDIATE_TEXTURE_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mip_views = vec![texture.create_view(&mip_view_descriptor(0))];
        let bind_group = Arc::new(self.texture_bind_group_for(device, &view));
        if let Some((_old_key, old_texture)) = self.intermediate_textures.push(
            key,
            Arc::new(GpuIntermediateTexture {
                _texture: texture,
                _view: view,
                mip_views,
                bind_group,
                size: [target_size[0] as usize, target_size[1] as usize],
                content_key: key,
                byte_size,
                rendered: AtomicBool::new(false),
                last_used_pass: AtomicU64::new(self.current_pass),
            }),
        ) {
            self.intermediate_texture_bytes = self
                .intermediate_texture_bytes
                .saturating_sub(old_texture.byte_size);
        }
        self.intermediate_texture_bytes = self.intermediate_texture_bytes.saturating_add(byte_size);
        self.prune_intermediate_textures();
    }

    pub(super) fn ensure_mipmapped_intermediate_texture(
        &mut self,
        device: &wgpu::Device,
        key: u64,
        target_size: [u32; 2],
        mip_levels: u32,
    ) {
        if let Some(texture) = self.intermediate_textures.get(&key) {
            texture
                .last_used_pass
                .store(self.current_pass, Ordering::Relaxed);
            return;
        }
        let byte_size = texture_byte_size(target_size, mip_levels);
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("suisuiview-gpu-downscale-mipmap-intermediate"),
            size: wgpu::Extent3d {
                width: target_size[0],
                height: target_size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: mip_levels,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: INTERMEDIATE_TEXTURE_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mip_views = (0..mip_levels)
            .map(|level| texture.create_view(&mip_view_descriptor(level)))
            .collect::<Vec<_>>();
        let bind_group = Arc::new(self.texture_bind_group_for(device, &view));
        if let Some((_old_key, old_texture)) = self.intermediate_textures.push(
            key,
            Arc::new(GpuIntermediateTexture {
                _texture: texture,
                _view: view,
                mip_views,
                bind_group,
                size: [target_size[0] as usize, target_size[1] as usize],
                content_key: key,
                byte_size,
                rendered: AtomicBool::new(false),
                last_used_pass: AtomicU64::new(self.current_pass),
            }),
        ) {
            self.intermediate_texture_bytes = self
                .intermediate_texture_bytes
                .saturating_sub(old_texture.byte_size);
        }
        self.intermediate_texture_bytes = self.intermediate_texture_bytes.saturating_add(byte_size);
        self.prune_intermediate_textures();
    }

    pub(super) fn insert_draw_state(&mut self, key: u64, mut draw_state: GpuDrawState) {
        draw_state.inserted_pass = self.current_pass;
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

    pub(super) fn texture_bind_group_for(
        &self,
        device: &wgpu::Device,
        source_view: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("suisuiview-gpu-effect-texture-bind-group"),
            layout: &self.texture_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(source_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.texture_sampler),
                },
            ],
        })
    }

    pub(super) fn params_bind_group_for(
        &self,
        device: &wgpu::Device,
        params: crate::core::gpu_effect::EffectParams,
    ) -> wgpu::BindGroup {
        let params_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("suisuiview-gpu-effect-params"),
            contents: bytemuck::bytes_of(&params),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("suisuiview-gpu-effect-params-bind-group"),
            layout: &self.params_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: params_buffer.as_entire_binding(),
            }],
        })
    }

    /// Mirror the current byte counters into the read-only `*_LIVE` statics so the app/UI thread
    /// can display live GPU pool usage. Display-only; does not affect budget or eviction logic.
    pub(super) fn publish_gpu_pool_bytes(&self) {
        GPU_SOURCE_TEXTURE_BYTES_LIVE.store(self.source_texture_bytes, Ordering::Relaxed);
        GPU_INTERMEDIATE_TEXTURE_BYTES_LIVE
            .store(self.intermediate_texture_bytes, Ordering::Relaxed);
        GPU_DRAW_STATE_BYTES_LIVE.store(self.draw_state_intermediate_bytes, Ordering::Relaxed);
    }

    pub(super) fn prune_source_textures(&mut self) {
        while self.source_texture_bytes > self.source_texture_budget_bytes
            && self.source_textures.len() > 1
        {
            let Some((_key, texture)) = self.source_textures.pop_lru() else {
                break;
            };
            self.source_texture_bytes = self.source_texture_bytes.saturating_sub(texture.byte_size);
        }
        self.publish_gpu_pool_bytes();
    }

    pub(super) fn drop_original_inspection_sources(&mut self) {
        let keys = self
            .source_textures
            .iter()
            .filter_map(|(key, _texture)| {
                (key.page.target_long_edge > crate::core::worker::MAX_TARGET_LONG_EDGE)
                    .then_some(*key)
            })
            .collect::<Vec<_>>();
        if keys.is_empty() {
            return;
        }

        for key in keys {
            if let Some(texture) = self.source_textures.pop(&key) {
                self.source_texture_bytes =
                    self.source_texture_bytes.saturating_sub(texture.byte_size);
            }
        }

        self.draw_bind_groups.clear();
        self.draw_state_intermediate_bytes = 0;
        self.intermediate_textures.clear();
        self.intermediate_texture_bytes = 0;
        self.publish_gpu_pool_bytes();
    }

    pub(super) fn prune_intermediate_textures(&mut self) {
        while self.intermediate_texture_bytes > self.intermediate_texture_budget_bytes
            && self.intermediate_textures.len() > 1
        {
            // Entries the CURRENT egui pass created or reused must survive it: a
            // multi-page frame (the vertical strip) can legitimately need more
            // than the steady-state budget at once, and evicting mid-frame made
            // every earlier page silently vanish. Stamps and LRU order move
            // together, so once the LRU end is current-pass, everything is.
            if self
                .intermediate_textures
                .peek_lru()
                .is_some_and(|(_key, texture)| {
                    texture.last_used_pass.load(Ordering::Relaxed) == self.current_pass
                })
            {
                break;
            }
            let Some((_key, texture)) = self.intermediate_textures.pop_lru() else {
                break;
            };
            self.intermediate_texture_bytes = self
                .intermediate_texture_bytes
                .saturating_sub(texture.byte_size);
        }
        self.publish_gpu_pool_bytes();
    }

    pub(super) fn prune_draw_states(&mut self) {
        while self.draw_state_intermediate_bytes > self.intermediate_texture_budget_bytes
            && self.draw_bind_groups.len() > 1
        {
            // Same current-pass shield as `prune_intermediate_textures`: the
            // paint callback silently skips on a missing draw state, so evicting
            // one inserted earlier THIS frame blanks that page on screen.
            if self
                .draw_bind_groups
                .peek_lru()
                .is_some_and(|(_key, state)| state.inserted_pass == self.current_pass)
            {
                break;
            }
            let Some((_key, draw_state)) = self.draw_bind_groups.pop_lru() else {
                break;
            };
            self.draw_state_intermediate_bytes = self
                .draw_state_intermediate_bytes
                .saturating_sub(draw_state.intermediate_byte_size);
        }
        self.publish_gpu_pool_bytes();
    }
}

#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
fn record_gpu_texture_upload_stage(
    event: &'static str,
    started: Instant,
    width: usize,
    height: usize,
) {
    perf_trace::record_duration_if_at_least(
        event,
        started.elapsed(),
        Duration::from_millis(1),
        &[
            PerfField::Usize("width", width),
            PerfField::Usize("height", height),
        ],
    );
}

pub(super) fn intermediate_texture_key(
    source_key: GpuPaintSourceKey,
    source_size: [usize; 2],
    output_size: [usize; 2],
    effects: ViewEffects,
    wgpu_upscale_method: WgpuUpscaleMethod,
    display_rect: GpuDisplayRect,
    deband: crate::core::deband::DebandStrength,
) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    // EASU renders from the (possibly debanded) source bind group; key on it.
    deband.token().hash(&mut hasher);
    source_key.hash(&mut hasher);
    source_size.hash(&mut hasher);
    output_size.hash(&mut hasher);
    effects.hash(&mut hasher);
    wgpu_upscale_method.token().hash(&mut hasher);
    display_rect.visible_size.hash(&mut hasher);
    display_rect.sample_offset.hash(&mut hasher);
    display_rect.full_size.hash(&mut hasher);
    hasher.finish()
}

pub(super) fn source_texture_content_key(
    source_key: GpuPaintSourceKey,
    source_size: [usize; 2],
    output_size: [usize; 2],
    effects: ViewEffects,
    deband: crate::core::deband::DebandStrength,
) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    "source_texture_content".hash(&mut hasher);
    source_key.hash(&mut hasher);
    source_size.hash(&mut hasher);
    output_size.hash(&mut hasher);
    effects.hash(&mut hasher);
    // The debanded pre-pass changes the pixels every downstream stage renders
    // from, so the content roots must separate per strength — otherwise a
    // strength change keeps serving stale rendered=true chain intermediates
    // until LRU eviction.
    deband.token().hash(&mut hasher);
    hasher.finish()
}

// established key surface; the linear flag pushed it to 8 args
#[allow(clippy::too_many_arguments)]
pub(super) fn downscale_intermediate_texture_key(
    namespace: &'static str,
    content_key: u64,
    downscaler: WgpuDownscaleMethod,
    effects: ViewEffects,
    linear_downscale: bool,
    stage_size: [u32; 2],
    current_size: [usize; 2],
    stage_index: u32,
) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    namespace.hash(&mut hasher);
    content_key.hash(&mut hasher);
    downscaler.token().hash(&mut hasher);
    // The first pyramid stage bakes `effects` into its output, and every later stage inherits that
    // through its input. `content_key` only carries `effects` on the direct-from-source path; on the
    // post-realtime-SR path it does not, so hash `effects` here to keep the key content-complete
    // (harmless redundancy on the source path — it only splits cache entries).
    effects.hash(&mut hasher);
    // Linear-light changes every downscale stage's pixels; keying on it lets a
    // live settings toggle re-render instead of serving the other mode's
    // rendered=true intermediates until LRU eviction (the V10 stale-key lesson).
    linear_downscale.hash(&mut hasher);
    stage_size.hash(&mut hasher);
    current_size.hash(&mut hasher);
    stage_index.hash(&mut hasher);
    hasher.finish()
}

// NOTE: deliberately NOT keyed on linear-downscale — the hardware-mipmap chain
// blends in hardware trilinear sampling and stays gamma regardless of the flag
// (see the V13 deviation note in gpu_effect.wgsl), so its pixels are flag-free.
pub(super) fn mipmap_intermediate_texture_key(content_key: u64, effects: ViewEffects) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    "hardware_mipmap_linear".hash(&mut hasher);
    content_key.hash(&mut hasher);
    // Mip 0 bakes `effects` into the chain; `content_key` omits `effects` on the post-realtime-SR
    // path, so hash it here to keep the key content-complete.
    effects.hash(&mut hasher);
    hasher.finish()
}

pub(super) fn mip_level_count(size: [usize; 2]) -> u32 {
    let mut width = size[0].max(1);
    let mut height = size[1].max(1);
    let mut levels = 1u32;
    while width > 1 || height > 1 {
        width = (width / 2).max(1);
        height = (height / 2).max(1);
        levels = levels.saturating_add(1);
    }
    levels
}

pub(super) fn mip_size(size: [usize; 2], level: u32) -> [usize; 2] {
    [
        size[0].checked_shr(level).unwrap_or(0).max(1),
        size[1].checked_shr(level).unwrap_or(0).max(1),
    ]
}

#[cfg_attr(
    not(any(feature = "perf-dev", feature = "perf-diagnostics")),
    allow(dead_code)
)]
pub(super) fn texture_format_label(format: wgpu::TextureFormat) -> &'static str {
    match format {
        wgpu::TextureFormat::Rgba8Unorm => "rgba8_unorm",
        wgpu::TextureFormat::Rgba8UnormSrgb => "rgba8_unorm_srgb",
        wgpu::TextureFormat::Bgra8Unorm => "bgra8_unorm",
        wgpu::TextureFormat::Bgra8UnormSrgb => "bgra8_unorm_srgb",
        _ => "other",
    }
}

/// Render-target format for the quality chain's intermediate textures (V12).
/// `Rgba16Float` so every resample / deband / upscale hop keeps sub-8-bit
/// precision instead of re-quantizing to 256 levels between passes; the final
/// composite (`gpu_effect.wgsl`) then dithers the one remaining quantization to
/// the egui target. Source uploads stay `Rgba8Unorm` — 8-bit decoded pixels gain
/// nothing from fp16. `Rgba16Float` is a core WebGPU renderable + filterable
/// format, so no feature gating is needed.
pub(super) const INTERMEDIATE_TEXTURE_FORMAT: wgpu::TextureFormat =
    wgpu::TextureFormat::Rgba16Float;

/// Bytes per pixel of [`INTERMEDIATE_TEXTURE_FORMAT`]. The pool byte accounting
/// (and, through the app-side floor, the eviction budget) must track this so the
/// doubled intermediate footprint flows through instead of under-counting.
const INTERMEDIATE_BYTES_PER_PIXEL: usize = 8;

pub(super) fn texture_byte_size(size: [u32; 2], mip_levels: u32) -> usize {
    (0..mip_levels)
        .map(|level| {
            let mip_size = mip_size([size[0] as usize, size[1] as usize], level);
            mip_size[0]
                .saturating_mul(mip_size[1])
                .saturating_mul(INTERMEDIATE_BYTES_PER_PIXEL)
        })
        .sum()
}

pub(super) fn mip_view_descriptor(base_mip_level: u32) -> wgpu::TextureViewDescriptor<'static> {
    wgpu::TextureViewDescriptor {
        label: Some("suisuiview-gpu-effect-mip-view"),
        dimension: Some(wgpu::TextureViewDimension::D2),
        base_mip_level,
        mip_level_count: Some(1),
        ..Default::default()
    }
}
