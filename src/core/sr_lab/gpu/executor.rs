use crate::core::sr_lab::blob::SrLabWeights;
use crate::core::sr_lab::cpu::FeatureMap;
use crate::core::sr_lab::{SrLabFamily, SrLabManifest};
use std::borrow::Cow;
use std::sync::mpsc;
use std::time::Instant;
use wgpu::util::DeviceExt;

const MAX_TRANSIENT_BYTES: u64 = 768 * 1024 * 1024;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct SpanParams {
    width: u32,
    height: u32,
    input_channels: u32,
    output_channels: u32,
    kernel: u32,
    padding: u32,
    scale: u32,
    activation: u32,
    rgb_mean: [f32; 4],
    img_range: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
}

pub(super) struct SpanGpuExecutor {
    device: wgpu::Device,
    queue: wgpu::Queue,
    bind_group_layout: wgpu::BindGroupLayout,
    mean_shift_pipeline: wgpu::ComputePipeline,
    conv2d_pipeline: wgpu::ComputePipeline,
    gate_pipeline: wgpu::ComputePipeline,
    concat_pipeline: wgpu::ComputePipeline,
    pixel_shuffle_pipeline: wgpu::ComputePipeline,
    dummy: GpuBuffer,
}

pub(super) struct SpanGpuModel {
    tensors: Vec<GpuTensor>,
}

struct GpuTensor {
    name: String,
    shape: Vec<u32>,
    buffer: wgpu::Buffer,
}

struct GpuBuffer {
    buffer: wgpu::Buffer,
    channels: usize,
    height: usize,
    width: usize,
}

pub(super) struct RunStats {
    pub(super) output: FeatureMap,
    pub(super) elapsed_ms: f64,
}

impl SpanGpuExecutor {
    pub(super) fn new() -> Result<Self, String> {
        pollster::block_on(Self::new_async())
    }

