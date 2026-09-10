use super::*;
use crate::app::gpu_paint::{
    tests::{render_stored_slot_rgba, smoke_device},
    GpuPaintSourceKey,
};
use crate::app::PageCacheKey;
use crate::core::deband::ResolvedDeband;
use crate::core::effects::ViewTransform;
use crate::core::source::PageId;
use crate::core::worker::{DecodeOptions, PagePixels};

fn display(size: [u32; 2]) -> GpuDisplayRect {
    GpuDisplayRect {
        origin: [0, 0],
        visible_size: size,
        sample_offset: [0, 0],
        full_size: size,
    }
}

#[allow(clippy::too_many_arguments)]
fn prepare_input(
    resources: &mut GpuPaintResources,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    encoder: &mut wgpu::CommandEncoder,
    source: GpuPaintSourceKey,
    slot: u64,
    effects: ViewEffects,
) -> GpuDrawState {
    let bind_group = resources
        .source_textures
        .peek(&source)
        .unwrap()
        .bind_group
        .clone();
    resources.prepare_draw_state(
        device,
        queue,
        slot,
        encoder,
        source,
        bind_group,
        [32, 16],
        [16, 32],
        effects,
        WgpuUpscaleMethod::WgslFsr1EasuRcas,
        WgpuDownscaleMethod::PyramidLanczos3,
        1.1,
        display([32, 64]),
        1.0,
        false,
        ResolvedDeband::Off,
        &egui::Context::default(),
    )
}

fn assert_near(actual: &[u8], expected: &[u8]) {
    assert_eq!(actual.len(), expected.len());
    let max = actual
        .iter()
        .zip(expected)
        .map(|(a, b)| a.abs_diff(*b))
        .max()
        .unwrap();
    assert!(
        max <= 1,
        "inspection differs by {max} levels, expected at most fp16 rounding"
    );
}

#[test]
#[ignore = "requires a local WGPU adapter; checks actual A/B, alpha, transformed crop, and result replacement"]
fn inspection_gpu_matches_full_chain_and_updates_cached_result() {
    pollster::block_on(async {
        let (device, queue) = smoke_device().await.expect("local WGPU adapter required");
        let mut resources = GpuPaintResources::new(&device, wgpu::TextureFormat::Rgba8Unorm);
        let source = GpuPaintSourceKey {
            book: 7,
            page: PageCacheKey {
                page_id: PageId(2),
                target_long_edge: 32,
                decode: DecodeOptions::default(),
            },
        };
        let rgba = (0..32 * 16)
            .flat_map(|index| {
                let x = index % 32;
                let y = index / 32;
                [
                    (x * 7) as u8,
                    (y * 11) as u8,
                    (50 + x * 3) as u8,
                    (40 + x * 6) as u8,
                ]
            })
            .collect::<Vec<_>>();
        resources.ensure_source_texture(
            &device,
            &queue,
            source,
            [32, 16],
            &PagePixels::Rgba(rgba.into()),
        );
        let a = ViewEffects {
            transform: ViewTransform {
                rotation_quadrants: 1,
                flip_horizontal: true,
                ..ViewTransform::default()
            },
            ..ViewEffects::default()
        };
        let mut b = a;
        b.gamma = true;
        b.tone.gamma_pct = 150;
        b.tone.brightness_pct = 10;
        b.tone.contrast_pct = 120;
        let full = Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0));
        let reference_a = GpuInspection {
            reference_size: [32, 64],
            uv: full,
            reference_id: 101,
        };
        let reference_b = GpuInspection {
            reference_id: 102,
            ..reference_a
        };
        let mut encoder = device.create_command_encoder(&Default::default());
        for (slot, effects) in [(1, a), (2, b)] {
            let draw = prepare_input(
                &mut resources,
                &device,
                &queue,
                &mut encoder,
                source,
                slot,
                effects,
            );
            resources.insert_draw_state(slot, draw);
        }
        for (slot, effects, reference) in [(3, a, reference_a), (4, b, reference_b)] {
            let input = prepare_input(
                &mut resources,
                &device,
                &queue,
                &mut encoder,
                source,
                slot + 100,
                effects,
            );
            let draw = resources.prepare_inspection_draw(
                &device,
                &queue,
                &mut encoder,
                slot,
                input,
                reference,
                display([32, 64]),
            );
            resources.insert_draw_state(slot, draw);
        }
        let input = prepare_input(
            &mut resources,
            &device,
            &queue,
            &mut encoder,
            source,
            105,
            a,
        );
        let crop = GpuInspection {
            uv: Rect::from_min_max(egui::pos2(0.25, 0.25), egui::pos2(0.75, 0.75)),
            ..reference_a
        };
        let draw = resources.prepare_inspection_draw(
            &device,
            &queue,
            &mut encoder,
            5,
            input,
            crop,
            display([16, 32]),
        );
        resources.insert_draw_state(5, draw);
        queue.submit(Some(encoder.finish()));
        let direct_a = render_stored_slot_rgba(&device, &queue, &resources, 1, [32, 64]);
        let direct_b = render_stored_slot_rgba(&device, &queue, &resources, 2, [32, 64]);
        assert_ne!(
            direct_a, direct_b,
            "A/B fixture must exercise different color controls"
        );
        let inspected_a = render_stored_slot_rgba(&device, &queue, &resources, 3, [32, 64]);
        assert_near(&inspected_a, &direct_a);
        assert_near(
            &render_stored_slot_rgba(&device, &queue, &resources, 4, [32, 64]),
            &direct_b,
        );
        let expected_crop = (16..48)
            .flat_map(|y| {
                direct_a[(y * 32 + 8) * 4..(y * 32 + 24) * 4]
                    .iter()
                    .copied()
            })
            .collect::<Vec<_>>();
        assert_near(
            &render_stored_slot_rgba(&device, &queue, &resources, 5, [16, 32]),
            &expected_crop,
        );

        // A newly completed input may replace an earlier fallback under the same
        // processing identity. Both the main result and crop must see that write.
        let mut encoder = device.create_command_encoder(&Default::default());
        let input = prepare_input(
            &mut resources,
            &device,
            &queue,
            &mut encoder,
            source,
            106,
            b,
        );
        let draw = resources.prepare_inspection_draw(
            &device,
            &queue,
            &mut encoder,
            3,
            input,
            reference_a,
            display([32, 64]),
        );
        resources.insert_draw_state(3, draw);
        queue.submit(Some(encoder.finish()));
        assert_near(
            &render_stored_slot_rgba(&device, &queue, &resources, 3, [32, 64]),
            &direct_b,
        );
        let expected_crop = (16..48)
            .flat_map(|y| {
                direct_b[(y * 32 + 8) * 4..(y * 32 + 24) * 4]
                    .iter()
                    .copied()
            })
            .collect::<Vec<_>>();
        assert_near(
            &render_stored_slot_rgba(&device, &queue, &resources, 5, [16, 32]),
            &expected_crop,
        );
    });
}
