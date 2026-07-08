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

/// Base keyboard step (fraction of the viewport height): the fallback when no
/// cut gives a better answer, and the walking step within a cut taller than the
/// viewport. Slightly shorter than the snap-off `0.9` for comfortable overlap.
pub(in crate::app) const STRIP_SNAP_BASE_STEP_FRAC: f32 = 0.85;
/// Points of breathing room left above a taller-than-viewport cut's top when
/// top-aligning it, so the cut edge is not flush against the viewport edge.
const STRIP_SNAP_LANDING_PAD: f32 = 12.0;

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

/// Two content spans this close (points) merge into one panel: page seams and
/// rounding produce sub-pixel gaps between spans that are really one cut.
const PANEL_MERGE_EPSILON: f32 = 0.5;
/// A panel counts as "still being read" while more than this fraction of a
/// viewport of it remains off-screen in the step direction; the step then walks
/// within the cut instead of jumping to the next one.
const PANEL_REMAINDER_FRAC: f32 = 0.15;
/// A single panel-slideshow step never travels further than this many viewports,
/// so a pathological gap cannot fling the reader across the book in one press.
pub(in crate::app) const PANEL_STEP_MAX_VIEWPORTS: f32 = 3.0;

/// One page as the panel walk sees it. `analyzed == false` means no decoded
/// pixels were available yet: the page is treated as full content (the
/// conservative estimate) but its span is an ASSUMED cut, never a landing.
#[derive(Debug, Clone, PartialEq)]
pub(in crate::app) struct PanelPage {
    pub top: f32,
    pub height: f32,
    pub gutters: Vec<(f32, f32)>,
    pub analyzed: bool,
}

/// One content span ("cut") in delta space. `assumed` marks spans that include
/// any unanalyzed page: they behave as content for walking and for the no-loss
/// clamp, but are not landing targets.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(in crate::app) struct PanelSpan {
    pub top: f32,
    pub bottom: f32,
    pub assumed: bool,
}

/// Content spans ("cuts") in delta space: the complement of each page's gutters,
/// merged across touching boundaries — art that runs over a page seam with no
/// gutter on either side is one panel. Pages may arrive unsorted. A page whose
/// gutters cover it entirely (a blank/transition page) contributes no span,
/// i.e. it becomes part of the gap between cuts.
pub(in crate::app) fn collect_panels(pages: &[PanelPage]) -> Vec<PanelSpan> {
    let mut sorted: Vec<&PanelPage> = pages.iter().collect();
    sorted.sort_by(|a, b| a.top.partial_cmp(&b.top).unwrap_or(Ordering::Equal));

    let mut spans: Vec<PanelSpan> = Vec::new();
    let push_span = |spans: &mut Vec<PanelSpan>, top: f32, bottom: f32, assumed: bool| {
        if bottom - top <= f32::EPSILON {
            return;
        }
        if let Some(last) = spans.last_mut() {
            if top - last.bottom <= PANEL_MERGE_EPSILON {
                last.bottom = last.bottom.max(bottom);
                last.assumed |= assumed;
                return;
            }
        }
        spans.push(PanelSpan {
            top,
            bottom,
            assumed,
        });
    };

    for page in sorted {
        let mut cursor = page.top;
        for (start_frac, end_frac) in page.gutters.iter().copied() {
            let gutter_top = page.top + start_frac * page.height;
            let gutter_bottom = page.top + end_frac * page.height;
            push_span(&mut spans, cursor, gutter_top, !page.analyzed);
            cursor = cursor.max(gutter_bottom);
        }
        push_span(&mut spans, cursor, page.top + page.height, !page.analyzed);
    }
    spans
}

/// A step prefers to advance up to this fraction of a viewport (with the
/// remaining 10% as reading overlap): among the cuts it could land on, the
/// furthest within this travel wins, so a stack of small dialogue-bubble cuts
/// is crossed a screenful per press instead of one bubble per press.
const PANEL_STEP_CAP_FRAC: f32 = 0.9;
/// A cut at least this fraction of the viewport tall is a MAJOR cut (an art
/// panel rather than a speech bubble). Major cuts win the landing over any
/// small cut in reach, so bubbles never displace a panel from the center.
const PANEL_MAJOR_MIN_FRAC: f32 = 0.25;
/// Major cuts may be reached up to a full viewport of travel — the exact
/// no-content-loss limit — so a bubble just inside the normal cap cannot steal
/// the landing from a panel just beyond it.
const PANEL_MAJOR_CAP_FRAC: f32 = 1.0;

