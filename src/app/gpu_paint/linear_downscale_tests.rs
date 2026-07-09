//! V13 offscreen measurement: gamma-space vs linear-light WGPU downscale, on a
//! real multi-stage pyramid. Renders both legs (flag forced per-render via the
//! params path, not the env override) and prints the numbers the supervisor uses
//! to pick the shipped default. Shares the real-device harness in `super::tests`.

use super::tests::{capture_gpu_frame, smoke_device, DownscaleSmokeFixture};
use crate::core::gpu_effect::set_linear_downscale_test_override;
use crate::core::state::{WgpuDownscaleMethod, WgpuUpscaleMethod};

/// 1px black/white checkerboard. Downscaling this exercises the worst case for
/// gamma-space averaging: the mean lands at sRGB ~127 (21% light) instead of the
/// physically-correct 50%-light mean (sRGB ~188).
fn checker_rgba(size: [usize; 2]) -> Vec<u8> {
    let [width, height] = size;
    let mut rgba = vec![0u8; width * height * 4];
    for y in 0..height {
        for x in 0..width {
            let value = if (x + y) % 2 == 0 { 0u8 } else { 255u8 };
            let offset = (y * width + x) * 4;
            rgba[offset] = value;
            rgba[offset + 1] = value;
            rgba[offset + 2] = value;
            rgba[offset + 3] = 255;
        }
    }
    rgba
}

/// Light background (sRGB 235) with 2px-wide black vertical strokes every 16px,
/// like `deband_quality`'s line-art fixture. Downscaling thins dark strokes more
/// under linear-light than gamma — the reason font AA is done in gamma space and
/// the tension this whole stage measures.
fn line_art_rgba(size: [usize; 2]) -> Vec<u8> {
    let [width, height] = size;
    let mut rgba = vec![0u8; width * height * 4];
    for y in 0..height {
        for x in 0..width {
            let in_stroke = (x % 16) < 2;
            let value = if in_stroke { 0u8 } else { 235u8 };
            let offset = (y * width + x) * 4;
            rgba[offset] = value;
            rgba[offset + 1] = value;
            rgba[offset + 2] = value;
            rgba[offset + 3] = 255;
        }
    }
    rgba
}

/// Mean of the red channel over an interior region (skips `border` px on every
/// side to avoid filter/clamp edge effects).
fn interior_mean(pixels: &[u8], size: [usize; 2], border: usize) -> f64 {
    let [width, height] = size;
    let mut sum = 0u64;
    let mut count = 0u64;
    for y in border..height.saturating_sub(border) {
        for x in border..width.saturating_sub(border) {
            sum += pixels[(y * width + x) * 4] as u64;
            count += 1;
        }
    }
    sum as f64 / count.max(1) as f64
}

/// Per-column mean of the red channel over interior rows.
fn column_means(pixels: &[u8], size: [usize; 2], border: usize) -> Vec<f64> {
    let [width, height] = size;
    (0..width)
        .map(|x| {
            let mut sum = 0u64;
            let mut count = 0u64;
            for y in border..height.saturating_sub(border) {
                sum += pixels[(y * width + x) * 4] as u64;
                count += 1;
            }
            sum as f64 / count.max(1) as f64
        })
        .collect()
}

fn all_alpha_opaque(pixels: &[u8]) -> bool {
    pixels.chunks_exact(4).all(|pixel| pixel[3] == 255)
}

fn any_nonblank(pixels: &[u8]) -> bool {
    pixels
        .chunks_exact(4)
        .any(|pixel| pixel[0] != 0 || pixel[1] != 0 || pixel[2] != 0)
}

