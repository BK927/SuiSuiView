use std::time::Instant;

use egui_wgpu::wgpu;

use super::elapsed_ms;

pub(super) struct PrewarmedWgpu {
    pub(super) instance: wgpu::Instance,
    pub(super) adapter: wgpu::Adapter,
    pub(super) device: wgpu::Device,
    pub(super) queue: wgpu::Queue,
}

pub(super) struct PrewarmReport {
    pub(super) ready_ms: f64,
    pub(super) init_ms: f64,
    pub(super) adapter_name: Option<String>,
    pub(super) backend: Option<String>,
    pub(super) device_type: Option<String>,
    pub(super) result: Result<PrewarmedWgpu, String>,
}

pub(super) fn run_wgpu_prewarm(started_at: Instant) -> PrewarmReport {
    pollster::block_on(run_wgpu_prewarm_async(started_at))
}

async fn run_wgpu_prewarm_async(started_at: Instant) -> PrewarmReport {
    let init_started = Instant::now();
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::from_env().unwrap_or(wgpu::Backends::PRIMARY),
        flags: wgpu::InstanceFlags::empty().with_env(),
        backend_options: wgpu::BackendOptions::from_env_or_default(),
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
            return PrewarmReport {
                ready_ms: elapsed_ms(started_at.elapsed()),
                init_ms: elapsed_ms(init_started.elapsed()),
                adapter_name: None,
                backend: None,
                device_type: None,
                result: Err(format!("request_adapter failed: {error}")),
            };
        }
    };

    let info = adapter.get_info();
    let device_queue = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("suisuiview-app-handoff-prewarm-device"),
            required_features: wgpu::Features::default(),
            required_limits: wgpu::Limits {
                max_texture_dimension_2d: 8192,
                ..wgpu::Limits::default()
            },
            memory_hints: wgpu::MemoryHints::default(),
            trace: wgpu::Trace::Off,
        })
        .await;
    let (device, queue) = match device_queue {
        Ok(value) => value,
        Err(error) => {
            return PrewarmReport {
                ready_ms: elapsed_ms(started_at.elapsed()),
                init_ms: elapsed_ms(init_started.elapsed()),
                adapter_name: Some(info.name),
                backend: Some(info.backend.to_str().to_owned()),
                device_type: Some(device_type_label(info.device_type).to_owned()),
                result: Err(format!("request_device failed: {error}")),
            };
        }
    };

    PrewarmReport {
        ready_ms: elapsed_ms(started_at.elapsed()),
        init_ms: elapsed_ms(init_started.elapsed()),
        adapter_name: Some(info.name),
        backend: Some(info.backend.to_str().to_owned()),
        device_type: Some(device_type_label(info.device_type).to_owned()),
        result: Ok(PrewarmedWgpu {
            instance,
            adapter,
            device,
            queue,
        }),
    }
}

fn device_type_label(device_type: wgpu::DeviceType) -> &'static str {
    match device_type {
        wgpu::DeviceType::Other => "other",
        wgpu::DeviceType::IntegratedGpu => "integrated",
        wgpu::DeviceType::DiscreteGpu => "discrete",
        wgpu::DeviceType::VirtualGpu => "virtual",
        wgpu::DeviceType::Cpu => "cpu",
    }
}
