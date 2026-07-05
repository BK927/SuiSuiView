use super::*;
use crate::core::source::PageId;
use crate::core::worker::DecodeOptions;
use egui::{pos2, vec2};

#[test]
fn draw_id_separates_same_page_in_different_panes() {
    let source_key = GpuPaintSourceKey {
        book: 7,
        page: PageCacheKey {
            page_id: PageId(3),
            target_long_edge: 2048,
            decode: DecodeOptions::default(),
        },
    };
    let left = Rect::from_min_size(pos2(0.0, 0.0), vec2(640.0, 900.0));
    let right = Rect::from_min_size(pos2(640.0, 0.0), vec2(640.0, 900.0));

    assert_ne!(
        draw_id(
            source_key,
            ViewEffects::default(),
            WgpuUpscaleMethod::WgslFsr1Style,
            WgpuDownscaleMethod::Bilinear,
            left,
            1.0,
        ),
        draw_id(
            source_key,
            ViewEffects::default(),
            WgpuUpscaleMethod::WgslFsr1Style,
            WgpuDownscaleMethod::Bilinear,
            right,
            1.0,
        )
    );
}

#[test]
fn viewport_rect_keeps_full_target_for_oversized_clipped_rect() {
    let screen = ScreenDescriptor {
        size_in_pixels: [800, 600],
        pixels_per_point: 1.0,
    };
    let rect = Rect::from_min_size(pos2(-200.0, -100.0), vec2(1000.0, 800.0));

    assert_eq!(
        viewport_rect(rect, &screen),
        GpuDisplayRect {
            origin: [0, 0],
            visible_size: [800, 600],
            sample_offset: [200, 100],
            full_size: [1000, 800],
        }
    );
}

#[test]
fn viewport_rect_converts_points_to_physical_pixels() {
    let screen = ScreenDescriptor {
        size_in_pixels: [800, 600],
        pixels_per_point: 1.5,
    };
    let rect = Rect::from_min_size(pos2(10.0, 20.0), vec2(200.0, 100.0));

    assert_eq!(
        viewport_rect(rect, &screen),
        GpuDisplayRect {
            origin: [15, 30],
            visible_size: [300, 150],
            sample_offset: [0, 0],
            full_size: [300, 150],
        }
    );
}

#[test]
fn manual_and_original_modes_disable_display_upscalers() {
    assert!(!fit_mode_allows_display_upscale(FitMode::Manual));
    assert!(!fit_mode_allows_display_upscale(FitMode::Original));
    assert!(fit_mode_allows_display_upscale(FitMode::FitPage));
    assert!(fit_mode_allows_display_upscale(FitMode::FitWidth));
    assert!(fit_mode_allows_display_upscale(FitMode::FitHeight));
}

#[test]
fn experimental_wgpu_upscale_method_parses_hidden_artcnn() {
    assert_eq!(
        parse_experimental_wgpu_upscale_method(Some(" artcnn_c4f16 "), false, false),
        Some(WgpuUpscaleMethod::WgslArtcnnC4F16)
    );
    assert_eq!(
        parse_experimental_wgpu_upscale_method(Some("artcnn_c4f32_ds"), false, false),
        Some(WgpuUpscaleMethod::WgslArtcnnC4F32Ds)
    );
}

#[test]
fn experimental_wgpu_upscale_method_keeps_span_manifest_gated() {
    assert_eq!(
        parse_experimental_wgpu_upscale_method(Some("srlab_span_x2"), false, false),
        None
    );
    assert_eq!(
        parse_experimental_wgpu_upscale_method(Some("srlab_span_x2"), false, true),
        Some(WgpuUpscaleMethod::WgslSrLabSpanX2)
    );
    assert_eq!(
        parse_experimental_wgpu_upscale_method(None, true, true),
        Some(WgpuUpscaleMethod::WgslSrLabSpanX2)
    );
}

#[test]
fn settings_span_upscaler_requires_manifest_for_render() {
    assert_eq!(
        wgpu_upscale_method_from_settings(WgpuUpscaleMethod::WgslSrLabSpanX2, false),
        WgpuUpscaleMethod::Auto
    );
    assert_eq!(
        wgpu_upscale_method_from_settings(WgpuUpscaleMethod::WgslSrLabSpanX2, true),
        WgpuUpscaleMethod::WgslSrLabSpanX2
    );
    assert_eq!(
        wgpu_upscale_method_from_settings(WgpuUpscaleMethod::WgslArtcnnC4F16, false),
        WgpuUpscaleMethod::WgslArtcnnC4F16
    );
    assert_eq!(
        wgpu_upscale_method_from_settings(WgpuUpscaleMethod::NvidiaNis, true),
        WgpuUpscaleMethod::Auto
    );
}

