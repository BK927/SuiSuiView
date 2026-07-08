//! Pure panel-boundary math for the vertical strip's keyboard "panel snap".
//!
//! Webtoon pages stack panels separated by tall uniform-colour gutters. A
//! keyboard viewport-step should land the next panel's top at the viewport top
//! instead of taking a blind fraction-of-a-viewport jump. This module holds the
//! estimate-stable, testable pieces: gutter detection from decoded pixels, the
//! delta-space walk that turns nearby pages into candidate landing offsets, and
//! the nearest-candidate selection. The app driver in `view.rs` supplies the
//! pixels/heights and feeds the chosen delta into the smooth-scroll animator.

use std::cmp::Ordering;

/// Base keyboard step when snap is enabled, as a fraction of the viewport
/// height. Slightly shorter than the snap-off `0.9` so a snap that misses still
/// leaves a comfortable panel overlap.
pub(in crate::app) const STRIP_SNAP_BASE_STEP_FRAC: f32 = 0.85;
/// Half-width of the search window around the raw target, as a fraction of the
/// viewport height. A candidate within this of the raw target wins.
pub(in crate::app) const STRIP_SNAP_WINDOW_FRAC: f32 = 0.25;
/// Points of breathing room left above the next panel's top when landing on a
/// gutter, so the panel edge is not flush against the viewport top.
pub(in crate::app) const STRIP_SNAP_LANDING_PAD: f32 = 12.0;

/// A sampled row counts as uniform when its luminance spread (max-min over the
/// sampled pixels, 0..=255) is at most this.
const ROW_UNIFORM_SPREAD: u32 = 14;
/// A uniform row joins the current run only while the spread of per-row means
/// across the whole run stays within this. Keeps a black band from merging with
/// an adjoining white band, and breaks a smooth gradient into sub-gutter pieces.
const RUN_MEAN_DELTA: u32 = 10;
/// Absolute floor on a gutter's height in pixels.
const GUTTER_MIN_PX: f32 = 24.0;
/// A gutter must also span at least this fraction of the page height.
const GUTTER_MIN_FRAC: f32 = 0.015;

/// Detect uniform-colour gutter bands in a decoded page, returned as sorted,
/// non-overlapping `(start_frac, end_frac)` ranges of the full page height.
///
/// `pixels` is the raw decoded buffer with `bytes_per_pixel` interleaved
/// channels (4 = RGBA, 1 = luma); luminance is a simple integer approximation of
/// the first three channels (or the single channel for luma). Rows are sampled
/// at a stride and columns every few pixels, so the scan is sub-millisecond even
/// on a tall page. Noise and gradients naturally yield few or no gutters; the
/// caller treats "no gutters" as "just use the page top".
pub(in crate::app) fn detect_gutter_rows(
    pixels: &[u8],
    bytes_per_pixel: usize,
    width: usize,
    height: usize,
) -> Vec<(f32, f32)> {
    if width == 0 || height == 0 || bytes_per_pixel == 0 {
        return Vec::new();
    }
    if pixels.len() < width * height * bytes_per_pixel {
        return Vec::new();
    }

    let row_stride = (height / 512).max(1);
    let col_step = width.clamp(1, 8);
    let min_gutter_px = (GUTTER_MIN_FRAC * height as f32).max(GUTTER_MIN_PX);

    let mut runs: Vec<(usize, usize)> = Vec::new();
    let mut run_start: Option<usize> = None;
    let mut run_last = 0usize;
    let mut run_mean_min = 0u32;
    let mut run_mean_max = 0u32;

    let mut y = 0;
    while y < height {
        let (uniform, mean) = sample_row(pixels, bytes_per_pixel, width, col_step, y);
        if uniform {
            match run_start {
                Some(_) => {
                    let next_min = run_mean_min.min(mean);
                    let next_max = run_mean_max.max(mean);
                    if next_max - next_min <= RUN_MEAN_DELTA {
                        run_last = y;
                        run_mean_min = next_min;
                        run_mean_max = next_max;
                    } else {
                        push_run(
                            &mut runs,
                            run_start.take().unwrap(),
                            run_last,
                            row_stride,
                            height,
                            min_gutter_px,
                        );
                        run_start = Some(y);
                        run_last = y;
                        run_mean_min = mean;
                        run_mean_max = mean;
                    }
                }
                None => {
                    run_start = Some(y);
                    run_last = y;
                    run_mean_min = mean;
                    run_mean_max = mean;
                }
            }
        } else if let Some(start) = run_start.take() {
            push_run(
                &mut runs,
                start,
                run_last,
                row_stride,
                height,
                min_gutter_px,
            );
        }
        y += row_stride;
    }
    if let Some(start) = run_start.take() {
        push_run(
            &mut runs,
            start,
            run_last,
            row_stride,
            height,
            min_gutter_px,
        );
    }

    let height = height as f32;
    runs.into_iter()
        .map(|(start, end)| (start as f32 / height, end as f32 / height))
        .collect()
}

