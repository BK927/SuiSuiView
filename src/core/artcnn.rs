use std::borrow::Cow;
use wgpu::util::DeviceExt;

mod validation;
mod variants;

use validation::workspace_texture_bytes;
pub(crate) use validation::{extent_for_size, validate_render_options};
pub use variants::ArtcnnVariant;

pub(crate) const OUTPUT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

const FEATURE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const FEATURE_BYTES_PER_PIXEL: u64 = 8;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct ArtcnnParams {
    source_width: u32,
    source_height: u32,
    output_width: u32,
    output_height: u32,
    feature_width: u32,
    feature_height: u32,
    _pad2: u32,
    _pad3: u32,
}

pub(crate) struct Artcnn {
    variant: ArtcnnVariant,
    bind_group_layout: wgpu::BindGroupLayout,
    pipelines: Vec<wgpu::ComputePipeline>,
}

pub(crate) struct ArtcnnOutput {
    pub(crate) texture: wgpu::Texture,
    pub(crate) size: [usize; 2],
}

pub(crate) struct ArtcnnWorkspace {
    pub(crate) variant: ArtcnnVariant,
    pub(crate) source_size: [usize; 2],
    exact_output_size: [usize; 2],
    feature_size: [usize; 2],
    _skip: wgpu::Texture,
    _tmp_a: wgpu::Texture,
    _tmp_b: wgpu::Texture,
    _conv6: wgpu::Texture,
    skip_view: wgpu::TextureView,
    tmp_a_view: wgpu::TextureView,
    tmp_b_view: wgpu::TextureView,
    conv6_view: wgpu::TextureView,
    #[allow(dead_code)] // Used by the GUI binary; the library target does not compile app modules.
    pub(crate) byte_size: u64,
}

#[derive(Clone, Copy)]
pub(crate) struct ArtcnnRenderOptions {
    pub(crate) output_size: [usize; 2],
    pub(crate) output_usage: wgpu::TextureUsages,
    pub(crate) transient_limit: u64,
    pub(crate) readback_padded_bytes_per_row: Option<u32>,
}

pub(crate) fn exact_output_size(
    variant: ArtcnnVariant,
    source_size: [usize; 2],
) -> Result<[usize; 2], String> {
    if source_size[0] == 0 || source_size[1] == 0 {
        return Err(format!(
            "{} requires a non-empty source image",
            variant.label()
        ));
    }
    Ok([
        source_size[0]
            .checked_mul(2)
            .ok_or_else(|| format!("{} output width overflowed", variant.label()))?,
        source_size[1]
            .checked_mul(2)
            .ok_or_else(|| format!("{} output height overflowed", variant.label()))?,
    ])
}