#[test]
fn hidden_realtime_sr_methods_defer_the_first_frame() {
    assert!(defer_initial_realtime_sr_frame(
        WgpuUpscaleMethod::WgslArtcnnC4F16
    ));
    assert!(defer_initial_realtime_sr_frame(
        WgpuUpscaleMethod::WgslArtcnnC4F32Ds
    ));
    assert!(defer_initial_realtime_sr_frame(
        WgpuUpscaleMethod::WgslSrLabSpanX2
    ));
    assert!(!defer_initial_realtime_sr_frame(
        WgpuUpscaleMethod::WgslAnime4kV32CnnX2S
    ));
}

#[test]
fn realtime_sr_stage_texture_keys_separate_stack_stages() {
    let source_key = GpuPaintSourceKey {
        book: 1,
        page: PageCacheKey {
            page_id: PageId(0),
            target_long_edge: 512,
            decode: DecodeOptions::default(),
        },
    };
    let pass_1 = realtime_sr_stage_texture_key(
        source_key,
        [512, 512],
        WgpuUpscaleMethod::WgslAnime4kV32CnnX2S,
        0,
        [512, 512],
        2,
    );
    let pass_2 = realtime_sr_stage_texture_key(
        source_key,
        [512, 512],
        WgpuUpscaleMethod::WgslAnime4kV32CnnX2S,
        1,
        [1024, 1024],
        2,
    );
    let single_pass = realtime_sr_stage_texture_key(
        source_key,
        [512, 512],
        WgpuUpscaleMethod::WgslAnime4kV32CnnX2S,
        0,
        [512, 512],
        1,
    );
    assert_ne!(pass_1, pass_2);
    assert_ne!(pass_1, single_pass);
}

#[test]
fn post_sr_downscale_keys_include_intermediate_content() {
    let anime_content = 10;
    let cunny_content = 20;
    let anime_downscale = downscale_intermediate_texture_key(
        "pyramid",
        anime_content,
        WgpuDownscaleMethod::PyramidLanczos3,
        [1536, 1536],
        [2048, 2048],
        0,
    );
    let cunny_downscale = downscale_intermediate_texture_key(
        "pyramid",
        cunny_content,
        WgpuDownscaleMethod::PyramidLanczos3,
        [1536, 1536],
        [2048, 2048],
        0,
    );
    assert_ne!(anime_downscale, cunny_downscale);
    assert_ne!(
        mipmap_intermediate_texture_key(anime_content),
        mipmap_intermediate_texture_key(cunny_content)
    );
}

#[test]
fn post_realtime_sr_downscale_preserves_requested_filter() {
    assert_eq!(
        post_realtime_sr_downscale_method(
            [2048, 2048],
            [1536, 1536],
            WgpuDownscaleMethod::PyramidLanczos3
        ),
        WgpuDownscaleMethod::PyramidLanczos3
    );
    assert_eq!(
        post_realtime_sr_downscale_method([2048, 2048], [1536, 1536], WgpuDownscaleMethod::Hamming),
        WgpuDownscaleMethod::Hamming
    );
    assert_eq!(
        post_realtime_sr_downscale_method(
            [2048, 2048],
            [1536, 1536],
            WgpuDownscaleMethod::HardwareMipmapLinear
        ),
        WgpuDownscaleMethod::HardwareMipmapLinear
    );
    assert_eq!(
        post_realtime_sr_downscale_method(
            [2048, 2048],
            [3072, 3072],
            WgpuDownscaleMethod::PyramidLanczos3
        ),
        WgpuDownscaleMethod::Bilinear
    );
}

#[test]
fn pyramid_stage_size_halves_until_target_is_within_two_x() {
    assert!(needs_multi_pass_downscale([4096, 4096], [1024, 1024]));
    assert_eq!(
        next_pyramid_stage_size([4096, 4096], [1024, 1024]),
        [2048, 2048]
    );
    assert!(!needs_multi_pass_downscale([2048, 2048], [1024, 1024]));
    assert_eq!(
        next_pyramid_stage_size([4096, 1000], [1024, 1200]),
        [2048, 1000]
    );
}

#[test]
fn mip_helpers_match_expected_floor_chain() {
    assert_eq!(mip_level_count([4096, 1024]), 13);
    assert_eq!(mip_size([4096, 1024], 0), [4096, 1024]);
    assert_eq!(mip_size([4096, 1024], 2), [1024, 256]);
    assert_eq!(mip_size([3, 3], 1), [1, 1]);
}