    async fn new_async() -> Result<Self, String> {
        let instance = wgpu::Instance::default();
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: None,
                force_fallback_adapter: false,
            })
            .await
            .map_err(|error| format!("wgpu adapter unavailable: {error}"))?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("suisuiview-sr-lab-span-device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                memory_hints: wgpu::MemoryHints::Performance,
                trace: wgpu::Trace::Off,
            })
            .await
            .map_err(|error| format!("wgpu device unavailable: {error}"))?;

        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let executor = Self::new_with_device(device, queue);
        match executor.device.pop_error_scope().await {
            Some(error) => Err(format!("SPAN GPU reference pipeline unavailable: {error}")),
            None => Ok(executor),
        }
    }

    fn new_with_device(device: wgpu::Device, queue: wgpu::Queue) -> Self {
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("suisuiview-sr-lab-span-bind-group-layout"),
            entries: &[
                storage_read_entry(0),
                storage_read_entry(1),
                storage_read_entry(2),
                storage_read_entry(3),
                storage_read_entry(4),
                storage_read_entry(5),
                storage_read_write_entry(6),
                wgpu::BindGroupLayoutEntry {
                    binding: 7,
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
            label: Some("suisuiview-sr-lab-span-pipeline-layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("suisuiview-sr-lab-span-shader"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(include_str!("../span.wgsl"))),
        });
        let mean_shift_pipeline =
            create_pipeline(&device, &pipeline_layout, &shader, "span_mean_shift");
        let conv2d_pipeline = create_pipeline(&device, &pipeline_layout, &shader, "span_conv2d");
        let gate_pipeline = create_pipeline(&device, &pipeline_layout, &shader, "span_gate");
        let concat_pipeline = create_pipeline(&device, &pipeline_layout, &shader, "span_concat4");
        let pixel_shuffle_pipeline =
            create_pipeline(&device, &pipeline_layout, &shader, "span_pixel_shuffle2x");
        let dummy = buffer_from_values(&device, "suisuiview-sr-lab-span-dummy", 1, 1, 1, &[0.0]);

        Self {
            device,
            queue,
            bind_group_layout,
            mean_shift_pipeline,
            conv2d_pipeline,
            gate_pipeline,
            concat_pipeline,
            pixel_shuffle_pipeline,
            dummy,
        }
    }

    pub(super) fn upload_model(&self, weights: &SrLabWeights) -> SpanGpuModel {
        let tensors = weights
            .tensors
            .iter()
            .map(|tensor| GpuTensor {
                name: tensor.name.clone(),
                shape: tensor.shape.clone(),
                buffer: self
                    .device
                    .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some(&format!("suisuiview-sr-lab-span-{}", tensor.name)),
                        contents: bytemuck::cast_slice(&tensor.values),
                        usage: wgpu::BufferUsages::STORAGE,
                    }),
            })
            .collect();
        SpanGpuModel { tensors }
    }

    pub(super) fn run(
        &self,
        manifest: &SrLabManifest,
        model: &SpanGpuModel,
        input: &FeatureMap,
    ) -> Result<RunStats, String> {
        validate_span_manifest(manifest, input)?;
        let span = manifest
            .span
            .as_ref()
            .ok_or_else(|| "SPAN GPU reference requires span metadata".to_owned())?;
        let feature_channels = span.feature_channels as usize;
        let output_channels = manifest.output_channels as usize;
        validate_transient_size(
            input,
            feature_channels,
            output_channels,
            manifest.scale as usize,
        )?;

        let input_buffer = buffer_from_values(
            &self.device,
            "suisuiview-sr-lab-span-input",
            input.channels,
            input.height,
            input.width,
            &input.values,
        );
        let feature_buffer = |label| {
            empty_buffer(
                &self.device,
                label,
                feature_channels,
                input.height,
                input.width,
            )
        };
        let shifted = empty_buffer(
            &self.device,
            "suisuiview-sr-lab-span-shifted",
            input.channels,
            input.height,
            input.width,
        );
        let out_feature = feature_buffer("suisuiview-sr-lab-span-out-feature");
        let current_a = feature_buffer("suisuiview-sr-lab-span-current-a");
        let current_b = feature_buffer("suisuiview-sr-lab-span-current-b");
        let out1 = feature_buffer("suisuiview-sr-lab-span-out1");
        let out2 = feature_buffer("suisuiview-sr-lab-span-out2");
        let out3 = feature_buffer("suisuiview-sr-lab-span-out3");
        let out_b1 = feature_buffer("suisuiview-sr-lab-span-out-b1");
        let out_b5_2 = feature_buffer("suisuiview-sr-lab-span-out-b5-2");
        let out_b6 = feature_buffer("suisuiview-sr-lab-span-out-b6");
        let joined = empty_buffer(
            &self.device,
            "suisuiview-sr-lab-span-joined",
            feature_channels * 4,
            input.height,
            input.width,
        );
        let cat = empty_buffer(
            &self.device,
            "suisuiview-sr-lab-span-cat",
            feature_channels,
            input.height,
            input.width,
        );
        let up = empty_buffer(
            &self.device,
            "suisuiview-sr-lab-span-up",
            output_channels * manifest.scale as usize * manifest.scale as usize,
            input.height,
            input.width,
        );
        let output = empty_buffer(
            &self.device,
            "suisuiview-sr-lab-span-output",
            output_channels,
            input.height * manifest.scale as usize,
            input.width * manifest.scale as usize,
        );

        let started = Instant::now();
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("suisuiview-sr-lab-span-encoder"),
            });

        self.run_mean_shift(
            &mut encoder,
            &input_buffer,
            &shifted,
            span.rgb_mean,
            span.img_range,
        );
        self.run_conv(
            &mut encoder,
            model,
            &shifted,
            &out_feature,
            "conv_1",
            3,
            1,
            false,
        )?;
        encoder.copy_buffer_to_buffer(
            &out_feature.buffer,
            0,
            &current_a.buffer,
            0,
            current_a.byte_len(),
        );

        let mut current_is_a = true;
        for block in 1..=span.block_count {
            let current = if current_is_a { &current_a } else { &current_b };
            let next = if current_is_a { &current_b } else { &current_a };
            self.run_conv(
                &mut encoder,
                model,
                current,
                &out1,
                &format!("block_{block}.c1_r"),
                3,
                1,
                false,
            )?;
            self.run_conv(
                &mut encoder,
                model,
                &out1,
                &out2,
                &format!("block_{block}.c2_r"),
                3,
                1,
                true,
            )?;
            self.run_conv(
                &mut encoder,
                model,
                &out2,
                &out3,
                &format!("block_{block}.c3_r"),
                3,
                1,
                true,
            )?;
            self.run_gate(&mut encoder, &out3, current, next);
            if block == 1 {
                encoder.copy_buffer_to_buffer(
                    &next.buffer,
                    0,
                    &out_b1.buffer,
                    0,
                    out_b1.byte_len(),
                );
            }
            if block == span.block_count {
                encoder.copy_buffer_to_buffer(
                    &out1.buffer,
                    0,
                    &out_b5_2.buffer,
                    0,
                    out_b5_2.byte_len(),
                );
            }
            current_is_a = !current_is_a;
        }

        let current = if current_is_a { &current_a } else { &current_b };
        self.run_conv(&mut encoder, model, current, &out_b6, "conv_2", 3, 1, false)?;
        self.run_concat4(
            &mut encoder,
            &out_feature,
            &out_b6,
            &out_b1,
            &out_b5_2,
            &joined,
            feature_channels,
        );
        self.run_conv(&mut encoder, model, &joined, &cat, "conv_cat", 1, 0, false)?;
        self.run_conv(&mut encoder, model, &cat, &up, "upsampler.0", 3, 1, false)?;
        self.run_pixel_shuffle(&mut encoder, &up, &output, manifest.scale);

        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("suisuiview-sr-lab-span-readback"),
            size: output.byte_len(),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_buffer_to_buffer(&output.buffer, 0, &readback, 0, output.byte_len());
        self.queue.submit(Some(encoder.finish()));

        let slice = readback.slice(..);
        let (sender, receiver) = mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result.map_err(|error| error.to_string()));
        });
        self.device
            .poll(wgpu::PollType::Wait)
            .map_err(|error| format!("wgpu poll failed: {error}"))?;
        receiver
            .recv()
            .map_err(|error| format!("wgpu readback channel failed: {error}"))?
            .map_err(|error| format!("wgpu readback failed: {error}"))?;
        let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
        let mapped = slice.get_mapped_range();
        let values = bytemuck::cast_slice::<u8, f32>(&mapped).to_vec();
        drop(mapped);
        readback.unmap();

        Ok(RunStats {
            output: FeatureMap {
                channels: output.channels,
                height: output.height,
                width: output.width,
                values,
            },
            elapsed_ms,
        })
    }

    fn run_mean_shift(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        input: &GpuBuffer,
        output: &GpuBuffer,
        rgb_mean: [f32; 3],
        img_range: f32,
    ) {
        let mut mean = [0.0; 4];
        mean[..3].copy_from_slice(&rgb_mean);
        let params = SpanParams {
            width: input.width as u32,
            height: input.height as u32,
            input_channels: input.channels as u32,
            output_channels: output.channels as u32,
            kernel: 1,
            padding: 0,
            scale: 1,
            activation: 0,
            rgb_mean: mean,
            img_range,
            _pad0: 0.0,
            _pad1: 0.0,
            _pad2: 0.0,
        };
        self.dispatch(
            encoder,
            &self.mean_shift_pipeline,
            params,
            [input, &self.dummy, &self.dummy, &self.dummy],
            &self.dummy.buffer,
            &self.dummy.buffer,
            output,
        );
    }

    fn run_conv(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        model: &SpanGpuModel,
        input: &GpuBuffer,
        output: &GpuBuffer,
        name: &str,
        kernel: u32,
        padding: u32,
        activate_input: bool,
    ) -> Result<(), String> {
        let weight = model.tensor(&format!("{name}.weight"))?;
        let bias = model.tensor(&format!("{name}.bias"))?;
        validate_conv_shape(input, output, weight, bias, kernel, name)?;
        let params = SpanParams {
            width: input.width as u32,
            height: input.height as u32,
            input_channels: input.channels as u32,
            output_channels: output.channels as u32,
            kernel,
            padding,
            scale: 1,
            activation: u32::from(activate_input),
            rgb_mean: [0.0; 4],
            img_range: 1.0,
            _pad0: 0.0,
            _pad1: 0.0,
            _pad2: 0.0,
        };
        self.dispatch(
            encoder,
            &self.conv2d_pipeline,
            params,
            [input, &self.dummy, &self.dummy, &self.dummy],
            &weight.buffer,
            &bias.buffer,
            output,
        );
        Ok(())
    }

    fn run_gate(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        out3: &GpuBuffer,
        current: &GpuBuffer,
        output: &GpuBuffer,
    ) {
        let params = SpanParams {
            width: out3.width as u32,
            height: out3.height as u32,
            input_channels: out3.channels as u32,
            output_channels: output.channels as u32,
            kernel: 1,
            padding: 0,
            scale: 1,
            activation: 0,
            rgb_mean: [0.0; 4],
            img_range: 1.0,
            _pad0: 0.0,
            _pad1: 0.0,
            _pad2: 0.0,
        };
        self.dispatch(
            encoder,
            &self.gate_pipeline,
            params,
            [out3, current, &self.dummy, &self.dummy],
            &self.dummy.buffer,
            &self.dummy.buffer,
            output,
        );
    }

    fn run_concat4(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        a: &GpuBuffer,
        b: &GpuBuffer,
        c: &GpuBuffer,
        d: &GpuBuffer,
        output: &GpuBuffer,
        feature_channels: usize,
    ) {
        let params = SpanParams {
            width: a.width as u32,
            height: a.height as u32,
            input_channels: feature_channels as u32,
            output_channels: output.channels as u32,
            kernel: 1,
            padding: 0,
            scale: 1,
            activation: 0,
            rgb_mean: [0.0; 4],
            img_range: 1.0,
            _pad0: 0.0,
            _pad1: 0.0,
            _pad2: 0.0,
        };
        self.dispatch(
            encoder,
            &self.concat_pipeline,
            params,
            [a, b, c, d],
            &self.dummy.buffer,
            &self.dummy.buffer,
            output,
        );
    }

    fn run_pixel_shuffle(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        input: &GpuBuffer,
        output: &GpuBuffer,
        scale: u32,
    ) {
        let params = SpanParams {
            width: input.width as u32,
            height: input.height as u32,
            input_channels: input.channels as u32,
            output_channels: output.channels as u32,
            kernel: 1,
            padding: 0,
            scale,
            activation: 0,
            rgb_mean: [0.0; 4],
            img_range: 1.0,
            _pad0: 0.0,
            _pad1: 0.0,
            _pad2: 0.0,
        };
        self.dispatch(
            encoder,
            &self.pixel_shuffle_pipeline,
            params,
            [input, &self.dummy, &self.dummy, &self.dummy],
            &self.dummy.buffer,
            &self.dummy.buffer,
            output,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn dispatch(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        pipeline: &wgpu::ComputePipeline,
        params: SpanParams,
        inputs: [&GpuBuffer; 4],
        weights: &wgpu::Buffer,
        bias: &wgpu::Buffer,
        output: &GpuBuffer,
    ) {
        let params_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("suisuiview-sr-lab-span-params"),
                contents: bytemuck::bytes_of(&params),
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("suisuiview-sr-lab-span-bind-group"),
            layout: &self.bind_group_layout,
            entries: &[
                storage_binding(0, &inputs[0].buffer),
                storage_binding(1, &inputs[1].buffer),
                storage_binding(2, &inputs[2].buffer),
                storage_binding(3, &inputs[3].buffer),
                storage_binding(4, weights),
                storage_binding(5, bias),
                storage_binding(6, &output.buffer),
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: params_buffer.as_entire_binding(),
                },
            ],
        });
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("suisuiview-sr-lab-span-pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.dispatch_workgroups(
            (output.width as u32).div_ceil(8),
            (output.height as u32).div_ceil(8),
            output.channels as u32,
        );
    }
}