/// Sample one row every `col_step` pixels; return `(is_uniform, mean_luma)`.
fn sample_row(
    pixels: &[u8],
    bytes_per_pixel: usize,
    width: usize,
    col_step: usize,
    y: usize,
) -> (bool, u32) {
    let row_start = y * width * bytes_per_pixel;
    let mut min = 255u32;
    let mut max = 0u32;
    let mut sum = 0u32;
    let mut count = 0u32;
    let mut x = 0;
    while x < width {
        let i = row_start + x * bytes_per_pixel;
        let luma = if bytes_per_pixel >= 3 {
            (pixels[i] as u32 + pixels[i + 1] as u32 + pixels[i + 2] as u32) / 3
        } else {
            pixels[i] as u32
        };
        min = min.min(luma);
        max = max.max(luma);
        sum += luma;
        count += 1;
        x += col_step;
    }
    let mean = if count > 0 { sum / count } else { 0 };
    (max - min <= ROW_UNIFORM_SPREAD, mean)
}

/// Emit a run of uniform rows as a pixel band `[start, end)` if it is tall
/// enough. The band extends one stride past the last sampled row so a gutter
/// found at a coarse stride is not clipped short.
fn push_run(
    runs: &mut Vec<(usize, usize)>,
    start: usize,
    last: usize,
    row_stride: usize,
    height: usize,
    min_gutter_px: f32,
) {
    let end = (last + row_stride).min(height);
    if (end - start) as f32 >= min_gutter_px {
        runs.push((start, end));
    }
}

/// Nearest candidate to `raw_target_px` within `±window_px`, else `raw_target_px`
/// unchanged. On an exact distance tie the earlier candidate in the slice wins
/// (candidates are built anchor-outward, so the one nearer the viewport wins).
pub(in crate::app) fn snap_step_target(
    raw_target_px: f32,
    candidates_px: &[f32],
    window_px: f32,
) -> f32 {
    candidates_px
        .iter()
        .copied()
        .filter(|candidate| (candidate - raw_target_px).abs() <= window_px)
        .min_by(|a, b| {
            (a - raw_target_px)
                .abs()
                .partial_cmp(&(b - raw_target_px).abs())
                .unwrap_or(Ordering::Equal)
        })
        .unwrap_or(raw_target_px)
}

/// Cap on how many nearby pages the delta-space walk collects, so a run of
/// zero-height estimates cannot spin the loop.
const MAX_BAND_PAGES: usize = 32;

