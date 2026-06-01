use super::buffers::{
    buffer_from_values, empty_buffer, storage_binding, storage_read_entry,
    storage_read_write_entry, GpuBuffer, SpanGpuModel,
};
use super::validation::{validate_conv_shape, validate_span_manifest, validate_transient_size};
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

pub(super) struct SpanGpuKernel {
    device: wgpu::Device,
    bind_group_layout: wgpu::BindGroupLayout,
    mean_shift_pipeline: wgpu::ComputePipeline,
    conv2d_pipeline: wgpu::ComputePipeline,
    gate_pipeline: wgpu::ComputePipeline,
    concat_pipeline: wgpu::ComputePipeline,
    pixel_shuffle_pipeline: wgpu::ComputePipeline,
    dummy: GpuBuffer,
}

pub(super) struct SpanGpuWorkspace {
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

impl SpanGpuKernel {
    pub(super) fn new(device: wgpu::Device) -> Self {
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

    pub(super) fn create_workspace(
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

    pub(super) fn encode_workspace(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        manifest: &SrLabManifest,
        model: &SpanGpuModel,
        workspace: &SpanGpuWorkspace,
    ) -> Result<(), String> {
        let span = manifest
            .span
            .as_ref()
            .ok_or_else(|| "SPAN GPU reference requires span metadata".to_owned())?;
        let feature_channels = span.feature_channels as usize;

        self.run_mean_shift(
            encoder,
            &workspace.input,
            &workspace.shifted,
            span.rgb_mean,
            span.img_range,
        );
        self.run_conv(
            encoder,
            model,
            &workspace.shifted,
            &workspace.out_feature,
            "conv_1",
            3,
            1,
            false,
        )?;
        encoder.copy_buffer_to_buffer(
            &workspace.out_feature.buffer,
            0,
            &workspace.current_a.buffer,
            0,
            workspace.current_a.byte_len(),
        );

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
            self.run_conv(
                encoder,
                model,
                current,
                &workspace.out1,
                &format!("block_{block}.c1_r"),
                3,
                1,
                false,
            )?;
            self.run_conv(
                encoder,
                model,
                &workspace.out1,
                &workspace.out2,
                &format!("block_{block}.c2_r"),
                3,
                1,
                true,
            )?;
            self.run_conv(
                encoder,
                model,
                &workspace.out2,
                &workspace.out3,
                &format!("block_{block}.c3_r"),
                3,
                1,
                true,
            )?;
            self.run_gate(encoder, &workspace.out3, current, next);
            if block == 1 {
                encoder.copy_buffer_to_buffer(
                    &next.buffer,
                    0,
                    &workspace.out_b1.buffer,
                    0,
                    workspace.out_b1.byte_len(),
                );
            }
            if block == span.block_count {
                encoder.copy_buffer_to_buffer(
                    &workspace.out1.buffer,
                    0,
                    &workspace.out_b5_2.buffer,
                    0,
                    workspace.out_b5_2.byte_len(),
                );
            }
            current_is_a = !current_is_a;
        }

        let current = if current_is_a {
            &workspace.current_a
        } else {
            &workspace.current_b
        };
        self.run_conv(
            encoder,
            model,
            current,
            &workspace.out_b6,
            "conv_2",
            3,
            1,
            false,
        )?;
        self.run_concat4(
            encoder,
            &workspace.out_feature,
            &workspace.out_b6,
            &workspace.out_b1,
            &workspace.out_b5_2,
            &workspace.joined,
            feature_channels,
        );
        self.run_conv(
            encoder,
            model,
            &workspace.joined,
            &workspace.cat,
            "conv_cat",
            1,
            0,
            false,
        )?;
        self.run_conv(
            encoder,
            model,
            &workspace.cat,
            &workspace.up,
            "upsampler.0",
            3,
            1,
            false,
        )?;
        self.run_pixel_shuffle(encoder, &workspace.up, &workspace.output, manifest.scale);

        Ok(())
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
        let mut params = params_for(input, output);
        params.rgb_mean = mean;
        params.img_range = img_range;
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

    #[allow(clippy::too_many_arguments)]
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
        let mut params = params_for(input, output);
        params.kernel = kernel;
        params.padding = padding;
        params.activation = u32::from(activate_input);
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
        let params = params_for(out3, output);
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

    #[allow(clippy::too_many_arguments)]
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
        let mut params = params_for(a, output);
        params.input_channels = feature_channels as u32;
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
        let mut params = params_for(input, output);
        params.scale = scale;
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
