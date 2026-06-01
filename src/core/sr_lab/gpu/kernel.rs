use super::buffers::{
    buffer_from_values, empty_buffer, storage_binding, storage_read_entry,
    storage_read_write_entry, GpuBuffer, SpanGpuModel,
};
use super::model_validation::validate_span_model;
use super::validation::{
    span_transient_byte_size, validate_span_manifest, validate_storage_buffer_sizes,
    validate_transient_size,
};
use crate::core::sr_lab::cpu::FeatureMap;
use crate::core::sr_lab::SrLabManifest;
use std::borrow::Cow;
use wgpu::util::DeviceExt;

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

pub(crate) struct SpanGpuKernel {
    device: wgpu::Device,
    bind_group_layout: wgpu::BindGroupLayout,
    mean_shift_pipeline: wgpu::ComputePipeline,
    conv2d_pipeline: wgpu::ComputePipeline,
    gate_pipeline: wgpu::ComputePipeline,
    concat_pipeline: wgpu::ComputePipeline,
    pixel_shuffle_pipeline: wgpu::ComputePipeline,
    dummy: GpuBuffer,
}

pub(crate) struct SpanGpuWorkspace {
    pub(super) input: GpuBuffer,
    pub(super) shifted: GpuBuffer,
    pub(super) out_feature: GpuBuffer,
    pub(super) current_a: GpuBuffer,
    pub(super) current_b: GpuBuffer,
    pub(super) out1: GpuBuffer,
    pub(super) out2: GpuBuffer,
    pub(super) out3: GpuBuffer,
    pub(super) out_b1: GpuBuffer,
    pub(super) out_b5_2: GpuBuffer,
    pub(super) out_b6: GpuBuffer,
    pub(super) joined: GpuBuffer,
    pub(super) cat: GpuBuffer,
    pub(super) up: GpuBuffer,
    pub(super) output: GpuBuffer,
}

pub(crate) struct SpanGpuGraphPlan {
    steps: Vec<SpanGpuGraphStep>,
}

enum SpanGpuGraphStep {
    Dispatch(SpanGpuDispatchStep),
    Copy(SpanGpuCopyStep),
}

struct SpanGpuDispatchStep {
    pipeline: SpanPipeline,
    bind_group: wgpu::BindGroup,
    _params_buffer: wgpu::Buffer,
    workgroups: [u32; 3],
}

struct SpanGpuCopyStep {
    source: wgpu::Buffer,
    destination: wgpu::Buffer,
    byte_len: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SpanPipeline {
    MeanShift,
    Conv2d,
    Gate,
    Concat4,
    PixelShuffle,
}

impl SpanGpuKernel {
    pub(crate) fn new(device: wgpu::Device) -> Self {
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
            bind_group_layout,
            mean_shift_pipeline,
            conv2d_pipeline,
            gate_pipeline,
            concat_pipeline,
            pixel_shuffle_pipeline,
            dummy,
        }
    }

    pub(crate) fn create_workspace(
        &self,
        manifest: &SrLabManifest,
        input: &FeatureMap,
        include_readback_in_guard: bool,
    ) -> Result<SpanGpuWorkspace, String> {
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
            include_readback_in_guard,
        )?;
        validate_storage_buffer_sizes(
            input,
            feature_channels,
            output_channels,
            manifest.scale as usize,
            self.device.limits().max_storage_buffer_binding_size as u64,
        )?;

        let feature_buffer = |label| {
            empty_buffer(
                &self.device,
                label,
                feature_channels,
                input.height,
                input.width,
            )
        };