impl Artcnn {
    pub(crate) fn new(device: &wgpu::Device, variant: ArtcnnVariant) -> Self {
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("suisuiview-artcnn-bind-group-layout"),
            entries: &[
                texture_entry(0),
                texture_entry(1),
                texture_entry(2),
                storage_entry(3, FEATURE_FORMAT),
                storage_entry(4, OUTPUT_FORMAT),
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::COMPUTE,
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
            label: Some("suisuiview-artcnn-pipeline-layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(variant.token()),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(variant.shader_source())),
        });
        let pipelines = variant
            .entry_points()
            .iter()
            .map(|entry_point| {
                device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some(entry_point),
                    layout: Some(&pipeline_layout),
                    module: &shader,
                    entry_point: Some(entry_point),
                    compilation_options: Default::default(),
                    cache: None,
                })
            })
            .collect();
        Self {
            variant,
            bind_group_layout,
            pipelines,
        }
    }

    pub(crate) fn render_to_texture(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        source_view: &wgpu::TextureView,
        source_size: [usize; 2],
        options: ArtcnnRenderOptions,
    ) -> Result<ArtcnnOutput, String> {
        let workspace = self.create_workspace(device, source_size, &options)?;
        self.render_to_texture_with_workspace(device, encoder, source_view, &workspace, options)
    }

    pub(crate) fn create_workspace(
        &self,
        device: &wgpu::Device,
        source_size: [usize; 2],
        options: &ArtcnnRenderOptions,
    ) -> Result<ArtcnnWorkspace, String> {
        let exact_output_size =
            validate_render_options(device, self.variant, source_size, options)?;
        let feature_size = self.variant.feature_size(source_size)?;

        let source_extent = extent_for_size(source_size);
        let feature_extent = extent_for_size(feature_size);
        let skip = create_feature_texture(device, feature_extent, "suisuiview-artcnn-skip");
        let tmp_a = create_feature_texture(device, feature_extent, "suisuiview-artcnn-tmp-a");
        let tmp_b = create_feature_texture(device, feature_extent, "suisuiview-artcnn-tmp-b");
        let conv6 = create_feature_texture(device, source_extent, "suisuiview-artcnn-conv6");
        let skip_view = skip.create_view(&wgpu::TextureViewDescriptor::default());
        let tmp_a_view = tmp_a.create_view(&wgpu::TextureViewDescriptor::default());
        let tmp_b_view = tmp_b.create_view(&wgpu::TextureViewDescriptor::default());
        let conv6_view = conv6.create_view(&wgpu::TextureViewDescriptor::default());
        let byte_size = workspace_texture_bytes(self.variant, source_size, feature_size)?;

        Ok(ArtcnnWorkspace {
            variant: self.variant,
            source_size,
            exact_output_size,
            feature_size,
            _skip: skip,
            _tmp_a: tmp_a,
            _tmp_b: tmp_b,
            _conv6: conv6,
            skip_view,
            tmp_a_view,
            tmp_b_view,
            conv6_view,
            byte_size,
        })
    }

    pub(crate) fn render_to_texture_with_workspace(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        source_view: &wgpu::TextureView,
        workspace: &ArtcnnWorkspace,
        options: ArtcnnRenderOptions,
    ) -> Result<ArtcnnOutput, String> {
        if workspace.variant != self.variant {
            return Err(format!(
                "{} workspace variant mismatch",
                self.variant.label()
            ));
        }
        let exact_output_size =
            validate_render_options(device, self.variant, workspace.source_size, &options)?;
        if exact_output_size != workspace.exact_output_size {
            return Err(format!(
                "{} workspace output shape mismatch",
                self.variant.label()
            ));
        }
        if self.variant.feature_size(workspace.source_size)? != workspace.feature_size {
            return Err(format!(
                "{} workspace feature shape mismatch",
                self.variant.label()
            ));
        }

        let source_size = workspace.source_size;
        let output_extent = extent_for_size(options.output_size);
        let output_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("suisuiview-artcnn-output"),
            size: output_extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: OUTPUT_FORMAT,
            usage: wgpu::TextureUsages::STORAGE_BINDING | options.output_usage,
            view_formats: &[],
        });
        let output_view = output_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let params_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("suisuiview-artcnn-params"),
            contents: bytemuck::bytes_of(&ArtcnnParams {
                source_width: source_size[0] as u32,
                source_height: source_size[1] as u32,
                output_width: options.output_size[0] as u32,
                output_height: options.output_size[1] as u32,
                feature_width: workspace.feature_size[0] as u32,
                feature_height: workspace.feature_size[1] as u32,
                _pad2: 0,
                _pad3: 0,
            }),
            usage: wgpu::BufferUsages::UNIFORM,
        });

        let source_groups = WorkSizes::from_size(self.variant, source_size)?;
        let output_groups = WorkSizes::from_size(self.variant, options.output_size)?;
        let mut pass_resources = PassResources {
            device,
            encoder,
            params_buffer: &params_buffer,
        };

        self.run_pass(
            &mut pass_resources,
            0,
            Views {
                source: source_view,
                input0: source_view,
                input1: source_view,
                out: &workspace.skip_view,
                final_out: &output_view,
            },
            source_groups,
        );
        self.run_pass(
            &mut pass_resources,
            1,
            Views {
                source: source_view,
                input0: &workspace.skip_view,
                input1: source_view,
                out: &workspace.tmp_a_view,
                final_out: &output_view,
            },
            source_groups,
        );
        self.run_pass(
            &mut pass_resources,
            2,
            Views {
                source: source_view,
                input0: &workspace.tmp_a_view,
                input1: source_view,
                out: &workspace.tmp_b_view,
                final_out: &output_view,
            },
            source_groups,
        );
        self.run_pass(
            &mut pass_resources,
            3,
            Views {
                source: source_view,
                input0: &workspace.tmp_b_view,
                input1: source_view,
                out: &workspace.tmp_a_view,
                final_out: &output_view,
            },
            source_groups,
        );
        self.run_pass(
            &mut pass_resources,
            4,
            Views {
                source: source_view,
                input0: &workspace.tmp_a_view,
                input1: source_view,
                out: &workspace.tmp_b_view,
                final_out: &output_view,
            },
            source_groups,
        );
        self.run_pass(
            &mut pass_resources,
            5,
            Views {
                source: source_view,
                input0: &workspace.tmp_b_view,
                input1: source_view,
                out: &workspace.tmp_a_view,
                final_out: &output_view,
            },
            source_groups,
        );
        self.run_pass(
            &mut pass_resources,
            6,
            Views {
                source: source_view,
                input0: &workspace.skip_view,
                input1: &workspace.tmp_a_view,
                out: &workspace.conv6_view,
                final_out: &output_view,
            },
            source_groups,
        );
        self.run_pass(
            &mut pass_resources,
            7,
            Views {
                source: source_view,
                input0: &workspace.conv6_view,
                input1: source_view,
                out: &workspace.tmp_b_view,
                final_out: &output_view,
            },
            output_groups,
        );

        Ok(ArtcnnOutput {
            texture: output_texture,
            size: options.output_size,
        })
    }

    fn run_pass(
        &self,
        pass_resources: &mut PassResources<'_>,
        pipeline_index: usize,
        views: Views<'_>,
        groups: WorkSizes,
    ) {
        let entry_point = self.variant.entry_points()[pipeline_index];
        let bind_group = pass_resources
            .device
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(entry_point),
                layout: &self.bind_group_layout,
                entries: &[
                    texture_binding(0, views.source),
                    texture_binding(1, views.input0),
                    texture_binding(2, views.input1),
                    storage_binding(3, views.out),
                    storage_binding(4, views.final_out),
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: pass_resources.params_buffer.as_entire_binding(),
                    },
                ],
            });
        let mut pass = pass_resources
            .encoder
            .begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some(entry_point),
                timestamp_writes: None,
            });
        pass.set_pipeline(&self.pipelines[pipeline_index]);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.dispatch_workgroups(groups.x.div_ceil(8), groups.y.div_ceil(8), 1);
    }
}

