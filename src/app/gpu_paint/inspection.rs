//! Full display-result inspection. Pointer movement changes only presentation
//! uniforms; source decode, SR and downscale retain their reference dimensions.

use super::{GpuDisplayRect, GpuDrawState, GpuPaintResources};
use crate::core::effects::ViewEffects;
use crate::core::gpu_effect::params_for_effects_with_display;
use crate::core::state::{WgpuDownscaleMethod, WgpuUpscaleMethod};
use egui::Rect;
use std::hash::{Hash, Hasher};

#[cfg(test)]
mod gpu_tests;

#[derive(Clone, Copy, Debug)]
pub(in crate::app) struct GpuInspection {
    pub reference_size: [u32; 2],
    pub uv: Rect,
    /// Complete processing identity, shared by the main image and its loupe.
    /// Includes source, effects, model, downscale, deband and linear-light mode.
    pub reference_id: u64,
}

impl GpuInspection {
    /// Bound this explicitly optional full-image allocation. Oversized images
    /// keep their regular rendering; inspection never substitutes a smaller SR
    /// reference or describes the full-image work as an ROI optimization.
    pub(in crate::app) fn supports_size(size: [u32; 2]) -> bool {
        size.iter().all(|value| (1..=8192).contains(value))
            && u64::from(size[0]) * u64::from(size[1]) <= 16 * 1024 * 1024
    }

    fn texture_key(self) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        "inspection-display-result-v1".hash(&mut hasher);
        self.reference_id.hash(&mut hasher);
        self.reference_size.hash(&mut hasher);
        hasher.finish()
    }
}

impl GpuPaintResources {
    /// `input` must already be prepared in reference coordinates (origin and
    /// offset zero, visible and full size equal to reference_size). The caller
    /// uses a distinct params slot for it: presentation must never rewrite the
    /// input buffer before the offscreen command consumes it.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn prepare_inspection_draw(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        draw_id: u64,
        input: GpuDrawState,
        inspection: GpuInspection,
        destination: GpuDisplayRect,
    ) -> GpuDrawState {
        let size = inspection.reference_size.map(|value| value as usize);
        let key = inspection.texture_key();
        self.ensure_intermediate_texture(device, key, inspection.reference_size);
        let reference = self
            .intermediate_textures
            .peek(&key)
            .expect("inspection reference was allocated")
            .clone();
        // Do not permanently cache a fallback frame while an asynchronous model
        // is pending. Finalization calls this again with the refined draw state.
        self.render_inspection_reference(device, encoder, &reference._view, &input);
        let params = params_for_effects_with_display(
            size,
            size,
            ViewEffects::default(),
            WgpuUpscaleMethod::None,
            WgpuDownscaleMethod::Bilinear,
            destination.origin,
            destination.visible_size,
            destination.sample_offset,
            destination.full_size,
            1.0,
        )
        .with_inspection_uv(
            inspection.uv,
            destination.sample_offset,
            destination.full_size,
        );
        let (params_buffer, params_bind_group) =
            self.recycle_params_pair(device, queue, draw_id, params);
        GpuDrawState::new(
            reference.bind_group.clone(),
            params_buffer,
            params_bind_group,
            vec![reference],
        )
    }

    fn render_inspection_reference(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        input: &GpuDrawState,
    ) {
        let pipeline = self.inspection_pipeline.get_or_insert_with(|| {
            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("suisuiview-inspection-shader"),
                source: wgpu::ShaderSource::Wgsl(include_str!("../../core/gpu_effect.wgsl").into()),
            });
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("suisuiview-inspection-layout"),
                bind_group_layouts: &[
                    &self.texture_bind_group_layout,
                    &self.params_bind_group_layout,
                ],
                push_constant_ranges: &[],
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("suisuiview-inspection-copy-pipeline"),
                layout: Some(&layout),
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
                        format: super::pools::INTERMEDIATE_TEXTURE_FORMAT,
                        // A finished shader result is stored without compositing.
                        // The ordinary final draw applies its alpha exactly once.
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                multiview: None,
                cache: None,
            })
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("suisuiview-inspection-reference-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
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
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, input.texture_bind_group.as_ref(), &[]);
        pass.set_bind_group(1, input.params_bind_group.as_ref(), &[]);
        pass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inspection_pointer_does_not_change_processing_key() {
        let base = GpuInspection {
            reference_size: [1920, 1080],
            uv: Rect::EVERYTHING,
            reference_id: 23,
        };
        let moved = GpuInspection {
            uv: Rect::from_min_max(egui::pos2(0.2, 0.3), egui::pos2(0.4, 0.5)),
            ..base
        };
        assert_eq!(base.texture_key(), moved.texture_key());
        assert_ne!(
            base.texture_key(),
            GpuInspection {
                reference_size: [3840, 2160],
                ..base
            }
            .texture_key()
        );
        assert_ne!(
            base.texture_key(),
            GpuInspection {
                reference_id: 24,
                ..base
            }
            .texture_key()
        );
    }

    #[test]
    fn inspection_bounds_full_reference_allocations() {
        assert!(GpuInspection::supports_size([3840, 2160]));
        assert!(!GpuInspection::supports_size([8192, 8192]));
        assert!(!GpuInspection::supports_size([1, 0]));
        assert!(!GpuInspection::supports_size([16384, 1]));
    }
}