impl SpanGpuModel {
    fn tensor(&self, name: &str) -> Result<&GpuTensor, String> {
        self.tensors
            .iter()
            .find(|tensor| tensor.name == name)
            .ok_or_else(|| format!("missing SR Lab GPU tensor: {name}"))
    }
}

impl GpuBuffer {
    fn byte_len(&self) -> u64 {
        (self.channels * self.height * self.width * std::mem::size_of::<f32>()) as u64
    }
}

fn validate_span_manifest(manifest: &SrLabManifest, input: &FeatureMap) -> Result<(), String> {
    if !matches!(manifest.family, SrLabFamily::Span | SrLabFamily::SpanS) {
        return Err("SPAN GPU reference requires a SPAN-family manifest".to_owned());
    }
    if manifest.scale != 2 {
        return Err(format!(
            "SPAN GPU reference currently supports x2 pixel shuffle only, got x{}",
            manifest.scale
        ));
    }
    if manifest.input_channels as usize != input.channels {
        return Err(format!(
            "input channel mismatch: manifest expects {}, image has {}",
            manifest.input_channels, input.channels
        ));
    }
    if manifest.output_channels != 3 {
        return Err("SPAN GPU reference currently supports RGB output only".to_owned());
    }
    let span = manifest
        .span
        .as_ref()
        .ok_or_else(|| "SPAN GPU reference requires span metadata".to_owned())?;
    if span.block_count == 0 || span.feature_channels == 0 {
        return Err("SPAN GPU reference requires positive span metadata".to_owned());
    }
    Ok(())
}

