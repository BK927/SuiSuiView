//! Anchor-based virtualized vertical strip: pure layout/scroll math plus the
//! background page-dimension prescan worker.
//!
//! The layout/scroll math below is the pure foundation for the webtoon
//! continuous-vertical-scroll `ViewMode`; the app-side methods that drive it
//! (paint, pointer, scroll, jump) live in `view.rs`, and the header prescan
//! worker in `scan.rs` feeds the `strip_dim_hints` fallback.

mod scan;
mod view;
pub(in crate::app) use scan::StripDimScanWorker;

use crate::core::source::PageId;
use crate::core::worker::NavigationDirection;
use egui::Rect;
use std::time::{Duration, Instant};

/// Where a virtualized strip is scrolled to: the topmost visible page plus how
/// far the viewport top has slid past that page's top.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(in crate::app) struct StripAnchor {
    pub page_id: PageId,
    /// Fraction of the anchor page's display height scrolled above the viewport
    /// top, in `[0, 1)`. Kept in range by [`scroll_by`]'s renormalization.
    pub offset_frac: f32,
}

/// One laid-out page: its index in the current source snapshot and the screen
/// rect it occupies (full viewport width, fit-width).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(in crate::app) struct StripPlacement {
    pub index: usize,
    pub rect: Rect,
}

/// Known page pixel dimensions, by provenance. `page_metrics` (post-EXIF,
/// authoritative) yields `Exact`; the header prescan yields `Hint`; anything not
/// yet measured is `Unknown` and falls back to the running median.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app) enum StripPageDims {
    Exact([u32; 2]),
    Hint([u32; 2]),
    Unknown,
}

/// Fit-width display height for a page: `viewport_width * h / w`. `Unknown` (or a
/// degenerate zero width) falls back to `fallback_height`.
pub(in crate::app) fn display_height(
    dims: StripPageDims,
    viewport_width: f32,
    fallback_height: f32,
) -> f32 {
    let [width, height] = match dims {
        StripPageDims::Exact(size) | StripPageDims::Hint(size) => size,
        StripPageDims::Unknown => return fallback_height,
    };
    if width == 0 {
        return fallback_height;
    }
    viewport_width * height as f32 / width as f32
}

