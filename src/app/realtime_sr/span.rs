use super::{RealtimeSrOutput, TEXTURE_FORMAT};
use crate::core::sr_lab::{
    self,
    blob::{self, SrLabWeights},
    cpu::FeatureMap,
    gpu::{
        buffers::SpanGpuModel,
        kernel::{SpanGpuKernel, SpanGpuWorkspace},
    },
    sha256::sha256_hex,
    SrLabFamily, SrLabManifest,
};
use std::borrow::Cow;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use wgpu::util::DeviceExt;

const EXPERIMENT_SPAN_MANIFEST_ENV: &str = "SUISUIVIEW_EXPERIMENT_SPAN_MANIFEST";
const SR_LAB_SPAN_MANIFEST_ENV: &str = "SUISUIVIEW_SR_LAB_SPAN_MANIFEST";
const MAX_WEIGHT_BLOB_BYTES: u64 = 256 * 1024 * 1024;
const MAX_DISPLAY_TRANSIENT_BYTES: u64 = 96 * 1024 * 1024;
const OUTPUT_BYTES_PER_PIXEL: usize = 4;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct SpanBridgeParams {
    source_width: u32,
    source_height: u32,
    output_width: u32,
    output_height: u32,
}

pub(super) struct SpanRenderer {
    state: SpanRendererState,
}

enum SpanRendererState {
    Pending,
    Ready(Box<LoadedSpanRenderer>),
    Disabled,
}

struct LoadedSpanRenderer {
    manifest: SrLabManifest,
    model: SpanGpuModel,
    kernel: SpanGpuKernel,
    input_bind_group_layout: wgpu::BindGroupLayout,
    output_bind_group_layout: wgpu::BindGroupLayout,
    input_pipeline: wgpu::ComputePipeline,
    output_pipeline: wgpu::ComputePipeline,
    workspace: Option<SpanWorkspaceSlot>,
}

struct SpanWorkspaceSlot {
    source_size: [usize; 2],
    workspace: SpanGpuWorkspace,
}

impl SpanRenderer {
    pub(super) fn new() -> Self {
        Self {
            state: SpanRendererState::Pending,
        }
    }

    pub(super) fn render(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        source_view: &wgpu::TextureView,
        source_size: [usize; 2],
    ) -> Option<RealtimeSrOutput> {
        if matches!(self.state, SpanRendererState::Pending) {
            self.state = match LoadedSpanRenderer::new(device) {
                Ok(renderer) => SpanRendererState::Ready(Box::new(renderer)),
                Err(_error) => SpanRendererState::Disabled,
            };
        }

        let SpanRendererState::Ready(renderer) = &mut self.state else {
            return None;
        };
        renderer.render(device, encoder, source_view, source_size)
    }
}

impl LoadedSpanRenderer {
    fn new(device: &wgpu::Device) -> Result<Self, String> {
        let manifest_path = span_manifest_path()
            .ok_or_else(|| "SPAN display experiment requires a manifest env var".to_owned())?;
        let manifest = sr_lab::read_manifest(&manifest_path).map_err(|error| error.to_string())?;
        sr_lab::inspect_manifest(&manifest).map_err(|error| error.to_string())?;
        validate_display_manifest(&manifest)?;
        let weights = read_checked_weights(&manifest_path, &manifest)?;

        let kernel = SpanGpuKernel::new(device.clone());
        let model = SpanGpuModel::from_weights(device, &weights);
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

        Ok(Self {
            manifest,
            model,
            kernel,
            input_bind_group_layout,
            output_bind_group_layout,
            input_pipeline,
            output_pipeline,
            workspace: None,
        })
    }

