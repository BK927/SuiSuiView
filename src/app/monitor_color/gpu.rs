use crate::core::monitor_color::{DisplayLut, LUT_EDGE};
use egui_wgpu::{CallbackResources, CallbackTrait};
use std::sync::Arc;
use wgpu;

pub(super) struct OutputLut {
    pub pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    lut: wgpu::TextureView,
}

impl OutputLut {
    // Called by the profile worker, including pipeline compilation and upload.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        lut: &DisplayLut,
    ) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("display-color-layout"),
            entries: &[
                texture_entry(0, wgpu::TextureViewDimension::D2, false),
                texture_entry(1, wgpu::TextureViewDimension::D3, true),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let shader_source = include_str!("output.wgsl").replace(
            "const FRAMEBUFFER_SRGB: bool = false;",
            if format.is_srgb() {
                "const FRAMEBUFFER_SRGB: bool = true;"
            } else {
                "const FRAMEBUFFER_SRGB: bool = false;"
            },
        );
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("display-color-output"),
            source: wgpu::ShaderSource::Wgsl(shader_source.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("display-color-pipeline-layout"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("display-color-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("display-color-lut"),
            size: wgpu::Extent3d {
                width: LUT_EDGE,
                height: LUT_EDGE,
                depth_or_array_layers: LUT_EDGE,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            format: wgpu::TextureFormat::Rgba16Float,
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
            bytemuck::cast_slice(&lut.rgba16),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(LUT_EDGE * 8),
                rows_per_image: Some(LUT_EDGE),
            },
            texture.size(),
        );
        Self {
            pipeline,
            layout,
            lut: texture.create_view(&Default::default()),
            sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("display-color-lut-sampler"),
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }),
        }
    }

    pub fn binding(&self, device: &wgpu::Device, scene: &wgpu::TextureView) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("display-color-scene-binding"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(scene),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&self.lut),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        })
    }
}

fn texture_entry(
    binding: u32,
    dimension: wgpu::TextureViewDimension,
    filterable: bool,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable },
            view_dimension: dimension,
            multisampled: false,
        },
        count: None,
    }
}

pub(super) struct SceneTexture {
    pub view: wgpu::TextureView,
    pub binding: Arc<wgpu::BindGroup>,
    pub size: [u32; 2],
    pub fingerprint: u64,
}

impl SceneTexture {
    pub fn new(device: &wgpu::Device, size: [u32; 2], fingerprint: u64, lut: &OutputLut) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("display-color-composited-scene"),
            size: wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: super::scene::FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let binding = Arc::new(lut.binding(device, &view));
        Self {
            view,
            binding,
            size,
            fingerprint,
        }
    }
}

pub(super) struct OutputCallback {
    pub lut: Arc<OutputLut>,
    pub binding: Arc<wgpu::BindGroup>,
}

impl CallbackTrait for OutputCallback {
    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        _resources: &CallbackResources,
    ) {
        pass.set_pipeline(&self.lut.pipeline);
        pass.set_bind_group(0, self.binding.as_ref(), &[]);
        pass.draw(0..3, 0..1);
    }
}