/// Signed scroll delta for one panel-slideshow step, or `None` when no panel
/// gives a better answer than the caller's plain step (no spans collected, or
/// no next cut inside the collected range).
///
/// Delta space: 0.0 = current viewport top, the viewport spans `[0, viewport_h]`.
/// Forward semantics: while the cut under the viewport center still has more
/// than [`PANEL_REMAINDER_FRAC`] of a viewport unread below, walk within it by
/// `walk_step` (an oversized cut reads like today). Otherwise land on a cut —
/// centered, or top-aligned with a small pad when taller than the viewport —
/// choosing the FURTHEST landing within [`PANEL_STEP_CAP_FRAC`] of a viewport
/// so dense small cuts are crossed a screenful at a time (everything skipped
/// past stays visible above the landing), and falling through to the nearest
/// landing beyond the cap when none is inside it (the gap skip), capped at
/// [`PANEL_STEP_MAX_VIEWPORTS`]. Backward is the mirror image. Blank pages and
/// inter-cut whitespace contribute no spans, so a step glides across them.
pub(in crate::app) fn panel_step_delta(
    viewport_h: f32,
    walk_step: f32,
    panels: &[PanelSpan],
    forward: bool,
) -> Option<f32> {
    if panels.is_empty() || viewport_h <= 0.0 {
        return None;
    }
    let center = viewport_h * 0.5;
    let remainder_slack = viewport_h * PANEL_REMAINDER_FRAC;
    let caps = LandingCaps {
        minor_cap: viewport_h * PANEL_STEP_CAP_FRAC,
        major_cap: viewport_h * PANEL_MAJOR_CAP_FRAC,
        major_min_height: viewport_h * PANEL_MAJOR_MIN_FRAC,
    };
    let current = panels
        .iter()
        .copied()
        .find(|span| span.top <= center && center < span.bottom);

    // Landing delta for one cut: center it, or top-align an oversized one.
    let land_forward = |span: PanelSpan| {
        if span.bottom - span.top <= viewport_h {
            (span.top + span.bottom) / 2.0 - center
        } else {
            span.top - STRIP_SNAP_LANDING_PAD
        }
    };
    let land_backward = |span: PanelSpan| {
        if span.bottom - span.top <= viewport_h {
            (span.top + span.bottom) / 2.0 - center
        } else {
            // Reading backward into a tall cut: land on its end.
            span.bottom - viewport_h + STRIP_SNAP_LANDING_PAD
        }
    };

    let delta = if forward {
        if let Some(span) = current {
            let remaining = span.bottom - viewport_h;
            if remaining > remainder_slack {
                // Still reading a (possibly assumed) tall cut: never step past
                // its end.
                return Some(walk_step.min(remaining));
            }
        }
        let from = current.map_or(center, |span| span.bottom);
        // Only VERIFIED cuts are landing targets; an assumed span (page with no
        // decoded pixels yet) is walked, not framed.
        let landings = panels
            .iter()
            .copied()
            .filter(|span| !span.assumed && span.top >= from - PANEL_MERGE_EPSILON)
            .map(|span| (land_forward(span), span.bottom - span.top))
            .filter(|(delta, _)| *delta >= 1.0);
        let mut delta = pick_landing(landings, caps)?;
        // Never fly past unverified content: clamp to the first assumed span
        // ahead (a plain step's reach past it stays lossless).
        if let Some(assumed_top) = panels
            .iter()
            .filter(|span| span.assumed && span.top >= from - PANEL_MERGE_EPSILON)
            .map(|span| span.top)
            .min_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal))
        {
            delta = delta.min(assumed_top.max(walk_step));
        }
        delta
    } else {
        if let Some(span) = current {
            let remaining = -span.top;
            if remaining > remainder_slack {
                return Some(-walk_step.min(remaining));
            }
        }
        let from = current.map_or(center, |span| span.top);
        let landings = panels
            .iter()
            .copied()
            .filter(|span| !span.assumed && span.bottom <= from + PANEL_MERGE_EPSILON)
            .map(|span| (land_backward(span), span.bottom - span.top))
            .filter(|(delta, _)| *delta <= -1.0)
            .map(|(delta, height)| (-delta, height));
        let mut magnitude = pick_landing(landings, caps)?;
        if let Some(assumed_reach) = panels
            .iter()
            .filter(|span| span.assumed && span.bottom <= from + PANEL_MERGE_EPSILON)
            .map(|span| -span.bottom)
            .min_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal))
        {
            magnitude = magnitude.min(assumed_reach.max(walk_step));
        }
        -magnitude
    };
    Some(delta.clamp(
        -PANEL_STEP_MAX_VIEWPORTS * viewport_h,
        PANEL_STEP_MAX_VIEWPORTS * viewport_h,
    ))
}

