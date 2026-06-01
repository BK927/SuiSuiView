use std::borrow::Cow;
use wgpu::util::DeviceExt;

mod validation;
use validation::workspace_texture_bytes;
pub(crate) use validation::{extent_for_size, validate_render_options};

pub(crate) const OUTPUT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

const FEATURE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const FEATURE_BYTES_PER_PIXEL: u64 = 8;
const ENTRY_POINTS: [&str; 8] = [
    "artcnn_c4f16_conv2d",
    "artcnn_c4f16_conv2d_1_relu",
    "artcnn_c4f16_conv2d_2_relu",
    "artcnn_c4f16_conv2d_3_relu",
    "artcnn_c4f16_conv2d_4_relu",
    "artcnn_c4f16_conv2d_5",
    "artcnn_c4f16_conv2d_6",
    "artcnn_c4f16_depth_to_space",
];

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct ArtcnnParams {
    source_width: u32,
    source_height: u32,
    output_width: u32,
    output_height: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
    _pad3: u32,
}

pub(crate) struct ArtcnnC4F16 {
    bind_group_layout: wgpu::BindGroupLayout,
    pipelines: Vec<wgpu::ComputePipeline>,
}

pub(crate) struct ArtcnnC4F16Output {
    pub(crate) texture: wgpu::Texture,
    pub(crate) size: [usize; 2],
}

pub(crate) struct ArtcnnC4F16Workspace {
    pub(crate) source_size: [usize; 2],
    exact_output_size: [usize; 2],
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
pub(crate) struct ArtcnnC4F16RenderOptions {
    pub(crate) output_size: [usize; 2],
    pub(crate) output_usage: wgpu::TextureUsages,
    pub(crate) transient_limit: u64,
    pub(crate) readback_padded_bytes_per_row: Option<u32>,
}

pub(crate) fn exact_output_size(source_size: [usize; 2]) -> Result<[usize; 2], String> {
    if source_size[0] == 0 || source_size[1] == 0 {
        return Err("ArtCNN C4F16 requires a non-empty source image".to_owned());
    }
    Ok([
        source_size[0]
            .checked_mul(2)
            .ok_or_else(|| "ArtCNN C4F16 output width overflowed".to_owned())?,
        source_size[1]
            .checked_mul(2)
            .ok_or_else(|| "ArtCNN C4F16 output height overflowed".to_owned())?,
    ])
}

impl ArtcnnC4F16 {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("suisuiview-artcnn-c4f16-bind-group-layout"),
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
            label: Some("suisuiview-artcnn-c4f16-pipeline-layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("suisuiview-artcnn-c4f16-shader"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(include_str!("artcnn_c4f16.wgsl"))),
        });
        let pipelines = ENTRY_POINTS
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
        options: ArtcnnC4F16RenderOptions,
    ) -> Result<ArtcnnC4F16Output, String> {
        let workspace = self.create_workspace(device, source_size, &options)?;
        self.render_to_texture_with_workspace(device, encoder, source_view, &workspace, options)
    }

    pub(crate) fn create_workspace(
        &self,
        device: &wgpu::Device,
        source_size: [usize; 2],
        options: &ArtcnnC4F16RenderOptions,
    ) -> Result<ArtcnnC4F16Workspace, String> {
        let exact_output_size = validate_render_options(device, source_size, options)?;

        let source_extent = extent_for_size(source_size);
        let feature_extent = extent_for_size(exact_output_size);
        let skip = create_feature_texture(device, feature_extent, "suisuiview-artcnn-c4f16-skip");
        let tmp_a = create_feature_texture(device, feature_extent, "suisuiview-artcnn-c4f16-tmp-a");
        let tmp_b = create_feature_texture(device, feature_extent, "suisuiview-artcnn-c4f16-tmp-b");
        let conv6 = create_feature_texture(device, source_extent, "suisuiview-artcnn-c4f16-conv6");
        let skip_view = skip.create_view(&wgpu::TextureViewDescriptor::default());
        let tmp_a_view = tmp_a.create_view(&wgpu::TextureViewDescriptor::default());
        let tmp_b_view = tmp_b.create_view(&wgpu::TextureViewDescriptor::default());
        let conv6_view = conv6.create_view(&wgpu::TextureViewDescriptor::default());
        let byte_size = workspace_texture_bytes(source_size, exact_output_size)?;

        Ok(ArtcnnC4F16Workspace {
            source_size,
            exact_output_size,
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
        workspace: &ArtcnnC4F16Workspace,
        options: ArtcnnC4F16RenderOptions,
    ) -> Result<ArtcnnC4F16Output, String> {
        let exact_output_size = validate_render_options(device, workspace.source_size, &options)?;
        if exact_output_size != workspace.exact_output_size {
            return Err("ArtCNN C4F16 workspace output shape mismatch".to_owned());
        }

        let source_size = workspace.source_size;
        let output_extent = extent_for_size(options.output_size);
        let output_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("suisuiview-artcnn-c4f16-output"),
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
            label: Some("suisuiview-artcnn-c4f16-params"),
            contents: bytemuck::bytes_of(&ArtcnnParams {
                source_width: source_size[0] as u32,
                source_height: source_size[1] as u32,
                output_width: options.output_size[0] as u32,
                output_height: options.output_size[1] as u32,
                _pad0: 0,
                _pad1: 0,
                _pad2: 0,
                _pad3: 0,
            }),
            usage: wgpu::BufferUsages::UNIFORM,
        });

        let source_groups = WorkSizes::from_size(source_size)?;
        let output_groups = WorkSizes::from_size(options.output_size)?;
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

        Ok(ArtcnnC4F16Output {
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
        let bind_group = pass_resources
            .device
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(ENTRY_POINTS[pipeline_index]),
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
                label: Some(ENTRY_POINTS[pipeline_index]),
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
    fn from_size(size: [usize; 2]) -> Result<Self, String> {
        Ok(Self {
            x: u32::try_from(size[0])
                .map_err(|_| "ArtCNN C4F16 dispatch width exceeds u32".to_owned())?,
            y: u32::try_from(size[1])
                .map_err(|_| "ArtCNN C4F16 dispatch height exceeds u32".to_owned())?,
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
    use super::ArtcnnC4F16;

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

            let _core = ArtcnnC4F16::new(&device);
        });
    }
}
