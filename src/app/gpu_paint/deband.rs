//! WGPU debanding pre-pass: source texture -> debanded intermediate at SOURCE
//! size. The rest of the paint chain (SR upscale / downscale / effects) samples
//! the debanded intermediate instead of the source. The intermediate enrolls in
//! the shared intermediate-texture pool (budget + current-pass pin invariant),
//! keyed by source identity + strength, so a static page runs the pass once and
//! reuses the cached result until the strength changes.
//!
//! The GPU algorithm lives in `../../core/deband.wgsl`; the CPU reference and the
//! algorithm contract live in `crate::core::deband`.

use super::pools::GpuIntermediateTexture;
use super::{GpuPaintResources, GpuPaintSourceKey};
use crate::core::deband::{DebandParams, DebandStrength};
use std::borrow::Cow;
use std::hash::{Hash, Hasher};
use std::sync::atomic::Ordering;
use std::sync::{Arc, OnceLock};
use wgpu::util::DeviceExt;

/// Uniform layout mirrored by `struct DebandParams` in `deband.wgsl`. `threshold`
/// and `grain` are normalized (preset 8-bit value / 255) for the 0..1 shader.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct DebandGpuParams {
    dims: [u32; 4],
    config: [f32; 4],
}

impl DebandGpuParams {
    fn new(source_size: [usize; 2], params: DebandParams) -> Self {
        Self {
            dims: [
                source_size[0] as u32,
                source_size[1] as u32,
                params.iterations,
                0,
            ],
            config: [
                params.base_radius,
                params.threshold / 255.0,
                params.grain / 255.0,
                0.0,
            ],
        }
    }
}

/// `SUISUIVIEW_DEBAND_LOG=1` traces the pipeline creation once and each pass
/// execution. Read once via `OnceLock` so the hot path pays nothing when unset.
pub(super) fn log_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("SUISUIVIEW_DEBAND_LOG").is_some())
}

/// Build the deband render pipeline. Reuses the shared texture + params bind
/// group layouts (via `pipeline_layout`) so the existing source bind group and
/// the params buffer feed it unchanged; targets the Rgba8Unorm intermediate.
pub(super) fn create_deband_pipeline(
    device: &wgpu::Device,
    pipeline_layout: &wgpu::PipelineLayout,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("suisuiview-deband-shader"),
        source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(include_str!("../../core/deband.wgsl"))),
    });
    if log_enabled() {
        eprintln!("[deband] pipeline created");
    }
    super::passes::create_effect_pipeline_timed(
        device,
        &shader,
        pipeline_layout,
        wgpu::TextureFormat::Rgba8Unorm,
        "suisuiview-deband-pipeline",
    )
}

impl GpuPaintResources {
    /// Ensure a debanded copy of the source exists for `strength`, recording the
    /// pass into `encoder` on first use. Returns the bind group the rest of the
    /// chain should sample and the intermediate to pin for lifetime (its texture
    /// backs the returned bind group). `Off` returns the source unchanged.
    pub(super) fn ensure_debanded_source(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        source_key: GpuPaintSourceKey,
        source_size: [usize; 2],
        source_bind_group: Arc<wgpu::BindGroup>,
        strength: DebandStrength,
    ) -> (Arc<wgpu::BindGroup>, Option<Arc<GpuIntermediateTexture>>) {
        let Some(params) = strength.params() else {
            return (source_bind_group, None);
        };
        let key = deband_source_texture_key(source_key, source_size, strength);
        let target = [source_size[0].max(1) as u32, source_size[1].max(1) as u32];
        // Enrolls in the intermediate pool and stamps last_used_pass = current
        // pass, so the current-pass prune shield keeps it alive this frame.
        self.ensure_intermediate_texture(device, key, target);
        let intermediate = self
            .intermediate_textures
            .peek(&key)
            .expect("deband intermediate should be cached before rendering")
            .clone();
        if !intermediate.rendered.load(Ordering::Relaxed) {
            let gpu_params = DebandGpuParams::new(source_size, params);
            let params_bind_group = self.deband_params_bind_group(device, gpu_params);
            let view = intermediate
                .mip_views
                .first()
                .expect("deband intermediate should expose a renderable mip 0 view");
            self.render_deband(encoder, view, &source_bind_group, &params_bind_group);
            intermediate.rendered.store(true, Ordering::Relaxed);
            if log_enabled() {
                eprintln!(
                    "[deband] pass page_id={} target_long_edge={} strength={} size={}x{}",
                    source_key.page.page_id.0,
                    source_key.page.target_long_edge,
                    strength.token(),
                    source_size[0],
                    source_size[1]
                );
            }
        }
        (intermediate.bind_group.clone(), Some(intermediate))
    }

    fn deband_params_bind_group(
        &self,
        device: &wgpu::Device,
        params: DebandGpuParams,
    ) -> wgpu::BindGroup {
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("suisuiview-deband-params"),
            contents: bytemuck::bytes_of(&params),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("suisuiview-deband-params-bind-group"),
            layout: &self.params_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        })
    }

    fn render_deband(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        output_view: &wgpu::TextureView,
        texture_bind_group: &wgpu::BindGroup,
        params_bind_group: &wgpu::BindGroup,
    ) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("suisuiview-deband-pass"),
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
        pass.set_pipeline(&self.deband_pipeline);
        pass.set_bind_group(0, texture_bind_group, &[]);
        pass.set_bind_group(1, params_bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

/// Cache key for a debanded source: source identity + size + strength. Content
/// complete, so a static page reuses the cached texture across frames and a
/// strength change picks a fresh key (the old one ages out of the LRU).
pub(super) fn deband_source_texture_key(
    source_key: GpuPaintSourceKey,
    source_size: [usize; 2],
    strength: DebandStrength,
) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    "deband_source".hash(&mut hasher);
    source_key.hash(&mut hasher);
    source_size.hash(&mut hasher);
    strength.token().hash(&mut hasher);
    hasher.finish()
}