#[test]
#[ignore = "requires a local WGPU adapter and reads back large render targets"]
fn wgpu_pyramid_downscalers_render_nonblank_output() {
    let cases = [
        WgpuDownscaleMethod::HardwareMipmapLinear,
        WgpuDownscaleMethod::PyramidBoxTent,
        WgpuDownscaleMethod::PyramidHamming,
        WgpuDownscaleMethod::PyramidMitchell,
        WgpuDownscaleMethod::PyramidLanczos2,
        WgpuDownscaleMethod::PyramidLanczos3,
    ];
    pollster::block_on(async {
        let Some((device, queue)) = smoke_device().await else {
            eprintln!("Skipping WGPU downscaler smoke: no adapter available");
            return;
        };
        for source_size in [[2048, 2048], [4096, 4096]] {
            for downscaler in cases {
                assert!(
                    render_downscale_smoke(&device, &queue, source_size, downscaler),
                    "{} {:?} -> 1024x1024 produced a blank output",
                    downscaler.token(),
                    source_size
                );
            }
        }
    });
}

#[test]
#[ignore = "release timing probe for local GPU downscale paths"]
fn wgpu_default_downscaler_timing_probe() {
    pollster::block_on(async {
        let Some((device, queue)) = smoke_device().await else {
            eprintln!("Skipping WGPU downscaler timing: no adapter available");
            return;
        };
        for source_size in [[2048, 2048], [4096, 4096]] {
            for downscaler in [
                WgpuDownscaleMethod::Hamming,
                WgpuDownscaleMethod::PyramidLanczos3,
            ] {
                let mut fixture = DownscaleSmokeFixture::new(&device, &queue, source_size);
                for _ in 0..2 {
                    assert!(render_downscale_frame(
                        &device,
                        &queue,
                        &mut fixture,
                        downscaler
                    ));
                }
                let mut samples = Vec::with_capacity(12);
                for _ in 0..12 {
                    let started = std::time::Instant::now();
                    assert!(render_downscale_frame(
                        &device,
                        &queue,
                        &mut fixture,
                        downscaler
                    ));
                    samples.push(started.elapsed().as_secs_f64() * 1000.0);
                }
                samples.sort_by(|left, right| left.total_cmp(right));
                let avg = samples.iter().sum::<f64>() / samples.len() as f64;
                let p95_index = (samples.len() * 95).div_ceil(100).saturating_sub(1);
                println!(
                        "wgpu_downscale_timing source={}x{} target=1024x1024 method={} avg_ms={:.3} p95_ms={:.3}",
                        source_size[0],
                        source_size[1],
                        downscaler.token(),
                        avg,
                        samples[p95_index]
                    );
            }
        }
    });
}

#[test]
#[ignore = "requires a local WGPU adapter and compiles realtime SR shaders"]
fn wgpu_realtime_sr_downscale_after_sr_renders_nonblank_output() {
    pollster::block_on(async {
        let Some((device, queue)) = smoke_device().await else {
            eprintln!("Skipping realtime SR downscale smoke: no adapter available");
            return;
        };
        assert!(render_realtime_sr_frame(
            &device,
            &queue,
            [1024, 1024],
            [1536, 1536],
            WgpuUpscaleMethod::WgslAnime4kV32CnnX2S,
            WgpuDownscaleMethod::PyramidLanczos3,
        ));
    });
}

#[test]
#[ignore = "requires a local WGPU adapter and compiles realtime SR shaders"]
fn wgpu_realtime_sr_stacking_renders_nonblank_output() {
    pollster::block_on(async {
        let Some((device, queue)) = smoke_device().await else {
            eprintln!("Skipping realtime SR stacking smoke: no adapter available");
            return;
        };
        assert!(render_realtime_sr_frame(
            &device,
            &queue,
            [512, 512],
            [1536, 1536],
            WgpuUpscaleMethod::WgslAnime4kV32CnnX2S,
            WgpuDownscaleMethod::PyramidLanczos3,
        ));
    });
}

async fn smoke_device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::default();
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
        })
        .await
        .ok()?;
    adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("suisuiview-gpu-downscale-smoke-device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::downlevel_webgl2_defaults()
                .using_resolution(adapter.limits()),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        })
        .await
        .ok()
}

fn render_downscale_smoke(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    source_size: [usize; 2],
    downscaler: WgpuDownscaleMethod,
) -> bool {
    let mut fixture = DownscaleSmokeFixture::new(device, queue, source_size);
    render_downscale_frame(device, queue, &mut fixture, downscaler)
}

struct DownscaleSmokeFixture {
    resources: GpuPaintResources,
    source_key: GpuPaintSourceKey,
    source_size: [usize; 2],
}

