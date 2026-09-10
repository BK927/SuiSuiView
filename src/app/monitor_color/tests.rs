use super::gpu::OutputLut;
use crate::core::monitor_color::build_display_lut;
use lcms2::{CIExyY, CIExyYTRIPLE, Intent, PixelFormat, Profile, ToneCurve, Transform};

#[test]
fn monitor_color_off_has_no_worker_or_gpu_state() {
    let controller = super::MonitorColor::default();
    assert!(controller.loader.is_none());
    assert!(controller.scene.is_none());
    assert!(controller.ready.is_none());
    assert_eq!(
        super::status(&egui::Context::default()),
        super::MonitorColorStatus::Off
    );
}

#[test]
fn monitor_color_ignores_previous_monitor_completion() {
    let ctx = egui::Context::default();
    let mut controller = super::MonitorColor {
        display: Some("display-2".to_owned()),
        ..Default::default()
    };
    let loaded = |display: &str| super::loader::LoadedProfile {
        display: display.to_owned(),
        fingerprint: 0,
        lut: None,
        renderer: None,
        status: super::MonitorColorStatus::SystemSrgb,
    };
    controller.accept_loaded(&ctx, loaded("display-1"));
    assert!(controller.ready.is_none());
    controller.accept_loaded(&ctx, loaded("display-2"));
    assert!(controller.ready.is_some());
    assert_eq!(super::status(&ctx), super::MonitorColorStatus::SystemSrgb);
}

#[test]
#[ignore = "requires a local WGPU adapter for display-color readback"]
fn monitor_color_gpu_matches_lcms_after_alpha_composition() {
    pollster::block_on(async {
        let instance = wgpu::Instance::default();
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions::default())
            .await
            .expect("GPU adapter required for monitor color validation");
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("monitor-color-test"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::downlevel_defaults()
                    .using_resolution(adapter.limits()),
                memory_hints: wgpu::MemoryHints::MemoryUsage,
                trace: wgpu::Trace::Off,
            })
            .await
            .unwrap();
        let curve = ToneCurve::new(1.8);
        let display = Profile::new_rgb(
            &CIExyY {
                x: 0.3127,
                y: 0.3290,
                Y: 1.0,
            },
            &CIExyYTRIPLE {
                Red: CIExyY {
                    x: 0.64,
                    y: 0.33,
                    Y: 1.0,
                },
                Green: CIExyY {
                    x: 0.30,
                    y: 0.60,
                    Y: 1.0,
                },
                Blue: CIExyY {
                    x: 0.15,
                    y: 0.06,
                    Y: 1.0,
                },
            },
            &[&curve, &curve, &curve],
        )
        .unwrap();
        for profile in [Profile::new_srgb(), display] {
            let bytes = profile.icc().unwrap();
            for format in [
                wgpu::TextureFormat::Rgba8Unorm,
                wgpu::TextureFormat::Rgba8UnormSrgb,
            ] {
                verify_composite(&device, &queue, format, &bytes);
            }
        }
        verify_egui_borrowed_textures(&device, &queue);
    });
}

fn verify_composite(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    format: wgpu::TextureFormat,
    profile: &[u8],
) {
    let values = build_display_lut(profile).unwrap();
    let lut = OutputLut::new(device, queue, format, &values);
    let size = wgpu::Extent3d {
        width: 256,
        height: 1,
        depth_or_array_layers: 1,
    };
    let texture = |label, format| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            view_formats: &[],
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
        })
    };
    let scene = texture("monitor-test-composite", super::SCENE_FORMAT);
    let output = texture("monitor-test-output", format);
    let scene_view = scene.create_view(&Default::default());
    let output_view = output.create_view(&Default::default());
    let binding = lut.binding(device, &scene_view);
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: None,
        source: wgpu::ShaderSource::Wgsl(
            r#"
            @vertex fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
                let xy = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
                return vec4<f32>(xy * 2.0 - 1.0, 0.0, 1.0);
            }
            @fragment fn fs_main(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
                return vec4<f32>(p.x / 512.0, 0.05, 0.10, 0.5);
            }
        "#
            .into(),
        ),
    });
    let blend_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: None,
        layout: None,
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: super::SCENE_FORMAT,
                blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        multiview: None,
        cache: None,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    for (view, pipeline, has_binding) in [
        (&scene_view, &blend_pipeline, false),
        (&output_view, &lut.pipeline, true),
    ] {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.1,
                        g: 0.4,
                        b: 0.6,
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(pipeline);
        if has_binding {
            pass.set_bind_group(0, &binding, &[]);
        }
        pass.draw(0..3, 0..1);
    }
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 3072,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    for (source, offset, bytes_per_row) in [(&scene, 0, 2048), (&output, 2048, 1024)] {
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: source,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(1),
                },
            },
            size,
        );
    }
    queue.submit([encoder.finish()]);
    let (send, receive) = std::sync::mpsc::channel();
    readback
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            let _ = send.send(result);
        });
    device.poll(wgpu::PollType::Wait).unwrap();
    receive.recv().unwrap().unwrap();
    let actual = readback.slice(..).get_mapped_range();
    let mut expected: Vec<[f32; 3]> = actual[..2048]
        .chunks_exact(8)
        .map(|pixel| {
            [0, 2, 4]
                .map(|offset| half_to_float(u16::from_le_bytes([pixel[offset], pixel[offset + 1]])))
        })
        .collect();
    let levels: std::collections::HashSet<u32> =
        expected.iter().map(|pixel| pixel[0].to_bits()).collect();
    assert_eq!(
        levels.len(),
        256,
        "composition must preserve sub-8-bit values before the ICC LUT"
    );
    let source = Profile::new_srgb();
    let destination = Profile::new_icc(profile).unwrap();
    let transform = Transform::<[f32; 3], [f32; 3]>::new(
        &source,
        PixelFormat::RGB_FLT,
        &destination,
        PixelFormat::RGB_FLT,
        Intent::RelativeColorimetric,
    )
    .unwrap();
    transform.transform_in_place(&mut expected);
    for (index, pixel) in actual[2048..].chunks_exact(4).enumerate() {
        assert_eq!(pixel[3], 255, "composition must precede output conversion");
        for channel in 0..3 {
            assert!(
                (pixel[channel] as f32 - expected[index][channel].clamp(0.0, 1.0) * 255.0).abs()
                    <= 1.5,
                "{format:?} pixel {index} channel {channel}: {} vs {}",
                pixel[channel],
                expected[index][channel]
            );
        }
    }
    drop(actual);
    readback.unmap();
}