/// Travel limits for [`pick_landing`], all in points.
struct LandingCaps {
    minor_cap: f32,
    major_cap: f32,
    major_min_height: f32,
}

/// Choose among positive `(landing, cut_height)` pairs, in priority order:
/// the furthest MAJOR cut within `major_cap` (an art panel always beats the
/// speech bubbles around it), else the furthest minor cut within `minor_cap`
/// (a pure dialogue stretch advances a screenful landing on a bubble), else
/// the nearest landing beyond the caps (the gap skip), else `None`.
fn pick_landing(landings: impl Iterator<Item = (f32, f32)>, caps: LandingCaps) -> Option<f32> {
    let mut major_within: Option<f32> = None;
    let mut minor_within: Option<f32> = None;
    let mut nearest_beyond: Option<f32> = None;
    for (landing, height) in landings {
        let major = height >= caps.major_min_height;
        if major && landing <= caps.major_cap {
            if major_within.is_none_or(|best| landing > best) {
                major_within = Some(landing);
            }
        } else if !major && landing <= caps.minor_cap {
            if minor_within.is_none_or(|best| landing > best) {
                minor_within = Some(landing);
            }
        } else if nearest_beyond.is_none_or(|nearest| landing < nearest) {
            nearest_beyond = Some(landing);
        }
    }
    major_within.or(minor_within).or(nearest_beyond)
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

    /// Analyzed page shorthand for panel tests.
    fn page(top: f32, height: f32, gutters: Vec<(f32, f32)>) -> PanelPage {
        PanelPage {
            top,
            height,
            gutters,
            analyzed: true,
        }
    }

    /// Unanalyzed (assumed full-content) page shorthand.
    fn assumed_page(top: f32, height: f32) -> PanelPage {
        PanelPage {
            top,
            height,
            gutters: Vec::new(),
            analyzed: false,
        }
    }

    /// Verified cut shorthand for step tests.
    fn span(top: f32, bottom: f32) -> PanelSpan {
        PanelSpan {
            top,
            bottom,
            assumed: false,
        }
    }

    fn assumed_span(top: f32, bottom: f32) -> PanelSpan {
        PanelSpan {
            top,
            bottom,
            assumed: true,
        }
    }

    #[test]
    fn panels_are_the_gutter_complement_and_merge_across_seams() {
        // Page A [0,1000]: gutter at 40%-60% -> cuts [0,400] and [600,1000].
        // Page B [1000,2000]: no gutters (art from its very top) -> its span
        // touches page A's trailing span at the seam and merges into one cut.
        let pages = vec![
            page(0.0, 1000.0, vec![(0.4, 0.6)]),
            page(1000.0, 1000.0, vec![]),
        ];
        assert_eq!(
            collect_panels(&pages),
            vec![span(0.0, 400.0), span(600.0, 2000.0)]
        );
    }

    #[test]
    fn fully_blank_page_becomes_part_of_the_gap() {
        // Page B is one full-page gutter (a blank transition page): the cuts on
        // either side stay separate with the whole page as the gap.
        let pages = vec![
            page(0.0, 1000.0, vec![(0.8, 1.0)]),
            page(1000.0, 1000.0, vec![(0.0, 1.0)]),
            page(2000.0, 1000.0, vec![(0.0, 0.2)]),
        ];
        assert_eq!(
            collect_panels(&pages),
            vec![span(0.0, 800.0), span(2200.0, 3000.0)]
        );
    }

    #[test]
    fn panels_sort_unsorted_page_input() {
        // collect_band_pages emits anchor, then downward, then upward pages.
        let pages = vec![
            page(1000.0, 1000.0, vec![(0.0, 1.0)]),
            page(0.0, 1000.0, vec![(0.8, 1.0)]),
        ];
        assert_eq!(collect_panels(&pages), vec![span(0.0, 800.0)]);
    }

    #[test]
    fn unanalyzed_page_yields_an_assumed_span_and_taints_merges() {
        // The unanalyzed page is one assumed full-content span; merging with the
        // analyzed neighbour's touching span keeps the assumed taint.
        let pages = vec![page(0.0, 1000.0, vec![]), assumed_page(1000.0, 1000.0)];
        assert_eq!(collect_panels(&pages), vec![assumed_span(0.0, 2000.0)]);
    }

    #[test]
    fn step_centers_the_next_cut() {
        // Viewport 800 (center 400). Current cut [200,600] is centered-ish and
        // fully visible; the next cut [900,1100] (height 200) should center at
        // 400 -> its center 1000 moves to 400 -> delta 600.
        let panels = vec![span(200.0, 600.0), span(900.0, 1100.0)];
        assert_eq!(panel_step_delta(800.0, 680.0, &panels, true), Some(600.0));
    }

    #[test]
    fn step_skips_a_long_gap_in_one_press_but_is_capped() {
        // Next cut far below: centering it needs 1900; cap is 3 viewports = 2400
        // (not hit). A pathological 10k gap is clamped to the cap.
        let panels = vec![span(200.0, 600.0), span(2200.0, 2400.0)];
        assert_eq!(panel_step_delta(800.0, 680.0, &panels, true), Some(1900.0));
        let far = vec![span(200.0, 600.0), span(10_000.0, 10_200.0)];
        assert_eq!(panel_step_delta(800.0, 680.0, &far, true), Some(2400.0));
    }

    #[test]
    fn step_walks_within_a_cut_taller_than_the_viewport() {
        // One huge cut [0, 5000] under an 800 viewport: walk by the base step,
        // and near its end never step past the cut bottom.
        let panels = vec![span(0.0, 5000.0)];
        assert_eq!(panel_step_delta(800.0, 680.0, &panels, true), Some(680.0));
        let near_end = vec![span(-3900.0, 1100.0)];
        assert_eq!(panel_step_delta(800.0, 680.0, &near_end, true), Some(300.0));
    }

    #[test]
    fn step_top_aligns_a_next_cut_taller_than_the_viewport() {
        // Next cut [900, 2900] (2000 tall > 800 viewport): land its top at the
        // viewport top minus the pad instead of centering.
        let panels = vec![span(200.0, 600.0), span(900.0, 2900.0)];
        assert_eq!(panel_step_delta(800.0, 680.0, &panels, true), Some(888.0));
    }

    #[test]
    fn step_crosses_dense_small_cuts_a_screenful_at_a_time() {
        // A dialogue stack of small bubble-cuts: the furthest landing within
        // the 0.9-viewport cap (720) wins - 550, centering the second bubble
        // with the first still visible above - instead of crawling one bubble
        // (350) per press.
        let panels = vec![
            span(200.0, 600.0),
            span(700.0, 800.0),
            span(900.0, 1000.0),
            span(1100.0, 1200.0),
            span(1500.0, 1600.0),
        ];
        assert_eq!(panel_step_delta(800.0, 680.0, &panels, true), Some(550.0));
    }

    #[test]
    fn major_cut_beats_nearer_bubbles_for_the_landing() {
        // By distance alone the second bubble (landing 430) would win, but the
        // big art panel (600 tall, landing 800 = exactly the no-loss major cap)
        // takes the landing: bubbles never displace a panel from the center.
        let panels = vec![
            span(200.0, 600.0),  // current cut
            span(650.0, 750.0),  // bubble, landing 300
            span(780.0, 880.0),  // bubble, landing 430
            span(900.0, 1500.0), // art panel, landing 800
        ];
        assert_eq!(panel_step_delta(800.0, 680.0, &panels, true), Some(800.0));
    }

    #[test]
    fn assumed_span_is_never_a_landing_target() {
        // The only thing ahead is an unverified page: no landing answer, the
        // caller takes the plain step (which walks into it losslessly).
        let panels = vec![span(200.0, 600.0), assumed_span(900.0, 2300.0)];
        assert_eq!(panel_step_delta(800.0, 680.0, &panels, true), None);
    }

    #[test]
    fn gap_skip_never_flies_past_unverified_content() {
        // A verified cut lies beyond an unverified page: the skip is clamped to
        // the assumed page's top so nothing unseen is jumped over.
        let panels = vec![
            span(200.0, 600.0),
            assumed_span(1200.0, 2600.0),
            span(2800.0, 3000.0),
        ];
        assert_eq!(panel_step_delta(800.0, 680.0, &panels, true), Some(1200.0));
    }

    #[test]
    fn step_falls_back_when_no_cut_answers() {
        assert_eq!(panel_step_delta(800.0, 680.0, &[], true), None);
        // Only cuts behind the center going forward -> None.
        let behind = vec![span(-500.0, -100.0)];
        assert_eq!(panel_step_delta(800.0, 680.0, &behind, true), None);
    }

    #[test]
    fn step_backward_mirrors_forward() {
        // Previous cut [-700,-500] (height 200): center it -> its center -600
        // moves to 400 -> delta -1000.
        let panels = vec![span(-700.0, -500.0), span(200.0, 600.0)];
        assert_eq!(
            panel_step_delta(800.0, 680.0, &panels, false),
            Some(-1000.0)
        );
        // Reading backward while the current cut still extends above: walk up.
        let tall = vec![span(-3000.0, 1100.0)];
        assert_eq!(panel_step_delta(800.0, 680.0, &tall, false), Some(-680.0));
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