#[test]
#[ignore = "requires a local WGPU adapter; measures gamma vs linear-light downscale"]
fn wgpu_linear_downscale_gamma_vs_linear_measurement() {
    pollster::block_on(async {
        let Some((device, queue)) = smoke_device().await else {
            eprintln!("Skipping linear-downscale measurement: no adapter available");
            return;
        };

        // Render one path with the linear flag forced on/off, on a FRESH fixture
        // each time so the content-keyed pyramid cache (which does not key on the
        // flag) can never serve the other path's stale intermediates.
        let render = |linear: bool, source_size: [usize; 2], target: [u32; 2], rgba: Vec<u8>| {
            set_linear_downscale_test_override(Some(linear));
            let mut fixture = DownscaleSmokeFixture::with_rgba(&device, &queue, source_size, rgba);
            let pixels = capture_gpu_frame(
                &device,
                &queue,
                &mut fixture,
                source_size,
                target,
                WgpuUpscaleMethod::None,
                WgpuDownscaleMethod::PyramidLanczos3,
            );
            set_linear_downscale_test_override(None);
            pixels
        };

        // (a) CHECKER, 4x downscale (multi-stage pyramid).
        let checker_source = [2048usize, 2048usize];
        let checker_target = [512u32, 512u32];
        let checker_dims = [checker_target[0] as usize, checker_target[1] as usize];
        let checker_gamma = render(
            false,
            checker_source,
            checker_target,
            checker_rgba(checker_source),
        );
        let checker_linear = render(
            true,
            checker_source,
            checker_target,
            checker_rgba(checker_source),
        );
        let checker_gamma_mean = interior_mean(&checker_gamma, checker_dims, 64);
        let checker_linear_mean = interior_mean(&checker_linear, checker_dims, 64);

        // (b) LINE-ART, 2x downscale (single-pass Lanczos).
        let line_source = [1024usize, 1024usize];
        let line_target = [512u32, 512u32];
        let line_dims = [line_target[0] as usize, line_target[1] as usize];
        let line_gamma = render(false, line_source, line_target, line_art_rgba(line_source));
        let line_linear = render(true, line_source, line_target, line_art_rgba(line_source));
        let line_gamma_mean = interior_mean(&line_gamma, line_dims, 16);
        let line_linear_mean = interior_mean(&line_linear, line_dims, 16);
        // Stroke-region mean: average over the darkest ~30% of columns (the stroke
        // troughs), selected from the gamma pass and applied to both so the same
        // columns are compared.
        let gamma_cols = column_means(&line_gamma, line_dims, 16);
        let linear_cols = column_means(&line_linear, line_dims, 16);
        let interior_cols: Vec<usize> = (16..line_dims[0] - 16).collect();
        let mut ranked = interior_cols.clone();
        ranked.sort_by(|a, b| gamma_cols[*a].total_cmp(&gamma_cols[*b]));
        let stroke_count = (ranked.len() * 3 / 10).max(1);
        let stroke_cols = &ranked[..stroke_count];
        let gamma_stroke_mean =
            stroke_cols.iter().map(|c| gamma_cols[*c]).sum::<f64>() / stroke_count as f64;
        let linear_stroke_mean =
            stroke_cols.iter().map(|c| linear_cols[*c]).sum::<f64>() / stroke_count as f64;
        let gamma_stroke_min = stroke_cols
            .iter()
            .map(|c| gamma_cols[*c])
            .fold(f64::INFINITY, f64::min);
        let linear_stroke_min = stroke_cols
            .iter()
            .map(|c| linear_cols[*c])
            .fold(f64::INFINITY, f64::min);

        println!("=== V13 linear-light downscale measurement (supervisor decision input) ===");
        println!(
            "(a) CHECKER 1px b/w, {}x{} -> {}x{} (4x, pyramid Lanczos3):",
            checker_source[0], checker_source[1], checker_target[0], checker_target[1]
        );
        println!(
            "      gamma  mean = {checker_gamma_mean:7.3} sRGB   (expect ~127-140; gamma-space average)"
        );
        println!(
            "      linear mean = {checker_linear_mean:7.3} sRGB   (expect ~180-196; physically-correct ~188)"
        );
        println!(
            "(b) LINE-ART bg235 / 2px strokes, {}x{} -> {}x{} (2x, single-pass Lanczos3):",
            line_source[0], line_source[1], line_target[0], line_target[1]
        );
        println!("      overall mean:      gamma = {line_gamma_mean:7.3}   linear = {line_linear_mean:7.3}");
        println!("      stroke-region mean: gamma = {gamma_stroke_mean:7.3}   linear = {linear_stroke_mean:7.3}   (higher linear = thinner/lighter strokes)");
        println!("      stroke-region min:  gamma = {gamma_stroke_min:7.3}   linear = {linear_stroke_min:7.3}");
        println!("========================================================================");

        // (c) Both paths: alpha untouched, non-blank.
        for (label, pixels) in [
            ("checker gamma", &checker_gamma),
            ("checker linear", &checker_linear),
            ("line-art gamma", &line_gamma),
            ("line-art linear", &line_linear),
        ] {
            assert!(all_alpha_opaque(pixels), "{label}: alpha must stay 255");
            assert!(any_nonblank(pixels), "{label}: output must be non-blank");
        }

        // (a) Checker means bracket the gamma vs linear-light split.
        assert!(
            (127.0..=140.0).contains(&checker_gamma_mean),
            "checker gamma mean {checker_gamma_mean:.3} outside [127,140]"
        );
        assert!(
            (180.0..=196.0).contains(&checker_linear_mean),
            "checker linear mean {checker_linear_mean:.3} outside [180,196]"
        );

        // (b) Sanity only: linear lightens the stroke region (>= gamma), and every
        // number stays in range.
        assert!(
            linear_stroke_mean >= gamma_stroke_mean,
            "linear stroke-region mean {linear_stroke_mean:.3} < gamma {gamma_stroke_mean:.3}"
        );
        for value in [
            line_gamma_mean,
            line_linear_mean,
            gamma_stroke_mean,
            linear_stroke_mean,
        ] {
            assert!(
                (0.0..=255.0).contains(&value),
                "line-art measurement {value:.3} out of [0,255]"
            );
        }
    });
}
