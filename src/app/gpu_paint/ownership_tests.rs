use super::*;
use crate::core::source::PageId;
use crate::core::worker::DecodeOptions;
use std::borrow::Cow;
use std::num::NonZeroUsize;

#[test]
fn rgba_upload_borrows_canonical_pixels() {
    let pixels = PagePixels::Rgba(vec![17, 29, 53, 127, 255, 0, 91, 0].into());
    let upload = pools::source_upload_bytes(&pixels, [2, 1]);
    assert!(matches!(upload, Cow::Borrowed(_)));
    assert_eq!(upload.as_ptr(), pixels.as_slice().as_ptr());
    assert_eq!(upload.as_ref(), pixels.as_slice());
}

#[test]
fn luma_upload_expands_without_retaining_rgba() {
    let pixels = PagePixels::Luma(vec![0, 127, 255].into());
    let upload = pools::source_upload_bytes(&pixels, [3, 1]);
    assert_eq!(
        upload.as_ref(),
        &[0, 0, 0, 255, 127, 127, 127, 255, 255, 255, 255, 255]
    );
    assert_eq!(pixels.byte_len(), 3);
}

#[test]
#[ignore = "local allocation probe with generated pixels; not a page-turn benchmark"]
fn rgba_upload_preparation_probe() {
    use std::hint::black_box;
    use std::time::Instant;

    for size in [[2048, 3072], [4096, 4096]] {
        let pixels = PagePixels::Rgba(vec![127; size[0] * size[1] * 4].into());
        let mut copy_ns = Vec::new();
        let mut borrow_ns = Vec::new();
        for iteration in 0..33 {
            let start = Instant::now();
            let copy = black_box(pixels.to_rgba_vec(size[0], size[1]));
            let copied_in = start.elapsed().as_nanos();
            assert_eq!(copy.len(), pixels.byte_len());
            drop(copy);
            let start = Instant::now();
            let upload = black_box(pools::source_upload_bytes(black_box(&pixels), size));
            let borrowed_in = start.elapsed().as_nanos();
            assert!(matches!(upload, Cow::Borrowed(_)));
            if iteration > 0 {
                copy_ns.push(copied_in);
                borrow_ns.push(borrowed_in);
            }
        }
        copy_ns.sort_unstable();
        borrow_ns.sort_unstable();
        println!(
            "{{\"event\":\"rgba_upload_preparation\",\"width\":{},\"height\":{},\"samples\":32,\"avoided_copy_bytes\":{},\"copy_p50_ns\":{},\"copy_p95_ns\":{},\"copy_max_ns\":{},\"borrow_p50_ns\":{},\"borrow_p95_ns\":{},\"borrow_max_ns\":{}}}",
            size[0], size[1], pixels.byte_len(), copy_ns[15], copy_ns[30], copy_ns[31],
            borrow_ns[15], borrow_ns[30], borrow_ns[31],
        );
    }
}

fn source_key(page: u32) -> GpuPaintSourceKey {
    GpuPaintSourceKey {
        book: 1,
        page: PageCacheKey {
            page_id: PageId(page),
            target_long_edge: 64,
            decode: DecodeOptions::default(),
        },
    }
}

fn insert_draw(
    resources: &mut GpuPaintResources,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    id: u64,
    binding: Arc<wgpu::BindGroup>,
    intermediates: Vec<Arc<GpuIntermediateTexture>>,
) {
    let params = crate::core::gpu_effect::params_for_effects(
        [64, 64],
        [64, 64],
        ViewEffects::default(),
        WgpuUpscaleMethod::None,
        WgpuDownscaleMethod::Bilinear,
        [0, 0],
        [64, 64],
        1.0,
    );
    let (buffer, params_binding) = resources.recycle_params_pair(device, queue, id, params);
    resources.insert_draw_state(
        id,
        GpuDrawState::new(binding, buffer, params_binding, intermediates),
    );
}