/// Median of the known page heights, used as the `Unknown` fallback so the strip
/// estimates unmeasured pages at a book-typical height. `None` when empty.
pub(in crate::app) fn median_known_height(heights: impl Iterator<Item = f32>) -> Option<f32> {
    let mut values: Vec<f32> = heights.collect();
    if values.is_empty() {
        return None;
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = values.len() / 2;
    Some(if values.len().is_multiple_of(2) {
        (values[mid - 1] + values[mid]) / 2.0
    } else {
        values[mid]
    })
}

/// Placements for every page that intersects `viewport`, plus one page of margin
/// below as prefetch. Positions are accumulated OUTWARD from the anchor, so a
/// height-estimate error in a distant page can never shift an on-screen rect.
pub(in crate::app) fn layout_visible(
    anchor_index: usize,
    offset_frac: f32,
    viewport: Rect,
    page_count: usize,
    height_of: &impl Fn(usize) -> f32,
) -> Vec<StripPlacement> {
    if page_count == 0 {
        return Vec::new();
    }
    let anchor_index = anchor_index.min(page_count - 1);
    let anchor_height = height_of(anchor_index);
    let anchor_top = viewport.top() - offset_frac * anchor_height;

    let mut placements = vec![StripPlacement {
        index: anchor_index,
        rect: placement_rect(viewport, anchor_top, anchor_height),
    }];

    // Walk UP, prepending each page whose bottom is still below the viewport top.
    // In normal operation the anchor is the topmost visible page, so this adds
    // nothing; it keeps the function correct if the anchor is not truly topmost.
    let mut top = anchor_top;
    let mut index = anchor_index;
    while index > 0 {
        if top <= viewport.top() {
            break;
        }
        let above = index - 1;
        let above_height = height_of(above);
        let above_top = top - above_height;
        placements.insert(
            0,
            StripPlacement {
                index: above,
                rect: placement_rect(viewport, above_top, above_height),
            },
        );
        top = above_top;
        index = above;
    }

    // Walk DOWN, appending each page that starts above the viewport bottom, then
    // exactly one more page past the edge as prefetch margin.
    let mut bottom = anchor_top + anchor_height;
    let mut index = anchor_index;
    let mut extra_pages_past_bottom = 1usize;
    while index + 1 < page_count {
        let below = index + 1;
        let below_top = bottom;
        if below_top >= viewport.bottom() {
            if extra_pages_past_bottom == 0 {
                break;
            }
            extra_pages_past_bottom -= 1;
        }
        let below_height = height_of(below);
        placements.push(StripPlacement {
            index: below,
            rect: placement_rect(viewport, below_top, below_height),
        });
        bottom = below_top + below_height;
        index = below;
    }

    placements
}

/// Apply a pixel scroll delta (positive scrolls the content up / advances the
/// book) and renormalize so the returned offset stays in `[0, 1)`. Clamps at the
/// top of page 0 and at the bottom (the last page's bottom pinned to the viewport
/// bottom), reporting the edge that was hit.
///
/// Takes `viewport_height` (not implied by the plan's short signature) because
/// the bottom clamp is defined relative to the viewport bottom and cannot be
/// computed without it.
pub(in crate::app) fn scroll_by(
    anchor_index: usize,
    offset_frac: f32,
    delta_px: f32,
    viewport_height: f32,
    page_count: usize,
    height_of: &impl Fn(usize) -> f32,
) -> (usize, f32, Option<NavigationDirection>) {
    if page_count == 0 {
        return (0, 0.0, None);
    }
    let mut index = anchor_index.min(page_count - 1);
    let mut pos = offset_frac * height_of(index) + delta_px;

    // Backward renormalize; clamp at the very top and report the Backward edge
    // only when the input actually tried to scroll above page 0.
    while pos < 0.0 {
        if index == 0 {
            return (0, 0.0, Some(NavigationDirection::Backward));
        }
        index -= 1;
        pos += height_of(index);
    }

    // Forward renormalize across page boundaries; stop advancing at the last page.
    loop {
        let height = height_of(index);
        if pos < height || index + 1 >= page_count {
            break;
        }
        pos -= height;
        index += 1;
    }

    // Bottom clamp: measure content from the viewport top down to the book bottom;
    // if it underflows the viewport the last page's bottom would rise above the
    // viewport bottom, so snap to the bottom-pinned position instead.
    let mut content_below = height_of(index) - pos;
    for next in (index + 1)..page_count {
        content_below += height_of(next);
    }
    if content_below < viewport_height {
        return clamp_to_bottom(viewport_height, page_count, height_of);
    }

    let height = height_of(index);
    let offset = if height > 0.0 {
        (pos / height).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (index, offset, None)
}

/// Anchor/offset that pins the last page's bottom to the viewport bottom. Walks
/// up from the last page consuming `viewport_height`; if the whole book is
/// shorter than the viewport it clamps to the top. Always the Forward edge.
fn clamp_to_bottom(
    viewport_height: f32,
    page_count: usize,
    height_of: &impl Fn(usize) -> f32,
) -> (usize, f32, Option<NavigationDirection>) {
    let mut remaining = viewport_height;
    let mut index = page_count - 1;
    loop {
        let height = height_of(index);
        if height >= remaining {
            let offset = if height > 0.0 {
                ((height - remaining) / height).clamp(0.0, 1.0)
            } else {
                0.0
            };
            return (index, offset, Some(NavigationDirection::Forward));
        }
        remaining -= height;
        if index == 0 {
            return (0, 0.0, Some(NavigationDirection::Forward));
        }
        index -= 1;
    }
}

/// The page under the viewport's vertical center, or the nearest by center
/// distance when a gap or edge leaves the center over no page.
pub(in crate::app) fn page_at_viewport_center(
    placements: &[StripPlacement],
    viewport: Rect,
) -> Option<usize> {
    let center_y = viewport.center().y;
    for placement in placements {
        if placement.rect.top() <= center_y && center_y <= placement.rect.bottom() {
            return Some(placement.index);
        }
    }
    placements
        .iter()
        .min_by(|a, b| {
            let da = (a.rect.center().y - center_y).abs();
            let db = (b.rect.center().y - center_y).abs();
            da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|placement| placement.index)
}

/// Jump target for an explicit page selection: land at that page's top. Kept for
/// API symmetry with [`scroll_by`].
pub(in crate::app) fn jump_to_page(index: usize) -> (usize, f32) {
    (index, 0.0)
}

/// Page-turn threshold: sustained edge overscroll must reach this many pixels
/// before a paged edge action (next/previous book, wrap, prompt) fires.
pub(in crate::app) const STRIP_EDGE_OVERSCROLL_THRESHOLD: f32 = 240.0;
/// Idle gap after which a partial edge-overscroll accumulation is forgotten, so
/// a nudge against the edge now does not surface as a page turn much later.
const STRIP_EDGE_OVERSCROLL_WINDOW: Duration = Duration::from_millis(500);

/// Accumulate one at-edge overscroll of `delta_px` in `direction` into
/// `(accum_px, accum_at)` (px signed +Forward/-Backward). The running total is
/// abandoned when the [`STRIP_EDGE_OVERSCROLL_WINDOW`] lapsed since the last push
/// or the push reverses direction. Returns `true` (and resets) once the total
/// magnitude reaches [`STRIP_EDGE_OVERSCROLL_THRESHOLD`], signalling the caller
/// to run one paged edge action. Pure for testing (`now` is injected).
pub(in crate::app) fn accumulate_edge_overscroll(
    accum_px: &mut f32,
    accum_at: &mut Option<Instant>,
    direction: NavigationDirection,
    delta_px: f32,
    now: Instant,
) -> bool {
    let expired =
        accum_at.is_some_and(|at| now.saturating_duration_since(at) > STRIP_EDGE_OVERSCROLL_WINDOW);
    if expired {
        *accum_px = 0.0;
    }
    let signed = match direction {
        NavigationDirection::Forward => delta_px.abs(),
        NavigationDirection::Backward => -delta_px.abs(),
    };
    if *accum_px != 0.0 && accum_px.signum() != signed.signum() {
        *accum_px = 0.0;
    }
    *accum_px += signed;
    *accum_at = Some(now);
    if accum_px.abs() >= STRIP_EDGE_OVERSCROLL_THRESHOLD {
        *accum_px = 0.0;
        *accum_at = None;
        true
    } else {
        false
    }
}

/// The page the strip should recenter the worker on: `Some(derived)` only when it
/// differs from the currently-centered page, so the worker recenter (and reading
/// position persist) fire once per integer page change, not every frame.
pub(in crate::app) fn recenter_target(current: usize, derived: usize) -> Option<usize> {
    (derived != current).then_some(derived)
}

/// Exponential ease-out rate for smooth scrolling: the pending debt decays with
/// time constant 1/rate (~83ms), so a wheel notch glides out over roughly a
/// quarter second instead of teleporting.
const STRIP_SCROLL_DECAY_PER_SEC: f32 = 12.0;
/// Below this remaining debt the tail is snapped in one step so the animation
/// (and its repaint chain) terminates.
const STRIP_SCROLL_SNAP_PX: f32 = 0.5;

/// Portion of the pending smooth-scroll debt to apply this frame. Pure: returns
/// `(step, remaining)`; `remaining == 0.0` means the animation is finished.
pub(in crate::app) fn smooth_scroll_step(pending_px: f32, dt_seconds: f32) -> (f32, f32) {
    if pending_px == 0.0 {
        return (0.0, 0.0);
    }
    if pending_px.abs() <= STRIP_SCROLL_SNAP_PX {
        return (pending_px, 0.0);
    }
    let step = pending_px * (1.0 - (-dt_seconds.max(0.0) * STRIP_SCROLL_DECAY_PER_SEC).exp());
    let remaining = pending_px - step;
    if remaining.abs() <= STRIP_SCROLL_SNAP_PX {
        (pending_px, 0.0)
    } else {
        (step, remaining)
    }
}

fn placement_rect(viewport: Rect, top: f32, height: f32) -> Rect {
    Rect::from_min_max(
        egui::pos2(viewport.left(), top),
        egui::pos2(viewport.right(), top + height),
    )
}

#[cfg(test)]
mod tests;