/// Walk pages near the anchor and return those whose vertical span overlaps the
/// candidate band `[lo, hi]`, each as `(index, top_delta, height)` where
/// `top_delta` is the page top's offset from the current viewport top (delta
/// space: 0.0 = viewport top; positive = below). Working relative to the
/// viewport top keeps every offset local, so far-page height estimates never
/// shift a candidate.
pub(in crate::app) fn collect_band_pages(
    anchor_index: usize,
    offset_frac: f32,
    lo: f32,
    hi: f32,
    page_count: usize,
    height_of: &impl Fn(usize) -> f32,
) -> Vec<(usize, f32, f32)> {
    let mut out = Vec::new();
    if page_count == 0 || lo > hi {
        return out;
    }
    let anchor_index = anchor_index.min(page_count - 1);
    let anchor_height = height_of(anchor_index);
    let anchor_top = -offset_frac * anchor_height;
    let overlaps = |top: f32, height: f32| top < hi && top + height > lo;

    if overlaps(anchor_top, anchor_height) {
        out.push((anchor_index, anchor_top, anchor_height));
    }

    // Downward from the anchor: page tops increase, so stop once a top clears hi.
    let mut top = anchor_top + anchor_height;
    let mut index = anchor_index + 1;
    while index < page_count && top < hi && out.len() < MAX_BAND_PAGES {
        let height = height_of(index);
        if overlaps(top, height) {
            out.push((index, top, height));
        }
        top += height;
        index += 1;
    }

    // Upward from the anchor: page bottoms decrease, so stop once one clears lo.
    let mut bottom = anchor_top;
    let mut index = anchor_index;
    while index > 0 && bottom > lo && out.len() < MAX_BAND_PAGES {
        let above = index - 1;
        let height = height_of(above);
        let top = bottom - height;
        if overlaps(top, height) {
            out.push((above, top, height));
        }
        bottom = top;
        index = above;
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build an RGBA image whose row `y` is filled by `color(y)` across `width`.
    fn rgba_image(width: usize, height: usize, color: impl Fn(usize) -> [u8; 4]) -> Vec<u8> {
        let mut pixels = Vec::with_capacity(width * height * 4);
        for y in 0..height {
            let [r, g, b, a] = color(y);
            for _ in 0..width {
                pixels.extend_from_slice(&[r, g, b, a]);
            }
        }
        pixels
    }

    /// A row that varies strongly across x (never uniform under sampling).
    fn busy_row(width: usize, y: usize) -> Vec<u8> {
        let mut row = Vec::with_capacity(width * 4);
        for x in 0..width {
            let v = (((x * 37 + y * 11) % 256) as u8).max(1);
            row.extend_from_slice(&[v, v, v, 255]);
        }
        row
    }

    /// Two busy panels separated by a solid band of `band_color` over
    /// `[band_start, band_end)`; the rest is busy content (never a gutter).
    fn two_panel_image(
        width: usize,
        height: usize,
        band_start: usize,
        band_end: usize,
        band_color: [u8; 4],
    ) -> Vec<u8> {
        let mut pixels = Vec::with_capacity(width * height * 4);
        for y in 0..height {
            if y >= band_start && y < band_end {
                for _ in 0..width {
                    pixels.extend_from_slice(&band_color);
                }
            } else {
                pixels.extend_from_slice(&busy_row(width, y));
            }
        }
        pixels
    }

    #[test]
    fn detects_interior_white_gutter_between_panels() {
        let (width, height) = (64, 1000);
        let pixels = two_panel_image(width, height, 450, 480, [255, 255, 255, 255]);
        let gutters = detect_gutter_rows(&pixels, 4, width, height);
        assert_eq!(
            gutters.len(),
            1,
            "one interior gutter expected: {gutters:?}"
        );
        let (start, end) = gutters[0];
        assert!((start - 0.45).abs() < 0.02, "start {start}");
        assert!((end - 0.48).abs() < 0.03, "end {end}");
    }

    #[test]
    fn detects_gutter_on_inverted_dark_page() {
        // Dark separator between light-busy panels: uniformity is polarity-blind.
        let (width, height) = (64, 800);
        let pixels = two_panel_image(width, height, 300, 360, [0, 0, 0, 255]);
        let gutters = detect_gutter_rows(&pixels, 4, width, height);
        assert_eq!(gutters.len(), 1, "{gutters:?}");
        assert!((gutters[0].0 - 0.375).abs() < 0.03);
    }

    #[test]
    fn noisy_flat_band_within_tolerance_is_still_a_gutter() {
        // A gutter whose pixels jitter within the spread tolerance still reads
        // as uniform. Row luma stays within ROW_UNIFORM_SPREAD.
        let (width, height) = (64, 800);
        let mut pixels = Vec::with_capacity(width * height * 4);
        for y in 0..height {
            if (300..360).contains(&y) {
                for x in 0..width {
                    // Values in 120..=131: spread 11 <= 14 across the row.
                    let v = (120 + ((x + y) % 12)) as u8;
                    pixels.extend_from_slice(&[v, v, v, 255]);
                }
            } else {
                pixels.extend_from_slice(&busy_row(width, y));
            }
        }
        let gutters = detect_gutter_rows(&pixels, 4, width, height);
        assert_eq!(gutters.len(), 1, "{gutters:?}");
    }

    #[test]
    fn fully_uniform_page_is_one_full_height_gutter() {
        let (width, height) = (32, 600);
        let pixels = rgba_image(width, height, |_| [128, 128, 128, 255]);
        let gutters = detect_gutter_rows(&pixels, 4, width, height);
        assert_eq!(gutters.len(), 1, "{gutters:?}");
        assert!((gutters[0].0 - 0.0).abs() < 1e-6);
        assert!((gutters[0].1 - 1.0).abs() < 1e-6);
    }

    #[test]
    fn steep_gradient_page_has_no_gutter() {
        // One grey level per row: any 24px run spans >10 levels, so runs break
        // below the minimum height and nothing survives.
        let (width, height) = (32, 256);
        let pixels = rgba_image(width, height, |y| {
            let v = (y % 256) as u8;
            [v, v, v, 255]
        });
        let gutters = detect_gutter_rows(&pixels, 4, width, height);
        assert!(gutters.is_empty(), "{gutters:?}");
    }

    #[test]
    fn tiny_uniform_gap_below_min_height_is_ignored() {
        let (width, height) = (64, 800);
        // A 10px uniform band is well under max(24, 1.5%*800 = 12) = 24px.
        let pixels = two_panel_image(width, height, 400, 410, [255, 255, 255, 255]);
        let gutters = detect_gutter_rows(&pixels, 4, width, height);
        assert!(gutters.is_empty(), "{gutters:?}");
    }

    #[test]
    fn luma_buffer_detects_gutter_like_rgba() {
        // Grayscale webtoons are cached as 1 byte/px; the luma path must agree.
        let (width, height) = (64, 800);
        let mut pixels = Vec::with_capacity(width * height);
        for y in 0..height {
            if (300..360).contains(&y) {
                pixels.extend(std::iter::repeat_n(255u8, width));
            } else {
                for x in 0..width {
                    pixels.push((((x * 37 + y * 11) % 256) as u8).max(1));
                }
            }
        }
        let gutters = detect_gutter_rows(&pixels, 1, width, height);
        assert_eq!(gutters.len(), 1, "{gutters:?}");
        assert!((gutters[0].0 - 0.375).abs() < 0.03);
    }

    #[test]
    fn detect_rejects_degenerate_and_short_buffers() {
        assert!(detect_gutter_rows(&[], 4, 0, 0).is_empty());
        assert!(detect_gutter_rows(&[0, 0, 0], 4, 10, 10).is_empty());
    }

    #[test]
    fn snap_picks_nearest_candidate_inside_window() {
        // Raw target 100, window 30: 90 (d=10) beats 125 (d=25); 200 is outside.
        assert_eq!(snap_step_target(100.0, &[90.0, 125.0, 200.0], 30.0), 90.0);
    }

    #[test]
    fn snap_returns_raw_when_no_candidate_in_window() {
        assert_eq!(snap_step_target(100.0, &[40.0, 200.0], 30.0), 100.0);
        assert_eq!(snap_step_target(100.0, &[], 30.0), 100.0);
    }

    #[test]
    fn snap_tie_prefers_earlier_candidate() {
        // Both 20 away; the first listed (nearer the viewport top) wins.
        assert_eq!(snap_step_target(100.0, &[80.0, 120.0], 30.0), 80.0);
    }

    #[test]
    fn snap_is_symmetric_for_backward_targets() {
        assert_eq!(snap_step_target(-100.0, &[-90.0, -130.0], 30.0), -90.0);
    }

    #[test]
    fn band_collects_pages_overlapping_a_forward_window() {
        let height_of = |_: usize| 100.0;
        // Anchor page 2 at offset 0.5: viewport top is 50px into page 2, so
        // page 2 top is at -50. Band [85, 135] (raw 110, window 25) overlaps
        // page 3 (top 50) and page 4 (top 150)? page4 top 150 >= 135 -> excluded.
        let band = collect_band_pages(2, 0.5, 85.0, 135.0, 10, &height_of);
        let indices: Vec<usize> = band.iter().map(|(i, _, _)| *i).collect();
        assert_eq!(indices, vec![3]);
        assert_eq!(band[0].1, 50.0);
        assert_eq!(band[0].2, 100.0);
    }

    #[test]
    fn band_collects_pages_overlapping_a_backward_window() {
        let height_of = |_: usize| 100.0;
        // Anchor page 5 at offset 0.0: page 5 top at 0. Band [-260, -210]
        // overlaps page 3 (top -200..-... no) — page 3 spans [-200,-100]? tops:
        // page4 -100, page3 -200, page2 -300. [-260,-210] overlaps page 2
        // ([-300,-200], bottom -200 > -260) and page 3 ([-200,-100], top -200 <
        // -210? no). So only page 2.
        let band = collect_band_pages(5, 0.0, -260.0, -210.0, 10, &height_of);
        let indices: Vec<usize> = band.iter().map(|(i, _, _)| *i).collect();
        assert_eq!(indices, vec![2]);
        assert_eq!(band[0].1, -300.0);
    }
}
