use std::time::{Duration, Instant};
use std::{borrow::Cow, fmt::Write as _};

use wgpu::util::DeviceExt;

use super::gpu_effect_worker::{WgpuEffectRenderer, WgpuEffectScenario};

pub(crate) const PROBE_IMAGE_SIZE: [usize; 2] = [256, 256];

#[derive(Clone, Debug)]
pub(crate) enum WgpuProbeInput {
    Synthetic,
    Rgba {
        image_size: [usize; 2],
        rgba: Vec<u8>,
    },
    Effect {
        image_size: [usize; 2],
        rgba: Vec<u8>,
        scenario: WgpuEffectScenario,
    },
}

#[derive(Clone, Debug)]
pub(crate) struct WgpuProbeReport {
    pub(crate) worker_started_ms: f64,
    pub(crate) init_ms: Option<f64>,
    pub(crate) compute_readback_ms: Option<f64>,
    pub(crate) shader_module_ms: Option<f64>,
    pub(crate) pipeline_ms: Option<f64>,
    pub(crate) upload_ms: Option<f64>,
    pub(crate) setup_ms: Option<f64>,
    pub(crate) encode_submit_ms: Option<f64>,
    pub(crate) readback_ms: Option<f64>,
    pub(crate) backend: Option<&'static str>,
    pub(crate) device_type: Option<&'static str>,
    pub(crate) checksum: Option<u64>,
    pub(crate) source_size: [usize; 2],
    pub(crate) image_size: [usize; 2],
    pub(crate) mode: &'static str,
    pub(crate) rgba: Option<Vec<u8>>,
    pub(crate) error: Option<String>,
}

pub(crate) fn spawn_wgpu_probe(
    started_at: Instant,
    input: WgpuProbeInput,
    on_report: impl FnOnce(WgpuProbeReport) + Send + 'static,
) {
    std::thread::Builder::new()
        .name("suisuiview-runtime-probe-wgpu".to_owned())
        .spawn(move || {
            on_report(run_wgpu_probe_blocking(started_at, input));
        })
        .expect("failed to spawn WGPU probe thread");
}

pub(crate) fn run_wgpu_probe_blocking(
    started_at: Instant,
    input: WgpuProbeInput,
) -> WgpuProbeReport {
    pollster::block_on(run_wgpu_probe(started_at, input))
}

async fn run_wgpu_probe(started_at: Instant, input: WgpuProbeInput) -> WgpuProbeReport {
    let worker_started_ms = elapsed_ms(started_at.elapsed());
    let source_size = input.image_size();
    let mode = input.mode();
    let init_started = Instant::now();
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::PRIMARY,
        flags: wgpu::InstanceFlags::empty().with_env(),
        backend_options: Default::default(),
    });
    let adapter = match instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
        })
        .await
    {
        Ok(adapter) => adapter,
        Err(error) => {
            return failed_report(
                worker_started_ms,
                source_size,
                mode,
                format!("request_adapter failed: {error}"),
            );
        }
    };
    let info = adapter.get_info();
    let (device, queue) = match adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("suisuiview-runtime-probe-device"),
            required_features: wgpu::Features::empty(),
            required_limits: adapter.limits(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        })
        .await
    {
        Ok(device_queue) => device_queue,
        Err(error) => {
            return failed_report(
                worker_started_ms,
                source_size,
                mode,
                format!("request_device failed: {error}"),
            );
        }
    };
    let init_ms = elapsed_ms(init_started.elapsed());
    let compute_started = Instant::now();
    let output = match compute_probe_rgba(&device, &queue, input) {
        Ok(output) => output,
        Err(error) => {
            return WgpuProbeReport {
                worker_started_ms,
                init_ms: Some(init_ms),
                compute_readback_ms: None,
                shader_module_ms: None,
                pipeline_ms: None,
                upload_ms: None,
                setup_ms: None,
                encode_submit_ms: None,
                readback_ms: None,
                backend: Some(info.backend.to_str()),
                device_type: Some(device_type_label(info.device_type)),
                checksum: None,
                source_size,
                image_size: source_size,
                mode,
                rgba: None,
                error: Some(error),
            };
        }
    };
    let compute_readback_ms = output
        .total_ms
        .unwrap_or_else(|| elapsed_ms(compute_started.elapsed()));
    WgpuProbeReport {
        worker_started_ms,
        init_ms: Some(init_ms),
        compute_readback_ms: Some(compute_readback_ms),
        shader_module_ms: output.shader_module_ms,
        pipeline_ms: output.pipeline_ms,
        upload_ms: output.upload_ms,
        setup_ms: output.setup_ms,
        encode_submit_ms: output.encode_submit_ms,
        readback_ms: output.readback_ms,
        backend: Some(info.backend.to_str()),
        device_type: Some(device_type_label(info.device_type)),
        checksum: Some(output.checksum),
        source_size,
        image_size: output.output_size,
        mode,
        rgba: Some(output.rgba),
        error: None,
    }
}