fn validate_transient_size(
    input: &FeatureMap,
    feature_channels: usize,
    output_channels: usize,
    scale: usize,
) -> Result<(), String> {
    let pixel_count = (input.width as u64)
        .checked_mul(input.height as u64)
        .ok_or_else(|| "SPAN GPU transient size overflowed".to_owned())?;
    let input_values = pixel_count
        .checked_mul(input.channels as u64)
        .ok_or_else(|| "SPAN GPU input size overflowed".to_owned())?;
    let feature_values = pixel_count
        .checked_mul(feature_channels as u64)
        .ok_or_else(|| "SPAN GPU feature size overflowed".to_owned())?;
    let joined_values = feature_values
        .checked_mul(4)
        .ok_or_else(|| "SPAN GPU joined size overflowed".to_owned())?;
    let up_values = pixel_count
        .checked_mul(output_channels as u64)
        .and_then(|values| values.checked_mul((scale * scale) as u64))
        .ok_or_else(|| "SPAN GPU upsample size overflowed".to_owned())?;
    let output_values = pixel_count
        .checked_mul(output_channels as u64)
        .and_then(|values| values.checked_mul((scale * scale) as u64))
        .ok_or_else(|| "SPAN GPU output size overflowed".to_owned())?;
    let readback_values = output_values;
    let transient_values = input_values
        .checked_mul(2)
        .and_then(|values| values.checked_add(feature_values.checked_mul(10)?))
        .and_then(|values| values.checked_add(joined_values))
        .and_then(|values| values.checked_add(up_values))
        .and_then(|values| values.checked_add(output_values))
        .and_then(|values| values.checked_add(readback_values))
        .ok_or_else(|| "SPAN GPU transient size overflowed".to_owned())?;
    let transient_bytes = transient_values
        .checked_mul(std::mem::size_of::<f32>() as u64)
        .ok_or_else(|| "SPAN GPU transient byte size overflowed".to_owned())?;
    if transient_bytes > MAX_TRANSIENT_BYTES {
        return Err(format!(
            "SPAN GPU reference would allocate about {} MiB of transient buffers, above the {} MiB safety limit",
            bytes_to_mib(transient_bytes),
            bytes_to_mib(MAX_TRANSIENT_BYTES)
        ));
    }
    Ok(())
}