impl DownscaleSmokeFixture {
    fn new(device: &wgpu::Device, queue: &wgpu::Queue, source_size: [usize; 2]) -> Self {
        let mut resources = GpuPaintResources::new(device, wgpu::TextureFormat::Rgba8Unorm);
        let source_key = GpuPaintSourceKey {
            book: 1,
            page: PageCacheKey {
                page_id: PageId(source_size[0] as u32),
                target_long_edge: source_size[0] as u32,
                decode: DecodeOptions::default(),
            },
        };
        let pixels = PagePixels::Rgba(smoke_rgba(source_size).into());
        resources.ensure_source_texture(device, queue, source_key, source_size, &pixels);
        Self {
            resources,
            source_key,
            source_size,
        }
    }
}

fn render_downscale_frame(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    fixture: &mut DownscaleSmokeFixture,
    downscaler: WgpuDownscaleMethod,
) -> bool {
    let output_size = fixture.source_size;
    render_gpu_frame(
        device,
        queue,
        fixture,
        output_size,
        [1024, 1024],
        WgpuUpscaleMethod::None,
        downscaler,
    )
}

fn render_realtime_sr_frame(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    source_size: [usize; 2],
    target_size: [u32; 2],
    upscaler: WgpuUpscaleMethod,
    downscaler: WgpuDownscaleMethod,
) -> bool {
    let mut fixture = DownscaleSmokeFixture::new(device, queue, source_size);
    render_gpu_frame(
        device,
        queue,
        &mut fixture,
        source_size,
        target_size,
        upscaler,
        downscaler,
    )
}

fn render_gpu_frame(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    fixture: &mut DownscaleSmokeFixture,
    output_size: [usize; 2],
    target_size: [u32; 2],
    upscaler: WgpuUpscaleMethod,
    downscaler: WgpuDownscaleMethod,
) -> bool {
    let resources = &mut fixture.resources;
    let source_key = fixture.source_key;
    let source_bind_group = resources
        .source_textures
        .peek(&source_key)
        .expect("smoke source texture should be uploaded")
        .bind_group
        .clone();
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("suisuiview-gpu-downscale-smoke-encoder"),
    });
    let draw_state = resources.prepare_draw_state(
        device,
        &mut encoder,
        source_key,
        source_bind_group,
        fixture.source_size,
        output_size,
        ViewEffects::default(),
        upscaler,
        downscaler,
        GpuDisplayRect {
            origin: [0, 0],
            visible_size: target_size,
            sample_offset: [0, 0],
            full_size: target_size,
        },
        1.0,
        &egui::Context::default(),
    );
    let output_texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("suisuiview-gpu-downscale-smoke-output"),
        size: wgpu::Extent3d {
            width: target_size[0],
            height: target_size[1],
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let output_view = output_texture.create_view(&wgpu::TextureViewDescriptor::default());
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("suisuiview-gpu-downscale-smoke-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &output_view,
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
        pass.set_pipeline(&resources.pipeline);
        pass.set_bind_group(0, draw_state.texture_bind_group.as_ref(), &[]);
        pass.set_bind_group(1, &draw_state.params_bind_group, &[]);
        pass.draw(0..3, 0..1);
    }

    let padded_bytes_per_row =
        align_to_smoke(target_size[0] * 4, wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("suisuiview-gpu-downscale-smoke-readback"),
        size: padded_bytes_per_row as u64 * u64::from(target_size[1]),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &output_texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_bytes_per_row),
                rows_per_image: Some(target_size[1]),
            },
        },
        wgpu::Extent3d {
            width: target_size[0],
            height: target_size[1],
            depth_or_array_layers: 1,
        },
    );
    queue.submit(Some(encoder.finish()));
    let buffer_slice = readback.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = tx.send(result);
    });
    device.poll(wgpu::PollType::Wait).unwrap();
    rx.recv().unwrap().unwrap();
    let mapped = buffer_slice.get_mapped_range();
    let nonblank = mapped
        .chunks_exact(4)
        .any(|pixel| pixel[0] != 0 || pixel[1] != 0 || pixel[2] != 0 || pixel[3] != 0);
    drop(mapped);
    readback.unmap();
    #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
    {
        let _ = crate::core::perf_trace::flush_timeout(Duration::from_secs(2));
    }
    nonblank
}

fn smoke_rgba(size: [usize; 2]) -> Vec<u8> {
    let [width, height] = size;
    let mut rgba = vec![0u8; width * height * 4];
    for y in 0..height {
        for x in 0..width {
            let offset = (y * width + x) * 4;
            rgba[offset] = (x % 251) as u8;
            rgba[offset + 1] = (y % 241) as u8;
            rgba[offset + 2] = ((x + y) % 239) as u8;
            rgba[offset + 3] = 255;
        }
    }
    rgba
}

fn align_to_smoke(value: u32, alignment: u32) -> u32 {
    value.div_ceil(alignment) * alignment
}