impl WgpuProbeInput {
    fn image_size(&self) -> [usize; 2] {
        match self {
            Self::Synthetic => PROBE_IMAGE_SIZE,
            Self::Rgba { image_size, .. } => *image_size,
            Self::Effect { image_size, .. } => *image_size,
        }
    }

    fn mode(&self) -> &'static str {
        match self {
            Self::Synthetic => "synthetic",
            Self::Rgba { .. } => "rgba_copy",
            Self::Effect { scenario, .. } => scenario.token(),
        }
    }
}

struct ProbeWorkOutput {
    rgba: Vec<u8>,
    output_size: [usize; 2],
    checksum: u64,
    total_ms: Option<f64>,
    shader_module_ms: Option<f64>,
    pipeline_ms: Option<f64>,
    upload_ms: Option<f64>,
    setup_ms: Option<f64>,
    encode_submit_ms: Option<f64>,
    readback_ms: Option<f64>,
}

fn compute_probe_rgba(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    input: WgpuProbeInput,
) -> Result<ProbeWorkOutput, String> {
    if let WgpuProbeInput::Effect {
        image_size,
        rgba,
        scenario,
    } = input
    {
        let renderer = WgpuEffectRenderer::new(device);
        let run = renderer.run(device, queue, image_size, rgba.as_slice(), scenario)?;
        let total_ms = run.total_ms + run.shader_module_ms + run.pipeline_ms;
        return Ok(ProbeWorkOutput {
            rgba: run.rgba,
            output_size: run.output_size,
            checksum: run.checksum,
            total_ms: Some(total_ms),
            shader_module_ms: Some(run.shader_module_ms),
            pipeline_ms: Some(run.pipeline_ms),
            upload_ms: Some(run.upload_ms),
            setup_ms: Some(run.setup_ms),
            encode_submit_ms: Some(run.encode_submit_ms),
            readback_ms: Some(run.readback_ms),
        });
    }
    let [width, height] = input.image_size();
    let byte_len = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or("probe image byte size overflowed")?;
    let (shader_source, input_buffer) = match input {
        WgpuProbeInput::Synthetic => (synthetic_shader(width, height), None),
        WgpuProbeInput::Rgba { rgba, .. } => {
            if rgba.len() != byte_len {
                return Err(format!(
                    "rgba length {} does not match expected {byte_len}",
                    rgba.len()
                ));
            }
            let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("suisuiview-runtime-probe-input"),
                contents: rgba.as_slice(),
                usage: wgpu::BufferUsages::STORAGE,
            });
            (copy_shader(width, height), Some(buffer))
        }
        WgpuProbeInput::Effect { .. } => unreachable!("effect probes return before compute setup"),
    };
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("suisuiview-runtime-probe-shader"),
        source: wgpu::ShaderSource::Wgsl(Cow::Owned(shader_source)),
    });
    let output = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("suisuiview-runtime-probe-output"),
        size: byte_len as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("suisuiview-runtime-probe-readback"),
        size: byte_len as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let layout_entries = bind_group_layout_entries(input_buffer.is_some());
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("suisuiview-runtime-probe-bind-group-layout"),
        entries: &layout_entries,
    });
    let mut entries = Vec::with_capacity(if input_buffer.is_some() { 2 } else { 1 });
    if let Some(input_buffer) = input_buffer.as_ref() {
        entries.push(wgpu::BindGroupEntry {
            binding: 0,
            resource: input_buffer.as_entire_binding(),
        });
        entries.push(wgpu::BindGroupEntry {
            binding: 1,
            resource: output.as_entire_binding(),
        });
    } else {
        entries.push(wgpu::BindGroupEntry {
            binding: 0,
            resource: output.as_entire_binding(),
        });
    }
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("suisuiview-runtime-probe-bind-group"),
        layout: &layout,
        entries: &entries,
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("suisuiview-runtime-probe-pipeline-layout"),
        bind_group_layouts: &[&layout],
        push_constant_ranges: &[],
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("suisuiview-runtime-probe-pipeline"),
        layout: Some(&pipeline_layout),
        module: &shader,
        entry_point: Some("main"),
        compilation_options: Default::default(),
        cache: None,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("suisuiview-runtime-probe-encoder"),
    });
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("suisuiview-runtime-probe-pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.dispatch_workgroups((width as u32).div_ceil(8), (height as u32).div_ceil(8), 1);
    }
    encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, byte_len as u64);
    queue.submit(Some(encoder.finish()));

    let slice = readback.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = tx.send(result);
    });
    device
        .poll(wgpu::PollType::Wait)
        .map_err(|error| format!("device poll failed: {error}"))?;
    rx.recv()
        .map_err(|error| format!("readback channel failed: {error}"))?
        .map_err(|error| format!("map_async failed: {error}"))?;
    let mapped = slice.get_mapped_range();
    let rgba = mapped.to_vec();
    drop(mapped);
    readback.unmap();
    let checksum = rgba
        .iter()
        .fold(0u64, |sum, byte| sum.wrapping_add(*byte as u64));
    Ok(ProbeWorkOutput {
        rgba,
        output_size: [width, height],
        checksum,
        total_ms: None,
        shader_module_ms: None,
        pipeline_ms: None,
        upload_ms: None,
        setup_ms: None,
        encode_submit_ms: None,
        readback_ms: None,
    })
}