fn validate_conv_shape(
    input: &GpuBuffer,
    output: &GpuBuffer,
    weight: &GpuTensor,
    bias: &GpuTensor,
    kernel: u32,
    name: &str,
) -> Result<(), String> {
    let expected_weight = vec![
        output.channels as u32,
        input.channels as u32,
        kernel,
        kernel,
    ];
    if weight.shape != expected_weight {
        return Err(format!(
            "{name}.weight shape {:?} does not match expected {:?}",
            weight.shape, expected_weight
        ));
    }
    let expected_bias = vec![output.channels as u32];
    if bias.shape != expected_bias {
        return Err(format!(
            "{name}.bias shape {:?} does not match expected {:?}",
            bias.shape, expected_bias
        ));
    }
    Ok(())
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

fn buffer_from_values(
    device: &wgpu::Device,
    label: &str,
    channels: usize,
    height: usize,
    width: usize,
    values: &[f32],
) -> GpuBuffer {
    debug_assert_eq!(values.len(), channels * height * width);
    GpuBuffer {
        buffer: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(label),
            contents: bytemuck::cast_slice(values),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        }),
        channels,
        height,
        width,
    }
}

fn empty_buffer(
    device: &wgpu::Device,
    label: &str,
    channels: usize,
    height: usize,
    width: usize,
) -> GpuBuffer {
    GpuBuffer {
        buffer: device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: (channels * height * width * std::mem::size_of::<f32>()) as u64,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }),
        channels,
        height,
        width,
    }
}

fn storage_read_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    storage_entry(binding, true)
}

fn storage_read_write_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    storage_entry(binding, false)
}

fn storage_entry(binding: u32, read_only: bool) -> wgpu::BindGroupLayoutEntry {
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

fn storage_binding(binding: u32, buffer: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: buffer.as_entire_binding(),
    }
}

fn bytes_to_mib(bytes: u64) -> u64 {
    bytes.div_ceil(1024 * 1024)
}
