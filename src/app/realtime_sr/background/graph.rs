//! A layer cursor keeps each GPU submission bounded. Every row of a layer is
//! finished before the next layer reads it; no partial result is published.
use super::super::{RealtimeSrOutput, TEXTURE_FORMAT};
use std::sync::Arc;
use wgpu::util::DeviceExt;

pub(super) const BATCH_PIXELS: u32 = 16_384;

pub(super) enum Layer {
    Render {
        pipeline: wgpu::RenderPipeline,
        bindings: wgpu::BindGroup,
        target: wgpu::TextureView,
        size: [u32; 2],
    },
    Compute {
        pipeline: wgpu::ComputePipeline,
        bindings: wgpu::BindGroup,
        params: wgpu::Buffer,
        dimensions: [u32; 4],
        size: [u32; 2],
        workgroup: u32,
    },
}

pub(super) struct TextureGraph {
    pub(super) layers: Vec<Layer>,
    pub(super) output: Arc<RealtimeSrOutput>,
    // Explicit owners make the full feature allocation/lifetime auditable.
    pub(super) _textures: Vec<wgpu::Texture>,
    pub(super) bytes: usize,
    layer: usize,
    row: u32,
}

impl TextureGraph {
    pub(super) fn new(
        layers: Vec<Layer>,
        output: Arc<RealtimeSrOutput>,
        textures: Vec<wgpu::Texture>,
        bytes: usize,
    ) -> Self {
        Self {
            layers,
            output,
            _textures: textures,
            bytes,
            layer: 0,
            row: 0,
        }
    }

    /// Returns true only after encoding the very last batch. Its output still
    /// must wait for submission completion before being handed to the UI.
    pub(super) fn encode_next(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
    ) -> bool {
        let Some(layer) = self.layers.get(self.layer) else {
            return true;
        };
        let (height, rows) = match layer {
            Layer::Render {
                pipeline,
                bindings,
                target,
                size,
            } => {
                let rows = (BATCH_PIXELS / size[0].max(1))
                    .max(1)
                    .min(size[1] - self.row);
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("suisuiview-refine-row"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: target,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: if self.row == 0 {
                                wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT)
                            } else {
                                wgpu::LoadOp::Load
                            },
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, bindings, &[]);
                pass.set_scissor_rect(0, self.row, size[0], rows);
                pass.draw(0..3, 0..1);
                (size[1], rows)
            }
            Layer::Compute {
                pipeline,
                bindings,
                params,
                dimensions,
                size,
                workgroup,
            } => {
                // Start rows are always workgroup aligned. Only the final
                // dispatch overhangs; the original shader's size guard clips it.
                let rows = ((BATCH_PIXELS / size[0].max(1) / workgroup).max(1) * workgroup)
                    .min(size[1] - self.row);
                let uniform = [
                    dimensions[0],
                    dimensions[1],
                    dimensions[2],
                    dimensions[3],
                    self.row,
                    0,
                    0,
                    0,
                ];
                queue.write_buffer(params, 0, bytemuck::cast_slice(&uniform));
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("suisuiview-refine-dispatch"),
                    timestamp_writes: None,
                });
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, bindings, &[]);
                pass.dispatch_workgroups(
                    size[0].div_ceil(*workgroup),
                    rows.div_ceil(*workgroup),
                    1,
                );
                (size[1], rows)
            }
        };
        self.row += rows;
        if self.row == height {
            self.layer += 1;
            self.row = 0;
        }
        self.layer == self.layers.len()
    }
}

pub(super) fn texture(
    device: &wgpu::Device,
    size: [usize; 2],
    format: wgpu::TextureFormat,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("suisuiview-refine-texture"),
        size: wgpu::Extent3d {
            width: size[0] as u32,
            height: size[1] as u32,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::STORAGE_BINDING
            | wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

pub(super) fn output(device: &wgpu::Device, source_size: [usize; 2]) -> Arc<RealtimeSrOutput> {
    let size = [source_size[0] * 2, source_size[1] * 2];
    let texture = texture(device, size, TEXTURE_FORMAT);
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    Arc::new(RealtimeSrOutput {
        texture,
        view,
        size,
        byte_size: size[0] * size[1] * 4,
    })
}

pub(super) fn params(device: &wgpu::Device, values: &[u32]) -> wgpu::Buffer {
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("suisuiview-refine-params"),
        contents: bytemuck::cast_slice(values),
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    })
}

pub(super) fn sampled(binding: u32, compute: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: if compute {
            wgpu::ShaderStages::COMPUTE
        } else {
            wgpu::ShaderStages::FRAGMENT
        },
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: false },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}
pub(super) fn storage(binding: u32, format: wgpu::TextureFormat) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::StorageTexture {
            access: wgpu::StorageTextureAccess::WriteOnly,
            format,
            view_dimension: wgpu::TextureViewDimension::D2,
        },
        count: None,
    }
}
pub(super) fn uniform(binding: u32, compute: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: if compute {
            wgpu::ShaderStages::COMPUTE
        } else {
            wgpu::ShaderStages::FRAGMENT
        },
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}
pub(super) fn binding(binding: u32, view: &wgpu::TextureView) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: wgpu::BindingResource::TextureView(view),
    }
}
