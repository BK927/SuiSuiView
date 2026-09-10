//! Opt-in, generated-pixel equivalence checks. No viewer, filesystem image,
//! personal profile or benchmark timing is involved.
use super::*;
use std::borrow::Cow;

#[test]
#[ignore = "requires a local GPU; worker cancellation and complete active-set test"]
fn background_worker_preserves_active_set_and_obeys_fences() {
    let (device, queue) = device();
    let size = [32, 32];
    let (texture, view) = source(&device, &queue, size);
    device.poll(wgpu::PollType::Wait).unwrap();
    let ready = Arc::new(AtomicBool::new(false));
    let request = |key, method| RefineRequest {
        key,
        source_key: 100,
        method,
        _source_texture: texture.clone(),
        source_view: view.clone(),
        source_size: size,
        source_byte_size: size[0] * size[1] * 4,
        stack_passes: 1,
        source_ready: ready.clone(),
    };
    let worker = BackgroundRefiner::new(device, queue, egui::Context::default());
    let publish = |generation, requests: Vec<RefineRequest>, quiet_until| {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if worker.update(RefineSnapshot {
                generation,
                requests: requests.clone(),
                quiet_until,
                budget_bytes: 64 * 1024 * 1024,
            }) {
                break;
            }
            assert!(Instant::now() < deadline, "snapshot retry");
            std::thread::yield_now();
        }
    };
    publish(
        1,
        vec![request(1, WgpuUpscaleMethod::WgslAnime4kV32CnnX2S)],
        Instant::now(),
    );
    std::thread::sleep(Duration::from_millis(30));
    assert!(
        worker.drain().is_empty(),
        "source fence must hold submission"
    );
    ready.store(true, Ordering::Release);
    let quiet = Instant::now() + INPUT_QUIET;
    publish(
        2,
        vec![request(2, WgpuUpscaleMethod::WgslAnime4kV32CnnX2S)],
        quiet,
    );
    std::thread::sleep(Duration::from_millis(30));
    assert!(
        worker.drain().is_empty(),
        "input quiet must hold submission"
    );
    // A new complete set cancels both old generations and must preserve both
    // methods, regardless of which pane was painted last.
    publish(
        3,
        vec![
            request(3, WgpuUpscaleMethod::WgslAnime4kV32CnnX2S),
            request(4, WgpuUpscaleMethod::WgslAnime4kV32CnnX2M),
        ],
        quiet,
    );
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut outputs = HashMap::new();
    while outputs.len() != 2 {
        assert!(
            worker.failure_reason().is_none(),
            "worker failed: {:?}",
            worker.failure_reason()
        );
        for event in worker.drain() {
            match event {
                RefineEvent::Ready {
                    generation,
                    key,
                    output,
                } => {
                    assert_eq!(generation, 3);
                    assert!([3, 4].contains(&key));
                    assert_eq!(output.size, [64, 64]);
                    outputs.insert(key, output);
                }
                RefineEvent::Skipped { reason, .. } => panic!("unexpected skip: {reason}"),
            }
        }
        publish(
            3,
            vec![
                request(3, WgpuUpscaleMethod::WgslAnime4kV32CnnX2S),
                request(4, WgpuUpscaleMethod::WgslAnime4kV32CnnX2M),
            ],
            quiet,
        );
        assert!(
            Instant::now() < deadline,
            "both active requests must finish"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
#[ignore = "requires a local GPU; generated image, no app/profile"]
fn background_rows_match_synchronous_manual_models() {
    let (device, queue) = device();
    // Odd dimensions exercise both partial workgroups and more than one row
    // batch for source/final layers, including edge clamping and alpha.
    let size = [97, 193];
    let (source, view) = source(&device, &queue, size);
    let mut realtime = super::super::RealtimeSrResources::new();
    for method in [
        WgpuUpscaleMethod::WgslAnime4kV32CnnX2S,
        WgpuUpscaleMethod::WgslAnime4kV32CnnX2M,
        WgpuUpscaleMethod::CunnyVeryfastNvl,
        WgpuUpscaleMethod::WgslAcnetF8B4Luma,
    ] {
        let mut encoder = device.create_command_encoder(&Default::default());
        let reference = realtime
            .render(method, 1, &device, &mut encoder, &view, size)
            .expect("synchronous method");
        queue.submit(Some(encoder.finish()));
        device.poll(wgpu::PollType::Wait).unwrap();
        let program = programs::Program::new(&device, method).unwrap();
        let mut job = program.graph(&device, &view, size);
        let mut submissions = 0;
        loop {
            let mut encoder = device.create_command_encoder(&Default::default());
            let done = job.encode_next(&queue, &mut encoder);
            queue.submit(Some(encoder.finish()));
            device.poll(wgpu::PollType::Wait).unwrap();
            submissions += 1;
            if done {
                break;
            }
        }
        assert!(
            submissions > job.layers.len(),
            "{} must exercise row splitting",
            method.token()
        );
        let expected = read(&device, &queue, &reference.view, reference.size);
        let actual = read(&device, &queue, &job.output.view, job.output.size);
        let max_diff = expected
            .iter()
            .zip(&actual)
            .map(|(&a, &b)| a.abs_diff(b))
            .max()
            .unwrap();
        assert!(
            max_diff <= 1,
            "{} maximum byte difference {max_diff}",
            method.token()
        );
    }
    drop(source);
}

#[test]
#[ignore = "requires GPU and explicitly configured local SPAN manifest; no app/profile"]
fn background_span_steps_match_existing_halo_tiles() {
    if std::env::var_os("SUISUIVIEW_SR_LAB_SPAN_MANIFEST").is_none()
        && std::env::var_os("SUISUIVIEW_EXPERIMENT_SPAN_MANIFEST").is_none()
    {
        eprintln!("SPAN equivalence skipped: no local manifest configured");
        return;
    }
    let (device, queue) = device();
    let size = [97, 129];
    let (_source, view) = source(&device, &queue, size);
    let mut realtime = super::super::RealtimeSrResources::new();
    let deadline = Instant::now() + Duration::from_secs(120);
    let reference = loop {
        let mut encoder = device.create_command_encoder(&Default::default());
        let output = realtime.render(
            WgpuUpscaleMethod::WgslSrLabSpanX2,
            10,
            &device,
            &mut encoder,
            &view,
            size,
        );
        queue.submit(Some(encoder.finish()));
        device.poll(wgpu::PollType::Wait).unwrap();
        if let Some(output) = output {
            break output;
        }
        assert!(
            Instant::now() < deadline,
            "SPAN reference failed to become ready"
        );
        std::thread::sleep(Duration::from_millis(5));
    };
    let mut graph = BackgroundSpanJob::new(&device, &view, size, 192 * 1024 * 1024).unwrap();
    loop {
        let mut encoder = device.create_command_encoder(&Default::default());
        let done = graph.encode_next(&device, &mut encoder);
        queue.submit(Some(encoder.finish()));
        device.poll(wgpu::PollType::Wait).unwrap();
        if done {
            break;
        }
    }
    let expected = read(&device, &queue, &reference.view, reference.size);
    let actual = read(&device, &queue, &graph.output.view, graph.output.size);
    let max_diff = expected
        .iter()
        .zip(actual)
        .map(|(&a, b)| a.abs_diff(b))
        .max()
        .unwrap();
    assert!(max_diff <= 1, "SPAN maximum byte difference {max_diff}");
}

#[test]
#[ignore = "requires GPU, local SPAN manifest and SPAN tiles-per-frame=1"]
fn normal_span_keeps_both_spread_requests_until_complete() {
    if std::env::var_os("SUISUIVIEW_SR_LAB_SPAN_MANIFEST").is_none()
        || super::super::span_display::span_display_tiles_per_frame() != 1
    {
        eprintln!("normal SPAN active-set test requires explicit manifest and one tile per frame");
        return;
    }
    let (device, queue) = device();
    let size = [97, 129];
    let (_texture, view) = source(&device, &queue, size);
    let mut realtime = super::super::RealtimeSrResources::new();
    let method = WgpuUpscaleMethod::WgslSrLabSpanX2;
    let mut outputs = HashMap::new();
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut frame = 1;
    while outputs.len() < 2 {
        realtime.begin_active_frame(frame);
        let mut encoder = device.create_command_encoder(&Default::default());
        for key in [10, 20] {
            if !outputs.contains_key(&key) {
                if let Some(output) =
                    realtime.render(method, key, &device, &mut encoder, &view, size)
                {
                    outputs.insert(key, output);
                }
            }
        }
        realtime.finish_active_frame(frame);
        queue.submit(Some(encoder.finish()));
        device.poll(wgpu::PollType::Wait).unwrap();
        assert!(
            Instant::now() < deadline,
            "a later pane must not cancel earlier tile progress"
        );
        std::thread::sleep(Duration::from_millis(2));
        frame += 1;
    }
    let first = read(&device, &queue, &outputs[&10].view, outputs[&10].size);
    let second = read(&device, &queue, &outputs[&20].view, outputs[&20].size);
    assert_eq!(first, second);
    // Establish new partial work and then remove every SPAN request next frame.
    realtime.begin_active_frame(frame);
    let mut encoder = device.create_command_encoder(&Default::default());
    assert!(realtime
        .render(method, 30, &device, &mut encoder, &view, size)
        .is_none());
    realtime.finish_active_frame(frame);
    queue.submit(Some(encoder.finish()));
    device.poll(wgpu::PollType::Wait).unwrap();
    assert!(realtime.has_pending_async_work(method));
    realtime.begin_active_frame(frame + 1);
    assert_eq!(realtime.finish_active_frame(frame + 1), Some(false));
    assert!(!realtime.has_pending_async_work(method));
}

fn device() -> (wgpu::Device, wgpu::Queue) {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::VULKAN | wgpu::Backends::GL,
        ..Default::default()
    });
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        force_fallback_adapter: false,
        compatible_surface: None,
    }))
    .expect("local WGPU adapter");
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("background-refine-equivalence"),
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::default(),
        memory_hints: wgpu::MemoryHints::MemoryUsage,
        trace: wgpu::Trace::Off,
    }))
    .unwrap()
}