    fn render(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        source_view: &wgpu::TextureView,
        source_size: [usize; 2],
    ) -> Option<RealtimeSrOutput> {
        let output_size = checked_output_size(source_size, self.manifest.scale as usize)?;
        if !fits_texture_limit(device, output_size) {
            return None;
        }
        self.ensure_workspace(source_size).ok()?;
        let workspace = &self.workspace.as_ref()?.workspace;
        let workspace_output_size = workspace.output_size();
        if workspace_output_size != output_size {
            return None;
        }

        let output_texture = create_output_texture(device, output_size);
        let output_view = output_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let params_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("suisuiview-realtime-span-bridge-params"),
            contents: bytemuck::bytes_of(&SpanBridgeParams {
                source_width: source_size[0] as u32,
                source_height: source_size[1] as u32,
                output_width: output_size[0] as u32,
                output_height: output_size[1] as u32,
            }),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let input_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("suisuiview-realtime-span-input-bind-group"),
            layout: &self.input_bind_group_layout,
            entries: &[
                texture_binding(0, source_view),
                buffer_binding(1, workspace.input_buffer()),
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: params_buffer.as_entire_binding(),
                },
            ],
        });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("suisuiview-realtime-span-rgba-to-chw"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.input_pipeline);
            pass.set_bind_group(0, &input_bind_group, &[]);
            pass.dispatch_workgroups(
                (source_size[0] as u32).div_ceil(8),
                (source_size[1] as u32).div_ceil(8),
                1,
            );
        }

        self.kernel
            .encode_workspace(encoder, &self.manifest, &self.model, workspace)
            .ok()?;

        let output_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("suisuiview-realtime-span-output-bind-group"),
            layout: &self.output_bind_group_layout,
            entries: &[
                buffer_binding(0, workspace.output_buffer()),
                texture_binding(1, &output_view),
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: params_buffer.as_entire_binding(),
                },
            ],
        });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("suisuiview-realtime-span-chw-to-rgba"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.output_pipeline);
            pass.set_bind_group(0, &input_bind_group, &[]);
            pass.set_bind_group(1, &output_bind_group, &[]);
            pass.dispatch_workgroups(
                (output_size[0] as u32).div_ceil(8),
                (output_size[1] as u32).div_ceil(8),
                1,
            );
        }

        Some(RealtimeSrOutput {
            texture: output_texture,
            view: output_view,
            size: output_size,
            byte_size: output_size[0]
                .saturating_mul(output_size[1])
                .saturating_mul(OUTPUT_BYTES_PER_PIXEL),
        })
    }

    fn ensure_workspace(&mut self, source_size: [usize; 2]) -> Result<(), String> {
        if self
            .workspace
            .as_ref()
            .is_some_and(|slot| slot.source_size == source_size)
        {
            return Ok(());
        }
        let input = FeatureMap {
            channels: 3,
            height: source_size[1],
            width: source_size[0],
            values: Vec::new(),
        };
        let workspace_bytes = self
            .kernel
            .workspace_byte_size(&self.manifest, &input, false)?;
        if workspace_bytes > MAX_DISPLAY_TRANSIENT_BYTES {
            return Err(format!(
                "SPAN display experiment would allocate about {} MiB of transient buffers, above the {} MiB display limit",
                workspace_bytes.div_ceil(1024 * 1024),
                MAX_DISPLAY_TRANSIENT_BYTES.div_ceil(1024 * 1024)
            ));
        }
        let workspace = self
            .kernel
            .create_workspace(&self.manifest, &input, false)?;
        self.workspace = Some(SpanWorkspaceSlot {
            source_size,
            workspace,
        });
        Ok(())
    }
}