struct PassResources<'a> {
    device: &'a wgpu::Device,
    encoder: &'a mut wgpu::CommandEncoder,
    params_buffer: &'a wgpu::Buffer,
}

struct Views<'a> {
    source: &'a wgpu::TextureView,
    input0: &'a wgpu::TextureView,
    input1: &'a wgpu::TextureView,
    out: &'a wgpu::TextureView,
    final_out: &'a wgpu::TextureView,
}

#[derive(Clone, Copy)]
struct WorkSizes {
    x: u32,
    y: u32,
}

impl WorkSizes {
    fn from_size(variant: ArtcnnVariant, size: [usize; 2]) -> Result<Self, String> {
        Ok(Self {
            x: u32::try_from(size[0])
                .map_err(|_| format!("{} dispatch width exceeds u32", variant.label()))?,
            y: u32::try_from(size[1])
                .map_err(|_| format!("{} dispatch height exceeds u32", variant.label()))?,
        })
    }
}

fn create_feature_texture(
    device: &wgpu::Device,
    size: wgpu::Extent3d,
    label: &'static str,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FEATURE_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING,
        view_formats: &[],
    })
}

fn texture_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: false },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn storage_entry(binding: u32, format: wgpu::TextureFormat) -> wgpu::BindGroupLayoutEntry {
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

fn texture_binding(binding: u32, view: &wgpu::TextureView) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: wgpu::BindingResource::TextureView(view),
    }
}

fn storage_binding(binding: u32, view: &wgpu::TextureView) -> wgpu::BindGroupEntry<'_> {
    texture_binding(binding, view)
}

#[cfg(test)]
mod tests {
    use super::{Artcnn, ArtcnnVariant};

    #[test]
    fn shader_pipelines_compile_when_wgpu_is_available() {
        pollster::block_on(async {
            let instance = wgpu::Instance::default();
            let Ok(adapter) = instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::LowPower,
                    force_fallback_adapter: false,
                    compatible_surface: None,
                })
                .await
            else {
                return;
            };
            let Ok((device, _queue)) = adapter
                .request_device(&wgpu::DeviceDescriptor {
                    label: Some("suisuiview-artcnn-c4f16-test-device"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default().using_resolution(adapter.limits()),
                    memory_hints: wgpu::MemoryHints::Performance,
                    trace: wgpu::Trace::Off,
                })
                .await
            else {
                return;
            };

            for variant in ArtcnnVariant::ALL {
                let _core = Artcnn::new(&device, variant);
            }
        });
    }
}
