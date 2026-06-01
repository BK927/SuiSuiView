use super::TEXTURE_FORMAT;
use crate::core::sr_lab::gpu::tiled::SpanTileSpec;
use std::borrow::Cow;
use wgpu::util::DeviceExt;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(super) struct SpanBridgeParams {
    pub(super) source_width: u32,
    pub(super) source_height: u32,
    pub(super) input_width: u32,
    pub(super) input_height: u32,
    pub(super) output_width: u32,
    pub(super) output_height: u32,
    pub(super) dest_width: u32,
    pub(super) dest_height: u32,
    pub(super) source_x: u32,
    pub(super) source_y: u32,
    pub(super) read_x: u32,
    pub(super) read_y: u32,
    pub(super) dest_x: u32,
    pub(super) dest_y: u32,
    pub(super) copy_width: u32,
    pub(super) copy_height: u32,
}

pub(super) struct SpanBridge {
    input_bind_group_layout: wgpu::BindGroupLayout,
    output_bind_group_layout: wgpu::BindGroupLayout,
    input_pipeline: wgpu::ComputePipeline,
    output_pipeline: wgpu::ComputePipeline,
}

pub(super) struct SpanBridgeTile {
    params_buffer: wgpu::Buffer,
    input_bind_group: wgpu::BindGroup,
}

pub(super) struct SpanBridgeOutput {
    bind_group: wgpu::BindGroup,
}

impl SpanBridge {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        let input_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("suisuiview-realtime-span-input-bind-group-layout"),
                entries: &[texture_entry(0), storage_entry(1), uniform_entry(2)],
            });
        let output_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("suisuiview-realtime-span-output-bind-group-layout"),
                entries: &[
                    storage_read_entry(0),
                    storage_texture_entry(1),
                    uniform_entry(2),
                ],
            });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("suisuiview-realtime-span-bridge-shader"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(include_str!("span.wgsl"))),
        });
        let input_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("suisuiview-realtime-span-input-pipeline-layout"),
                bind_group_layouts: &[&input_bind_group_layout],
                push_constant_ranges: &[],
            });
        let output_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("suisuiview-realtime-span-output-pipeline-layout"),
                bind_group_layouts: &[&input_bind_group_layout, &output_bind_group_layout],
                push_constant_ranges: &[],
            });
        let input_pipeline =
            create_pipeline(device, &input_pipeline_layout, &shader, "span_rgba_to_chw");
        let output_pipeline =
            create_pipeline(device, &output_pipeline_layout, &shader, "span_chw_to_rgba");

        Self {
            input_bind_group_layout,
            output_bind_group_layout,
            input_pipeline,
            output_pipeline,
        }
    }

    pub(super) fn bind_tile(
        &self,
        device: &wgpu::Device,
        source_view: &wgpu::TextureView,
        input_buffer: &wgpu::Buffer,
        params: SpanBridgeParams,
    ) -> SpanBridgeTile {
        let params_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("suisuiview-realtime-span-bridge-params"),
            contents: bytemuck::bytes_of(&params),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let input_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("suisuiview-realtime-span-input-bind-group"),
            layout: &self.input_bind_group_layout,
            entries: &[
                texture_binding(0, source_view),
                buffer_binding(1, input_buffer),
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: params_buffer.as_entire_binding(),
                },
            ],
        });
        SpanBridgeTile {
            params_buffer,
            input_bind_group,
        }
    }

    pub(super) fn bind_output(
        &self,
        device: &wgpu::Device,
        tile: &SpanBridgeTile,
        output_buffer: &wgpu::Buffer,
        output_view: &wgpu::TextureView,
    ) -> SpanBridgeOutput {
        let output_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("suisuiview-realtime-span-output-bind-group"),
            layout: &self.output_bind_group_layout,
            entries: &[
                buffer_binding(0, output_buffer),
                texture_binding(1, output_view),
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: tile.params_buffer.as_entire_binding(),
                },
            ],
        });
        SpanBridgeOutput {
            bind_group: output_bind_group,
        }
    }

    pub(super) fn dispatch_input(
        &self,
        pass: &mut wgpu::ComputePass<'_>,
        tile: &SpanBridgeTile,
        params: SpanBridgeParams,
    ) {
        pass.set_pipeline(&self.input_pipeline);
        pass.set_bind_group(0, &tile.input_bind_group, &[]);
        pass.dispatch_workgroups(
            params.input_width.div_ceil(8),
            params.input_height.div_ceil(8),
            1,
        );
    }

    pub(super) fn dispatch_output(
        &self,
        pass: &mut wgpu::ComputePass<'_>,
        tile: &SpanBridgeTile,
        output: &SpanBridgeOutput,
        params: SpanBridgeParams,
    ) {
        pass.set_pipeline(&self.output_pipeline);
        pass.set_bind_group(0, &tile.input_bind_group, &[]);
        pass.set_bind_group(1, &output.bind_group, &[]);
        pass.dispatch_workgroups(
            params.copy_width.div_ceil(8),
            params.copy_height.div_ceil(8),
            1,
        );
    }
}

