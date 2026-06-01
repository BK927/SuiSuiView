use super::buffers::{GpuBuffer, SpanGpuModel};
use super::kernel::{SpanGpuKernel, SpanGpuWorkspace};
use crate::core::sr_lab::cpu::FeatureMap;
use crate::core::sr_lab::{blob::SrLabWeights, SrLabManifest};
use std::sync::mpsc;
use std::time::Instant;

pub(super) struct SpanGpuExecutor {
    device: wgpu::Device,
    queue: wgpu::Queue,
    kernel: SpanGpuKernel,
}

pub(super) struct RunStats {
    pub(super) output: FeatureMap,
    pub(super) elapsed_ms: f64,
}

pub(super) struct SessionRunStats {
    pub(super) elapsed_ms: f64,
}

pub(super) struct SpanGpuSession<'a> {
    executor: &'a SpanGpuExecutor,
    manifest: &'a SrLabManifest,
    model: &'a SpanGpuModel,
    workspace: SpanGpuWorkspace,
}

pub(super) struct SpanGpuReadbackSession<'a> {
    executor: &'a SpanGpuExecutor,
    manifest: &'a SrLabManifest,
    model: &'a SpanGpuModel,
    workspace: SpanGpuWorkspace,
    readback: wgpu::Buffer,
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
        let kernel = SpanGpuKernel::new(device.clone());
        Self {
            device,
            queue,
            kernel,
        }
    }

    pub(super) fn upload_model(&self, weights: &SrLabWeights) -> SpanGpuModel {
        SpanGpuModel::from_weights(&self.device, weights)
    }

    pub(super) fn run(
        &self,
        manifest: &SrLabManifest,
        model: &SpanGpuModel,
        input: &FeatureMap,
    ) -> Result<RunStats, String> {
        self.with_validation_scope(|| {
            let workspace = self.create_workspace(manifest, input, true)?;
            self.write_input(&workspace, input);
            self.run_workspace_with_readback(manifest, model, &workspace)
        })
    }

    pub(super) fn create_session<'a>(
        &'a self,
        manifest: &'a SrLabManifest,
        model: &'a SpanGpuModel,
        input: &FeatureMap,
    ) -> Result<SpanGpuSession<'a>, String> {
        self.with_validation_scope(|| {
            let workspace = self.create_workspace(manifest, input, false)?;
            self.write_input(&workspace, input);
            Ok(SpanGpuSession {
                executor: self,
                manifest,
                model,
                workspace,
            })
        })
    }

    pub(super) fn create_readback_session<'a>(
        &'a self,
        manifest: &'a SrLabManifest,
        model: &'a SpanGpuModel,
        input: &FeatureMap,
    ) -> Result<SpanGpuReadbackSession<'a>, String> {
        self.with_validation_scope(|| {
            let workspace = self.create_workspace(manifest, input, true)?;
            let readback = self.create_readback_buffer(&workspace.output);
            Ok(SpanGpuReadbackSession {
                executor: self,
                manifest,
                model,
                workspace,
                readback,
            })
        })
    }

    fn create_workspace(
        &self,
        manifest: &SrLabManifest,
        input: &FeatureMap,
        include_readback_in_guard: bool,
    ) -> Result<SpanGpuWorkspace, String> {
        self.kernel
            .create_workspace(manifest, input, include_readback_in_guard)
    }

    fn write_input(&self, workspace: &SpanGpuWorkspace, input: &FeatureMap) {
        self.queue.write_buffer(
            &workspace.input.buffer,
            0,
            bytemuck::cast_slice(&input.values),
        );
    }

    fn run_workspace_with_readback(
        &self,
        manifest: &SrLabManifest,
        model: &SpanGpuModel,
        workspace: &SpanGpuWorkspace,
    ) -> Result<RunStats, String> {
        let readback = self.create_readback_buffer(&workspace.output);
        self.run_workspace_with_readback_buffer(manifest, model, workspace, &readback)
    }

    fn create_readback_buffer(&self, output: &GpuBuffer) -> wgpu::Buffer {
        self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("suisuiview-sr-lab-span-readback"),
            size: output.byte_len(),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        })
    }

    fn run_workspace_with_readback_buffer(
        &self,
        manifest: &SrLabManifest,
        model: &SpanGpuModel,
        workspace: &SpanGpuWorkspace,
        readback: &wgpu::Buffer,
    ) -> Result<RunStats, String> {
        let started = Instant::now();
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("suisuiview-sr-lab-span-encoder"),
            });

        self.encode_workspace(&mut encoder, manifest, model, workspace)?;
        let output = &workspace.output;
        encoder.copy_buffer_to_buffer(&output.buffer, 0, readback, 0, output.byte_len());
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

    fn run_workspace_no_readback(
        &self,
        manifest: &SrLabManifest,
        model: &SpanGpuModel,
        workspace: &SpanGpuWorkspace,
    ) -> Result<SessionRunStats, String> {
        let started = Instant::now();
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("suisuiview-sr-lab-span-session-encoder"),
            });

        self.encode_workspace(&mut encoder, manifest, model, workspace)?;
        self.queue.submit(Some(encoder.finish()));
        self.device
            .poll(wgpu::PollType::Wait)
            .map_err(|error| format!("wgpu poll failed: {error}"))?;

        Ok(SessionRunStats {
            elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
        })
    }

    fn encode_workspace(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        manifest: &SrLabManifest,
        model: &SpanGpuModel,
        workspace: &SpanGpuWorkspace,
    ) -> Result<(), String> {
        self.kernel
            .encode_workspace(encoder, manifest, model, workspace)
    }

    fn with_validation_scope<T>(
        &self,
        action: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let result = action();
        if let Some(error) = pollster::block_on(self.device.pop_error_scope()) {
            return Err(format!("SPAN GPU executor validation failed: {error}"));
        }
        result
    }
}

impl SpanGpuSession<'_> {
    pub(super) fn run(&self) -> Result<SessionRunStats, String> {
        self.executor.with_validation_scope(|| {
            self.executor
                .run_workspace_no_readback(self.manifest, self.model, &self.workspace)
        })
    }

    pub(super) fn output_width(&self) -> usize {
        self.workspace.output.width
    }

    pub(super) fn output_height(&self) -> usize {
        self.workspace.output.height
    }
}

impl SpanGpuReadbackSession<'_> {
    pub(super) fn run(&self, input: &FeatureMap) -> Result<RunStats, String> {
        if input.channels != self.workspace.input.channels
            || input.height != self.workspace.input.height
            || input.width != self.workspace.input.width
        {
            return Err(format!(
                "SPAN GPU readback session input shape changed: session {}x{}x{}, input {}x{}x{}",
                self.workspace.input.channels,
                self.workspace.input.width,
                self.workspace.input.height,
                input.channels,
                input.width,
                input.height
            ));
        }
        self.executor.with_validation_scope(|| {
            self.executor.write_input(&self.workspace, input);
            self.executor.run_workspace_with_readback_buffer(
                self.manifest,
                self.model,
                &self.workspace,
                &self.readback,
            )
        })
    }
}