fn source(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    size: [usize; 2],
) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: size[0] as u32,
            height: size[1] as u32,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let mut pixels = Vec::with_capacity(size[0] * size[1] * 4);
    for y in 0..size[1] {
        for x in 0..size[0] {
            pixels.extend_from_slice(&[
                (x * 7 % 256) as u8,
                (y * 5 % 256) as u8,
                ((x ^ y) * 3 % 256) as u8,
                ((x + y) % 256) as u8,
            ]);
        }
    }
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &pixels,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(size[0] as u32 * 4),
            rows_per_image: Some(size[1] as u32),
        },
        texture.size(),
    );
    let view = texture.create_view(&Default::default());
    (texture, view)
}

fn read(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    view: &wgpu::TextureView,
    size: [usize; 2],
) -> Vec<u8> {
    // Sample-copy first: synchronous outputs deliberately lack COPY_SRC usage.
    let texture = graph::texture(device, size, wgpu::TextureFormat::Rgba8Unorm);
    let target = texture.create_view(&Default::default());
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: None, source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(r#"
@group(0) @binding(0) var image: texture_2d<f32>;
@vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
 let p = array<vec2<f32>, 3>(vec2(-1.0,-1.0),vec2(3.0,-1.0),vec2(-1.0,3.0)); return vec4(p[i],0.0,1.0);
}
@fragment fn fs(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> { return textureLoad(image,vec2<i32>(p.xy),0); }
"#)) });
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &[graph::sampled(0, false)],
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[&layout],
        push_constant_ranges: &[],
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: None,
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs"),
            targets: &[Some(wgpu::ColorTargetState {
                format: wgpu::TextureFormat::Rgba8Unorm,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        multiview: None,
        cache: None,
    });
    let bindings = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &layout,
        entries: &[graph::binding(0, view)],
    });
    let stride = (size[0] as u32 * 4).div_ceil(256) * 256;
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: stride as u64 * size[1] as u64,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bindings, &[]);
        pass.draw(0..3, 0..1);
    }
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(stride),
                rows_per_image: Some(size[1] as u32),
            },
        },
        texture.size(),
    );
    queue.submit(Some(encoder.finish()));
    let (tx, rx) = bounded(1);
    readback
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
    device.poll(wgpu::PollType::Wait).unwrap();
    rx.recv().unwrap().unwrap();
    let bytes = readback.slice(..).get_mapped_range();
    let mut rgba = Vec::with_capacity(size[0] * size[1] * 4);
    for row in bytes.chunks_exact(stride as usize) {
        rgba.extend_from_slice(&row[..size[0] * 4]);
    }
    drop(bytes);
    readback.unmap();
    rgba
}