#[test]
#[ignore = "requires a local WGPU adapter; no app, user profile, or media files"]
fn evicted_sources_release_retired_direct_draws() {
    pollster::block_on(async {
        let (device, queue) = tests::smoke_device().await.expect("GPU adapter required");
        let pixels = PagePixels::Rgba(vec![128; 64 * 64 * 4].into());
        let mut resources = GpuPaintResources::new(&device, wgpu::TextureFormat::Rgba8Unorm);
        for capacity_eviction in [false, true] {
            resources.source_textures.clear();
            resources.source_texture_bytes = 0;
            resources.draw_bind_groups.clear();
            resources.draw_state_intermediate_bytes = 0;
            resources
                .source_textures
                .resize(NonZeroUsize::new(if capacity_eviction { 1 } else { 32 }).unwrap());
            resources.source_texture_budget_bytes = 64 * 64 * 4;
            resources.current_pass = 1;
            resources.ensure_source_texture(&device, &queue, source_key(1), [64, 64], &pixels);
            let binding = resources
                .source_textures
                .peek(&source_key(1))
                .unwrap()
                .bind_group
                .clone();
            let lifetime = Arc::downgrade(&binding);
            insert_draw(
                &mut resources,
                &device,
                &queue,
                1,
                binding.clone(),
                Vec::new(),
            );
            insert_draw(&mut resources, &device, &queue, 2, binding, Vec::new());
            // Cached quality output has independent storage and remains reusable.
            resources.ensure_intermediate_texture(&device, 9, [32, 32]);
            let intermediate = resources.intermediate_textures.peek(&9).unwrap().clone();
            let quality_bytes = intermediate.byte_size;
            insert_draw(
                &mut resources,
                &device,
                &queue,
                9,
                intermediate.bind_group.clone(),
                vec![intermediate],
            );
            resources.current_pass = 2;
            resources.ensure_source_texture(&device, &queue, source_key(2), [64, 64], &pixels);
            assert!(resources.source_textures.peek(&source_key(1)).is_none());
            assert!(lifetime.upgrade().is_none(), "a retired draw still owns the evicted source binding; capacity={capacity_eviction}");
            assert_eq!(resources.draw_bind_groups.len(), 1);
            assert!(resources.draw_bind_groups.peek(&9).is_some());
            assert_eq!(resources.draw_state_intermediate_bytes, quality_bytes);
            assert_eq!(resources.source_texture_bytes, 64 * 64 * 4);
        }
        device.poll(wgpu::PollType::Wait).unwrap();
    });
}

#[test]
#[ignore = "requires a local WGPU adapter; no app, user profile, or media files"]
fn current_pass_draw_survives_source_capacity_eviction() {
    pollster::block_on(async {
        let (device, queue) = tests::smoke_device().await.expect("GPU adapter required");
        let pixels = PagePixels::Rgba(vec![128; 64 * 64 * 4].into());
        let mut resources = GpuPaintResources::new(&device, wgpu::TextureFormat::Rgba8Unorm);
        resources
            .source_textures
            .resize(NonZeroUsize::new(1).unwrap());
        resources.current_pass = 1;
        resources.ensure_source_texture(&device, &queue, source_key(1), [64, 64], &pixels);
        let binding = resources
            .source_textures
            .peek(&source_key(1))
            .unwrap()
            .bind_group
            .clone();
        let lifetime = Arc::downgrade(&binding);
        insert_draw(&mut resources, &device, &queue, 1, binding, Vec::new());
        resources.ensure_source_texture(&device, &queue, source_key(2), [64, 64], &pixels);
        assert!(resources.source_textures.peek(&source_key(1)).is_none());
        assert!(resources.draw_bind_groups.peek(&1).is_some());
        assert!(
            lifetime.upgrade().is_some(),
            "current frame must still be able to paint"
        );
        device.poll(wgpu::PollType::Wait).unwrap();
    });
}
