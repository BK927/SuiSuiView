//! Anchor-based virtualized vertical strip: pure layout/scroll math plus the
//! background page-dimension prescan worker.
//!
//! V1 is a dark launch. None of the layout/scroll functions below are wired into
//! a renderer yet; the webtoon continuous-vertical-scroll `ViewMode` that lands
//! in V2 is their first consumer. They are marked `#[allow(dead_code)]` narrowly
//! (with this note) so the file compiles clean while nothing calls them. The
//! prescan worker in `scan.rs`, by contrast, is fully wired and live.

mod scan;
pub(in crate::app) use scan::StripDimScanWorker;

use crate::core::source::PageId;
use crate::core::worker::NavigationDirection;
use egui::Rect;

/// Where a virtualized strip is scrolled to: the topmost visible page plus how
/// far the viewport top has slid past that page's top. Consumed in V2.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub(in crate::app) struct StripAnchor {
    pub page_id: PageId,
    /// Fraction of the anchor page's display height scrolled above the viewport
    /// top, in `[0, 1)`. Kept in range by [`scroll_by`]'s renormalization.
    pub offset_frac: f32,
}

/// One laid-out page: its index in the current source snapshot and the screen
/// rect it occupies (full viewport width, fit-width). Consumed in V2.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub(in crate::app) struct StripPlacement {
    pub index: usize,
    pub rect: Rect,
}

/// Known page pixel dimensions, by provenance. `page_metrics` (post-EXIF,
/// authoritative) yields `Exact`; the header prescan yields `Hint`; anything not
/// yet measured is `Unknown` and falls back to the running median. Consumed in V2.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app) enum StripPageDims {
    Exact([u32; 2]),
    Hint([u32; 2]),
    Unknown,
}

/// Fit-width display height for a page: `viewport_width * h / w`. `Unknown` (or a
/// degenerate zero width) falls back to `fallback_height`. Consumed in V2.
#[allow(dead_code)]
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
/// Consumed in V2.
#[allow(dead_code)]
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
/// Consumed in V2.
#[allow(dead_code)]
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
/// bottom), reporting the edge that was hit. Consumed in V2.
///
/// Takes `viewport_height` (not implied by the plan's short signature) because
/// the bottom clamp is defined relative to the viewport bottom and cannot be
/// computed without it.
#[allow(dead_code)]
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
/// distance when a gap or edge leaves the center over no page. Consumed in V2.
#[allow(dead_code)]
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
/// API symmetry with [`scroll_by`]. Consumed in V2.
#[allow(dead_code)]
pub(in crate::app) fn jump_to_page(index: usize) -> (usize, f32) {
    (index, 0.0)
}