fn half_to_float(value: u16) -> f32 {
    let exponent = (value >> 10) & 31;
    let fraction = (value & 1023) as f32;
    if exponent == 0 {
        fraction * 2.0_f32.powi(-24)
    } else {
        (1.0 + fraction / 1024.0) * 2.0_f32.powi(exponent as i32 - 15)
    }
}

fn verify_egui_borrowed_textures(device: &wgpu::Device, queue: &wgpu::Queue) {
    use egui::{epaint::Primitive, Color32, ColorImage, TextureId, TextureOptions};
    let mut surface =
        egui_wgpu::Renderer::new(device, wgpu::TextureFormat::Rgba8Unorm, None, 1, true);
    let id = TextureId::Managed(77);
    surface.update_texture(
        device,
        queue,
        id,
        &egui::epaint::ImageDelta::full(
            ColorImage::filled([2, 2], Color32::WHITE),
            TextureOptions::LINEAR,
        ),
    );
    let source = surface.texture(&id).unwrap().texture.clone().unwrap();
    let mut composition = super::scene::SceneRenderer::new(device);
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(16.0, 16.0));
    let mut mesh = egui::epaint::Mesh::with_texture(id);
    mesh.add_rect_with_uv(
        rect,
        egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
        Color32::from_rgba_premultiplied(100, 25, 50, 128),
    );
    let primitives = vec![egui::ClippedPrimitive {
        clip_rect: rect,
        primitive: Primitive::Mesh(mesh),
    }];
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("monitor-egui-fp16-test"),
        size: wgpu::Extent3d {
            width: 16,
            height: 16,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: super::SCENE_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    // Existing font/image textures remain valid through managed frames and an
    // in-place update, with no duplicate pixel allocation in the FP16 renderer.
    for _ in 0..2 {
        let mut primitives = primitives.clone();
        composition.borrow_textures(device, &surface, &mut primitives);
        let Primitive::Mesh(mesh) = &primitives[0].primitive else {
            unreachable!()
        };
        assert!(composition
            .renderer
            .texture(&mesh.texture_id)
            .unwrap()
            .texture
            .is_none());
        assert_eq!(
            surface.texture(&id).unwrap().texture.as_ref().unwrap(),
            &source
        );
        let mut encoder = device.create_command_encoder(&Default::default());
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [16, 16],
            pixels_per_point: 1.0,
        };
        let before =
            composition
                .renderer
                .update_buffers(device, queue, &mut encoder, &primitives, &screen);
        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            composition
                .renderer
                .render(&mut pass.forget_lifetime(), &primitives, &screen);
        }
        queue.submit(before.into_iter().chain([encoder.finish()]));
        surface.update_texture(
            device,
            queue,
            id,
            &egui::epaint::ImageDelta::partial(
                [0, 0],
                ColorImage::filled([1, 1], Color32::LIGHT_GRAY),
                TextureOptions::LINEAR,
            ),
        );
    }
    composition.borrow_textures(device, &surface, &mut []);
    drop(composition);
    assert_eq!(
        surface.texture(&id).unwrap().texture.as_ref().unwrap(),
        &source
    );
    device.poll(wgpu::PollType::Wait).unwrap();
}