fn bind_group_layout_entries(has_input: bool) -> Vec<wgpu::BindGroupLayoutEntry> {
    let storage_binding = |binding, read_only| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    };
    if has_input {
        vec![storage_binding(0, true), storage_binding(1, false)]
    } else {
        vec![storage_binding(0, false)]
    }
}

fn synthetic_shader(width: usize, height: usize) -> String {
    let mut shader = shader_header(width, height);
    shader.push_str(
        r#"
@group(0) @binding(0)
var<storage, read_write> output_pixels: array<u32>;

@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= WIDTH || id.y >= HEIGHT) {
        return;
    }
    let index = id.y * WIDTH + id.x;
    let r = id.x & 255u;
    let g = id.y & 255u;
    let b = (id.x + id.y) & 255u;
    output_pixels[index] = 0xff000000u | (b << 16u) | (g << 8u) | r;
}
"#,
    );
    shader
}

fn copy_shader(width: usize, height: usize) -> String {
    let mut shader = shader_header(width, height);
    shader.push_str(
        r#"
@group(0) @binding(0)
var<storage, read> input_pixels: array<u32>;

@group(0) @binding(1)
var<storage, read_write> output_pixels: array<u32>;

@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= WIDTH || id.y >= HEIGHT) {
        return;
    }
    let index = id.y * WIDTH + id.x;
    output_pixels[index] = input_pixels[index];
}
"#,
    );
    shader
}

fn shader_header(width: usize, height: usize) -> String {
    let mut shader = String::new();
    let _ = writeln!(shader, "const WIDTH: u32 = {width}u;");
    let _ = writeln!(shader, "const HEIGHT: u32 = {height}u;");
    shader
}

fn failed_report(
    worker_started_ms: f64,
    source_size: [usize; 2],
    mode: &'static str,
    error: String,
) -> WgpuProbeReport {
    WgpuProbeReport {
        worker_started_ms,
        init_ms: None,
        compute_readback_ms: None,
        shader_module_ms: None,
        pipeline_ms: None,
        upload_ms: None,
        setup_ms: None,
        encode_submit_ms: None,
        readback_ms: None,
        backend: None,
        device_type: None,
        checksum: None,
        source_size,
        image_size: source_size,
        mode,
        rgba: None,
        error: Some(error),
    }
}

fn device_type_label(device_type: wgpu::DeviceType) -> &'static str {
    match device_type {
        wgpu::DeviceType::IntegratedGpu => "integrated_gpu",
        wgpu::DeviceType::DiscreteGpu => "discrete_gpu",
        wgpu::DeviceType::VirtualGpu => "virtual_gpu",
        wgpu::DeviceType::Cpu => "cpu",
        wgpu::DeviceType::Other => "other",
    }
}

pub(crate) fn elapsed_ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}