fn placement_rect(viewport: Rect, top: f32, height: f32) -> Rect {
    Rect::from_min_max(
        egui::pos2(viewport.left(), top),
        egui::pos2(viewport.right(), top + height),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::pos2;

    fn viewport(height: f32) -> Rect {
        Rect::from_min_max(pos2(0.0, 0.0), pos2(100.0, height))
    }

    fn uniform(height: f32) -> impl Fn(usize) -> f32 {
        move |_| height
    }

    #[test]
    fn display_height_is_fit_width() {
        assert_eq!(
            display_height(StripPageDims::Exact([100, 200]), 50.0, 999.0),
            100.0
        );
        assert_eq!(
            display_height(StripPageDims::Hint([100, 200]), 50.0, 999.0),
            100.0
        );
    }

    #[test]
    fn display_height_falls_back_on_unknown_and_degenerate() {
        assert_eq!(display_height(StripPageDims::Unknown, 50.0, 999.0), 999.0);
        // Zero width is degenerate; the fit-width ratio is undefined, so fall back.
        assert_eq!(
            display_height(StripPageDims::Exact([0, 200]), 50.0, 999.0),
            999.0
        );
    }

    #[test]
    fn median_known_height_handles_parity_and_empty() {
        assert_eq!(median_known_height(std::iter::empty()), None);
        assert_eq!(median_known_height([100.0].into_iter()), Some(100.0));
        assert_eq!(
            median_known_height([300.0, 100.0, 200.0].into_iter()),
            Some(200.0)
        );
        assert_eq!(
            median_known_height([400.0, 100.0, 300.0, 200.0].into_iter()),
            Some(250.0)
        );
    }

    #[test]
    fn layout_places_anchor_exactly_and_walks_down() {
        let height_of = uniform(100.0);
        let placements = layout_visible(2, 0.5, viewport(300.0), 10, &height_of);
        // Anchor is topmost (offset in range), so no page is prepended above it.
        assert_eq!(
            placements.iter().map(|p| p.index).collect::<Vec<_>>(),
            vec![2, 3, 4, 5, 6]
        );
        // Anchor top = viewport.top() - offset * height = 0 - 0.5 * 100 = -50.
        assert_eq!(placements[0].rect.top(), -50.0);
        assert_eq!(placements[0].rect.bottom(), 50.0);
        assert_eq!(placements[0].rect.left(), 0.0);
        assert_eq!(placements[0].rect.right(), 100.0);
        // Pages below stack with zero gap, accumulated from the anchor downward.
        assert_eq!(placements[1].rect.top(), 50.0);
        assert_eq!(placements[2].rect.top(), 150.0);
        assert_eq!(placements[3].rect.top(), 250.0);
        assert_eq!(placements[4].rect.top(), 350.0);
    }

    #[test]
    fn layout_walks_up_when_anchor_is_not_topmost() {
        // A negative offset is outside the documented [0,1) domain, but exercises
        // the defensive walk-up: the anchor's top sits below the viewport top, so
        // the page above peeks in and must be prepended.
        let height_of = uniform(100.0);
        let placements = layout_visible(3, -0.5, viewport(300.0), 10, &height_of);
        assert_eq!(placements[0].index, 2);
        assert_eq!(placements[0].rect.top(), -50.0);
        assert_eq!(placements[1].index, 3);
        assert_eq!(placements[1].rect.top(), 50.0);
    }

    #[test]
    fn layout_on_screen_rects_ignore_far_page_heights() {
        let viewport_width = 100.0;
        let fallback = 150.0;
        let mut dims = vec![
            StripPageDims::Exact([100, 120]),
            StripPageDims::Hint([100, 90]),
            StripPageDims::Unknown,
            StripPageDims::Exact([100, 100]),
            StripPageDims::Hint([100, 110]),
        ];
        dims.extend(std::iter::repeat_n(StripPageDims::Exact([100, 100]), 10));

        let layout = |dims: &[StripPageDims]| {
            let dims = dims.to_vec();
            let len = dims.len();
            let height_of =
                move |index: usize| display_height(dims[index], viewport_width, fallback);
            layout_visible(0, 0.0, viewport(250.0), len, &height_of)
        };
        let before = layout(&dims);

        // Mutating a page far below the visible window (never visited by the
        // walk) must leave every on-screen rect byte-identical.
        dims[12] = StripPageDims::Exact([100, 9000]);
        let after = layout(&dims);
        assert_eq!(before, after);
        // Sanity: the visible window really is only the near pages plus margin.
        assert!(before.iter().all(|p| p.index <= 4));
    }

    #[test]
    fn scroll_renormalizes_forward_across_boundaries() {
        let height_of = uniform(100.0);
        let (index, offset, edge) = scroll_by(0, 0.0, 250.0, 300.0, 10, &height_of);
        assert_eq!(index, 2);
        assert_eq!(offset, 0.5);
        assert_eq!(edge, None);
    }

    #[test]
    fn scroll_renormalizes_backward_across_boundaries() {
        let height_of = uniform(100.0);
        let (index, offset, edge) = scroll_by(5, 0.0, -250.0, 300.0, 10, &height_of);
        assert_eq!(index, 2);
        assert_eq!(offset, 0.5);
        assert_eq!(edge, None);
    }

    #[test]
    fn scroll_clamps_at_top_and_reports_only_when_beyond() {
        let height_of = uniform(100.0);
        // Tried to scroll above page 0: clamp and report the Backward edge.
        let beyond = scroll_by(0, 0.2, -100.0, 300.0, 10, &height_of);
        assert_eq!(beyond, (0, 0.0, Some(NavigationDirection::Backward)));
        // Landed exactly on the top without overshooting: no edge report.
        let exact = scroll_by(1, 0.0, -100.0, 300.0, 10, &height_of);
        assert_eq!(exact, (0, 0.0, None));
    }

    #[test]
    fn scroll_clamps_at_bottom_pinning_last_page() {
        let height_of = uniform(100.0);
        // Overshoot far past the end; the last three 100px pages exactly fill the
        // 300px viewport, so the bottom-pinned anchor is page 7 at offset 0.
        let clamped = scroll_by(8, 0.5, 500.0, 300.0, 10, &height_of);
        assert_eq!(clamped, (7, 0.0, Some(NavigationDirection::Forward)));
    }

    #[test]
    fn scroll_book_shorter_than_viewport_clamps_to_top() {
        let height_of = uniform(100.0);
        // Two 100px pages, 300px viewport: the book cannot fill the viewport.
        let down = scroll_by(0, 0.0, 50.0, 300.0, 2, &height_of);
        assert_eq!(down, (0, 0.0, Some(NavigationDirection::Forward)));
        let up = scroll_by(0, 0.0, -50.0, 300.0, 2, &height_of);
        assert_eq!(up, (0, 0.0, Some(NavigationDirection::Backward)));
    }

    #[test]
    fn page_at_center_prefers_containing_placement() {
        let height_of = uniform(100.0);
        let placements = layout_visible(0, 0.0, viewport(300.0), 10, &height_of);
        // Center y = 150 falls inside page 1 (100..200).
        assert_eq!(
            page_at_viewport_center(&placements, viewport(300.0)),
            Some(1)
        );
    }

    #[test]
    fn page_at_center_falls_back_to_nearest_over_a_gap() {
        let placements = vec![
            StripPlacement {
                index: 5,
                rect: Rect::from_min_max(pos2(0.0, 0.0), pos2(100.0, 40.0)),
            },
            StripPlacement {
                index: 6,
                rect: Rect::from_min_max(pos2(0.0, 200.0), pos2(100.0, 300.0)),
            },
        ];
        // Center y = 150 is over neither rect; page 6's center (250) is nearer.
        assert_eq!(
            page_at_viewport_center(&placements, viewport(300.0)),
            Some(6)
        );
        assert_eq!(page_at_viewport_center(&[], viewport(300.0)), None);
    }

    #[test]
    fn jump_to_page_lands_at_page_top() {
        assert_eq!(jump_to_page(7), (7, 0.0));
    }
}