fn span_manifest_path() -> Option<PathBuf> {
    env::var_os(EXPERIMENT_SPAN_MANIFEST_ENV)
        .or_else(|| env::var_os(SR_LAB_SPAN_MANIFEST_ENV))
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn validate_display_manifest(manifest: &SrLabManifest) -> Result<(), String> {
    if !matches!(manifest.family, SrLabFamily::Span | SrLabFamily::SpanS) {
        return Err("SPAN display experiment requires a SPAN-family manifest".to_owned());
    }
    if manifest.scale != 2 || manifest.input_channels != 3 || manifest.output_channels != 3 {
        return Err("SPAN display experiment requires a 3-channel x2 RGB manifest".to_owned());
    }
    if !manifest.license.eq_ignore_ascii_case("Apache-2.0") {
        return Err(format!(
            "SPAN display experiment only accepts Apache-2.0 local lab weights, got {}",
            manifest.license
        ));
    }
    Ok(())
}

fn read_checked_weights(
    manifest_path: &Path,
    manifest: &SrLabManifest,
) -> Result<SrLabWeights, String> {
    let weights_file = manifest
        .weights_file
        .as_deref()
        .ok_or_else(|| "SPAN display experiment requires manifest weights_file".to_owned())?;
    let weights_path = manifest_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(weights_file);
    let byte_len = fs::metadata(&weights_path)
        .map_err(|error| error.to_string())?
        .len();
    if byte_len > MAX_WEIGHT_BLOB_BYTES {
        return Err(format!(
            "SPAN display weight blob is too large: {} bytes",
            byte_len
        ));
    }
    let bytes = fs::read(&weights_path).map_err(|error| error.to_string())?;
    let actual_sha256 = sha256_hex(&bytes);
    if !actual_sha256.eq_ignore_ascii_case(&manifest.weights_sha256) {
        return Err(format!(
            "SPAN display weight SHA-256 mismatch for {}",
            weights_path.display()
        ));
    }
    blob::parse_weights(&bytes)
}

fn checked_output_size(source_size: [usize; 2], scale: usize) -> Option<[usize; 2]> {
    if source_size[0] == 0 || source_size[1] == 0 || scale == 0 {
        return None;
    }
    Some([
        source_size[0].checked_mul(scale)?,
        source_size[1].checked_mul(scale)?,
    ])
}

fn fits_texture_limit(device: &wgpu::Device, size: [usize; 2]) -> bool {
    let max = device.limits().max_texture_dimension_2d as usize;
    size[0] <= max && size[1] <= max
}

fn create_output_texture(device: &wgpu::Device, size: [usize; 2]) -> wgpu::Texture {
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

fn extent_for_size(size: [usize; 2]) -> wgpu::Extent3d {
    wgpu::Extent3d {
        width: size[0] as u32,
        height: size[1] as u32,
        depth_or_array_layers: 1,
    }
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
    use super::{
        checked_output_size, create_pipeline, storage_entry, storage_read_entry,
        storage_texture_entry, texture_entry, uniform_entry,
    };
    use std::borrow::Cow;

    #[test]
    fn checked_output_size_rejects_empty_source() {
        assert_eq!(checked_output_size([0, 8], 2), None);
        assert_eq!(checked_output_size([8, 0], 2), None);
        assert_eq!(checked_output_size([8, 8], 0), None);
        assert_eq!(checked_output_size([8, 8], 2), Some([16, 16]));
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

            let input_bind_group_layout =
                device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("suisuiview-realtime-span-test-input-layout"),
                    entries: &[texture_entry(0), storage_entry(1), uniform_entry(2)],
                });
            let output_bind_group_layout =
                device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("suisuiview-realtime-span-test-output-layout"),
                    entries: &[
                        storage_read_entry(0),
                        storage_texture_entry(1),
                        uniform_entry(2),
                    ],
                });
            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("suisuiview-realtime-span-test-shader"),
                source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(include_str!("span.wgsl"))),
            });
            let input_pipeline_layout =
                device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("suisuiview-realtime-span-test-input-pipeline-layout"),
                    bind_group_layouts: &[&input_bind_group_layout],
                    push_constant_ranges: &[],
                });
            let output_pipeline_layout =
                device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("suisuiview-realtime-span-test-output-pipeline-layout"),
                    bind_group_layouts: &[&input_bind_group_layout, &output_bind_group_layout],
                    push_constant_ranges: &[],
                });

            let _input_pipeline =
                create_pipeline(&device, &input_pipeline_layout, &shader, "span_rgba_to_chw");
            let _output_pipeline = create_pipeline(
                &device,
                &output_pipeline_layout,
                &shader,
                "span_chw_to_rgba",
            );
        });
    }
}