        Ok(SpanGpuWorkspace {
            input: empty_buffer(
                &self.device,
                "suisuiview-sr-lab-span-input",
                input.channels,
                input.height,
                input.width,
            ),
            shifted: empty_buffer(
                &self.device,
                "suisuiview-sr-lab-span-shifted",
                input.channels,
                input.height,
                input.width,
            ),
            out_feature: feature_buffer("suisuiview-sr-lab-span-out-feature"),
            current_a: feature_buffer("suisuiview-sr-lab-span-current-a"),
            current_b: feature_buffer("suisuiview-sr-lab-span-current-b"),
            out1: feature_buffer("suisuiview-sr-lab-span-out1"),
            out2: feature_buffer("suisuiview-sr-lab-span-out2"),
            out3: feature_buffer("suisuiview-sr-lab-span-out3"),
            out_b1: feature_buffer("suisuiview-sr-lab-span-out-b1"),
            out_b5_2: feature_buffer("suisuiview-sr-lab-span-out-b5-2"),
            out_b6: feature_buffer("suisuiview-sr-lab-span-out-b6"),
            joined: empty_buffer(
                &self.device,
                "suisuiview-sr-lab-span-joined",
                feature_channels * 4,
                input.height,
                input.width,
            ),
            cat: feature_buffer("suisuiview-sr-lab-span-cat"),
            up: empty_buffer(
                &self.device,
                "suisuiview-sr-lab-span-up",
                output_channels * manifest.scale as usize * manifest.scale as usize,
                input.height,
                input.width,
            ),
            output: empty_buffer(
                &self.device,
                "suisuiview-sr-lab-span-output",
                output_channels,
                input.height * manifest.scale as usize,
                input.width * manifest.scale as usize,
            ),
        })
    }

    pub(crate) fn encode_workspace(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        manifest: &SrLabManifest,
        model: &SpanGpuModel,
        workspace: &SpanGpuWorkspace,
    ) -> Result<(), String> {
        validate_span_model(
            self.device.limits().max_storage_buffer_binding_size as u64,
            manifest,
            model,
            workspace,
        )?;
        self.encode_prevalidated_workspace(encoder, manifest, model, workspace)
    }

    /// Encodes a workspace whose manifest, model, and buffer shapes were already validated.
    pub(crate) fn encode_prevalidated_workspace(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        manifest: &SrLabManifest,
        model: &SpanGpuModel,
        workspace: &SpanGpuWorkspace,
    ) -> Result<(), String> {
        let graph_plan = self.create_prevalidated_graph_plan(manifest, model, workspace)?;
        self.encode_graph_plan(encoder, &graph_plan);
        Ok(())
    }

    pub(crate) fn create_prevalidated_graph_plan(
        &self,
        manifest: &SrLabManifest,
        model: &SpanGpuModel,
        workspace: &SpanGpuWorkspace,
    ) -> Result<SpanGpuGraphPlan, String> {
        let span = manifest
            .span
            .as_ref()
            .ok_or_else(|| "SPAN GPU reference requires span metadata".to_owned())?;
        let feature_channels = span.feature_channels as usize;
        let mut steps = Vec::new();

        self.push_mean_shift(
            &mut steps,
            &workspace.input,
            &workspace.shifted,
            span.rgb_mean,
            span.img_range,
        );
        self.push_conv(
            &mut steps,
            model,
            &workspace.shifted,
            &workspace.out_feature,
            "conv_1",
            3,
            1,
            false,
        )?;
        push_copy(&mut steps, &workspace.out_feature, &workspace.current_a);

        let mut current_is_a = true;
        for block in 1..=span.block_count {
            let current = if current_is_a {
                &workspace.current_a
            } else {
                &workspace.current_b
            };
            let next = if current_is_a {
                &workspace.current_b
            } else {
                &workspace.current_a
            };
            self.push_conv(
                &mut steps,
                model,
                current,
                &workspace.out1,
                &format!("block_{block}.c1_r"),
                3,
                1,
                false,
            )?;
            self.push_conv(
                &mut steps,
                model,
                &workspace.out1,
                &workspace.out2,
                &format!("block_{block}.c2_r"),
                3,
                1,
                true,
            )?;
            self.push_conv(
                &mut steps,
                model,
                &workspace.out2,
                &workspace.out3,
                &format!("block_{block}.c3_r"),
                3,
                1,
                true,
            )?;
            self.push_gate(&mut steps, &workspace.out3, current, next);
            if block == 1 {
                push_copy(&mut steps, next, &workspace.out_b1);
            }
            if block == span.block_count {
                push_copy(&mut steps, &workspace.out1, &workspace.out_b5_2);
            }
            current_is_a = !current_is_a;
        }

        let current = if current_is_a {
            &workspace.current_a
        } else {
            &workspace.current_b
        };
        self.push_conv(
            &mut steps,
            model,
            current,
            &workspace.out_b6,
            "conv_2",
            3,
            1,
            false,
        )?;
        self.push_concat4(
            &mut steps,
            &workspace.out_feature,
            &workspace.out_b6,
            &workspace.out_b1,
            &workspace.out_b5_2,
            &workspace.joined,
            feature_channels,
        );
        self.push_conv(
            &mut steps,
            model,
            &workspace.joined,
            &workspace.cat,
            "conv_cat",
            1,
            0,
            false,
        )?;
        self.push_conv(
            &mut steps,
            model,
            &workspace.cat,
            &workspace.up,
            "upsampler.0",
            3,
            1,
            false,
        )?;
        self.push_pixel_shuffle(&mut steps, &workspace.up, &workspace.output, manifest.scale);

        Ok(SpanGpuGraphPlan { steps })
    }

    pub(crate) fn encode_graph_plan(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        graph_plan: &SpanGpuGraphPlan,
    ) {
        self.encode_graph_plan_with_hooks(encoder, graph_plan, |_| {}, |_| {});
    }

    pub(crate) fn encode_graph_plan_with_hooks(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        graph_plan: &SpanGpuGraphPlan,
        mut before_first_dispatch_run: impl FnMut(&mut wgpu::ComputePass<'_>),
        mut after_last_dispatch_run: impl FnMut(&mut wgpu::ComputePass<'_>),
    ) {
        let steps = graph_plan.steps.as_slice();
        let dispatch_runs = dispatch_run_count(steps);
        let mut dispatch_run_index = 0;
        let mut step_index = 0;
        while step_index < steps.len() {
            match &steps[step_index] {
                SpanGpuGraphStep::Dispatch(_) => {
                    let run_start = step_index;
                    while matches!(steps.get(step_index), Some(SpanGpuGraphStep::Dispatch(_))) {
                        step_index += 1;
                    }
                    let is_first_run = dispatch_run_index == 0;
                    dispatch_run_index += 1;
                    let is_last_run = dispatch_run_index == dispatch_runs;
                    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                        label: Some("suisuiview-sr-lab-span-pass"),
                        timestamp_writes: None,
                    });
                    if is_first_run {
                        before_first_dispatch_run(&mut pass);
                    }
                    self.dispatch_prebuilt_run(&mut pass, &steps[run_start..step_index]);
                    if is_last_run {
                        after_last_dispatch_run(&mut pass);
                    }
                }
                SpanGpuGraphStep::Copy(copy) => {
                    encoder.copy_buffer_to_buffer(
                        &copy.source,
                        0,
                        &copy.destination,
                        0,
                        copy.byte_len,
                    );
                    step_index += 1;
                }
            }
        }
    }

    #[allow(dead_code)]
    pub(crate) fn workspace_byte_size(
        &self,
        manifest: &SrLabManifest,
        input: &FeatureMap,
        include_readback: bool,
    ) -> Result<u64, String> {
        validate_span_manifest(manifest, input)?;
        let span = manifest
            .span
            .as_ref()
            .ok_or_else(|| "SPAN GPU reference requires span metadata".to_owned())?;
        span_transient_byte_size(
            input,
            span.feature_channels as usize,
            manifest.output_channels as usize,
            manifest.scale as usize,
            include_readback,
        )
    }

    fn push_mean_shift(
        &self,
        steps: &mut Vec<SpanGpuGraphStep>,
        input: &GpuBuffer,
        output: &GpuBuffer,
        rgb_mean: [f32; 3],
        img_range: f32,
    ) {
        let mut mean = [0.0; 4];
        mean[..3].copy_from_slice(&rgb_mean);
        let mut params = params_for(input, output);
        params.rgb_mean = mean;
        params.img_range = img_range;
        self.push_dispatch(
            steps,
            SpanPipeline::MeanShift,
            params,
            [input, &self.dummy, &self.dummy, &self.dummy],
            &self.dummy.buffer,
            &self.dummy.buffer,
            output,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn push_conv(
        &self,
        steps: &mut Vec<SpanGpuGraphStep>,
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
        let mut params = params_for(input, output);
        params.kernel = kernel;
        params.padding = padding;
        params.activation = u32::from(activate_input);
        self.push_dispatch(
            steps,
            SpanPipeline::Conv2d,
            params,
            [input, &self.dummy, &self.dummy, &self.dummy],
            &weight.buffer,
            &bias.buffer,
            output,
        );
        Ok(())
    }

    fn push_gate(
        &self,
        steps: &mut Vec<SpanGpuGraphStep>,
        out3: &GpuBuffer,
        current: &GpuBuffer,
        output: &GpuBuffer,
    ) {
        let params = params_for(out3, output);
        self.push_dispatch(
            steps,
            SpanPipeline::Gate,
            params,
            [out3, current, &self.dummy, &self.dummy],
            &self.dummy.buffer,
            &self.dummy.buffer,
            output,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn push_concat4(
        &self,
        steps: &mut Vec<SpanGpuGraphStep>,
        a: &GpuBuffer,
        b: &GpuBuffer,
        c: &GpuBuffer,
        d: &GpuBuffer,
        output: &GpuBuffer,
        feature_channels: usize,
    ) {
        let mut params = params_for(a, output);
        params.input_channels = feature_channels as u32;
        self.push_dispatch(
            steps,
            SpanPipeline::Concat4,
            params,
            [a, b, c, d],
            &self.dummy.buffer,
            &self.dummy.buffer,
            output,
        );
    }

    fn push_pixel_shuffle(
        &self,
        steps: &mut Vec<SpanGpuGraphStep>,
        input: &GpuBuffer,
        output: &GpuBuffer,
        scale: u32,
    ) {
        let mut params = params_for(input, output);
        params.scale = scale;
        self.push_dispatch(
            steps,
            SpanPipeline::PixelShuffle,
            params,
            [input, &self.dummy, &self.dummy, &self.dummy],
            &self.dummy.buffer,
            &self.dummy.buffer,
            output,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn push_dispatch(
        &self,
        steps: &mut Vec<SpanGpuGraphStep>,
        pipeline: SpanPipeline,
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
        steps.push(SpanGpuGraphStep::Dispatch(SpanGpuDispatchStep {
            pipeline,
            bind_group,
            _params_buffer: params_buffer,
            workgroups: [
                (output.width as u32).div_ceil(8),
                (output.height as u32).div_ceil(8),
                output.channels as u32,
            ],
        }));
    }

    fn dispatch_prebuilt_run(&self, pass: &mut wgpu::ComputePass<'_>, steps: &[SpanGpuGraphStep]) {
        let mut active_pipeline = None;
        for step in steps {
            let SpanGpuGraphStep::Dispatch(dispatch) = step else {
                debug_assert!(false, "dispatch run contained a non-dispatch step");
                continue;
            };
            if active_pipeline != Some(dispatch.pipeline) {
                pass.set_pipeline(self.pipeline(dispatch.pipeline));
                active_pipeline = Some(dispatch.pipeline);
            }
            pass.set_bind_group(0, &dispatch.bind_group, &[]);
            pass.dispatch_workgroups(
                dispatch.workgroups[0],
                dispatch.workgroups[1],
                dispatch.workgroups[2],
            );
        }
    }

    fn pipeline(&self, pipeline: SpanPipeline) -> &wgpu::ComputePipeline {
        match pipeline {
            SpanPipeline::MeanShift => &self.mean_shift_pipeline,
            SpanPipeline::Conv2d => &self.conv2d_pipeline,
            SpanPipeline::Gate => &self.gate_pipeline,
            SpanPipeline::Concat4 => &self.concat_pipeline,
            SpanPipeline::PixelShuffle => &self.pixel_shuffle_pipeline,
        }
    }
}

fn push_copy(steps: &mut Vec<SpanGpuGraphStep>, source: &GpuBuffer, destination: &GpuBuffer) {
    steps.push(SpanGpuGraphStep::Copy(SpanGpuCopyStep {
        source: source.buffer.clone(),
        destination: destination.buffer.clone(),
        byte_len: destination.byte_len(),
    }));
}

fn dispatch_run_count(steps: &[SpanGpuGraphStep]) -> usize {
    let mut count = 0;
    let mut previous_was_dispatch = false;
    for step in steps {
        let is_dispatch = matches!(step, SpanGpuGraphStep::Dispatch(_));
        if is_dispatch && !previous_was_dispatch {
            count += 1;
        }
        previous_was_dispatch = is_dispatch;
    }
    count
}

#[allow(dead_code)]
impl SpanGpuWorkspace {
    pub(crate) fn input_buffer(&self) -> &wgpu::Buffer {
        &self.input.buffer
    }

    pub(crate) fn output_buffer(&self) -> &wgpu::Buffer {
        &self.output.buffer
    }

    pub(crate) fn output_size(&self) -> [usize; 2] {
        [self.output.width, self.output.height]
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

fn params_for(input: &GpuBuffer, output: &GpuBuffer) -> SpanParams {
    SpanParams {
        width: input.width as u32,
        height: input.height as u32,
        input_channels: input.channels as u32,
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
    }
}