pub(super) fn bridge_params_for_tile(
    source_size: [usize; 2],
    output_size: [usize; 2],
    spec: SpanTileSpec,
    scale: usize,
    workspace_output_size: [usize; 2],
) -> Option<SpanBridgeParams> {
    let expected_workspace_output =
        checked_output_size([spec.crop_width, spec.crop_height], scale)?;
    if workspace_output_size != expected_workspace_output {
        return None;
    }

    let read_x = spec.x.checked_sub(spec.crop_x)?.checked_mul(scale)?;
    let read_y = spec.y.checked_sub(spec.crop_y)?.checked_mul(scale)?;
    let dest_x = spec.x.checked_mul(scale)?;
    let dest_y = spec.y.checked_mul(scale)?;
    let copy_width = spec.width.checked_mul(scale)?;
    let copy_height = spec.height.checked_mul(scale)?;
    if spec.crop_x.checked_add(spec.crop_width)? > source_size[0]
        || spec.crop_y.checked_add(spec.crop_height)? > source_size[1]
        || read_x.checked_add(copy_width)? > workspace_output_size[0]
        || read_y.checked_add(copy_height)? > workspace_output_size[1]
        || dest_x.checked_add(copy_width)? > output_size[0]
        || dest_y.checked_add(copy_height)? > output_size[1]
    {
        return None;
    }

    Some(SpanBridgeParams {
        source_width: to_u32(source_size[0])?,
        source_height: to_u32(source_size[1])?,
        input_width: to_u32(spec.crop_width)?,
        input_height: to_u32(spec.crop_height)?,
        output_width: to_u32(workspace_output_size[0])?,
        output_height: to_u32(workspace_output_size[1])?,
        dest_width: to_u32(output_size[0])?,
        dest_height: to_u32(output_size[1])?,
        source_x: to_u32(spec.crop_x)?,
        source_y: to_u32(spec.crop_y)?,
        read_x: to_u32(read_x)?,
        read_y: to_u32(read_y)?,
        dest_x: to_u32(dest_x)?,
        dest_y: to_u32(dest_y)?,
        copy_width: to_u32(copy_width)?,
        copy_height: to_u32(copy_height)?,
    })
}

pub(super) fn checked_output_size(source_size: [usize; 2], scale: usize) -> Option<[usize; 2]> {
    if source_size[0] == 0 || source_size[1] == 0 || scale == 0 {
        return None;
    }
    Some([
        source_size[0].checked_mul(scale)?,
        source_size[1].checked_mul(scale)?,
    ])
}

pub(super) fn create_output_texture(device: &wgpu::Device, size: [usize; 2]) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("suisuiview-realtime-span-output"),
        size: extent_for_size(size),
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: TEXTURE_FORMAT,
        usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    })
}

fn to_u32(value: usize) -> Option<u32> {
    u32::try_from(value).ok()
}

fn extent_for_size(size: [usize; 2]) -> wgpu::Extent3d {
    wgpu::Extent3d {
        width: size[0] as u32,
        height: size[1] as u32,
        depth_or_array_layers: 1,
    }
}

fn create_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    entry_point: &'static str,
) -> wgpu::ComputePipeline {
    device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some(entry_point),
        layout: Some(layout),
        module: shader,
        entry_point: Some(entry_point),
        compilation_options: Default::default(),
        cache: None,
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

fn storage_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    buffer_storage_entry(binding, false)
}

fn storage_read_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    buffer_storage_entry(binding, true)
}

fn buffer_storage_entry(binding: u32, read_only: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn storage_texture_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::StorageTexture {
            access: wgpu::StorageTextureAccess::WriteOnly,
            format: TEXTURE_FORMAT,
            view_dimension: wgpu::TextureViewDimension::D2,
        },
        count: None,
    }
}

fn uniform_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
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

fn buffer_binding(binding: u32, buffer: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: buffer.as_entire_binding(),
    }
}

#[cfg(test)]
mod tests {
    use super::{bridge_params_for_tile, checked_output_size, SpanBridge};
    use crate::core::sr_lab::gpu::tiled::SpanTileSpec;

    #[test]
    fn checked_output_size_rejects_empty_source() {
        assert_eq!(checked_output_size([0, 8], 2), None);
        assert_eq!(checked_output_size([8, 0], 2), None);
        assert_eq!(checked_output_size([8, 8], 0), None);
        assert_eq!(checked_output_size([8, 8], 2), Some([16, 16]));
    }

    #[test]
    fn bridge_params_copy_only_scaled_tile_interior() {
        let params = bridge_params_for_tile(
            [10, 8],
            [20, 16],
            SpanTileSpec {
                x: 4,
                y: 2,
                width: 3,
                height: 2,
                crop_x: 1,
                crop_y: 0,
                crop_width: 8,
                crop_height: 5,
            },
            2,
            [16, 10],
        )
        .unwrap();

        assert_eq!(params.input_width, 8);
        assert_eq!(params.input_height, 5);
        assert_eq!(params.source_x, 1);
        assert_eq!(params.source_y, 0);
        assert_eq!(params.read_x, 6);
        assert_eq!(params.read_y, 4);
        assert_eq!(params.dest_x, 8);
        assert_eq!(params.dest_y, 4);
        assert_eq!(params.copy_width, 6);
        assert_eq!(params.copy_height, 4);
    }

    #[test]
    fn bridge_shader_pipelines_compile_when_wgpu_is_available() {
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
                    label: Some("suisuiview-realtime-span-test-device"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default().using_resolution(adapter.limits()),
                    memory_hints: wgpu::MemoryHints::Performance,
                    trace: wgpu::Trace::Off,
                })
                .await
            else {
                return;
            };

            let _bridge = SpanBridge::new(&device);
        });
    }
}
