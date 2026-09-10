use super::state::CompareSide;
use crate::app::gpu_paint::{GpuInspection, GpuPaintRequest, GpuPaintSourceKey};
use crate::app::{PageVisual, SuiSuiViewApp};
use crate::core::worker::PagePixels;
use egui::{Align2, Color32, Pos2, Rect, Vec2};

pub(super) struct LoupePage {
    source: GpuPaintSourceKey,
    image_size: [usize; 2],
    pixels: PagePixels,
    reference_size: [u32; 2],
    center_uv: Pos2,
}

impl SuiSuiViewApp {
    pub(in crate::app) fn begin_compare_frame(&mut self) {
        self.debug_compare.loupe_page = None;
        if self.debug_compare.enabled {
            self.transition = None;
            self.current_view_state = None;
        }
    }

    /// Called with the ordinary spread/strip rect and the already selected input.
    /// Returning None lets the ordinary loading/error placeholder paint normally.
    pub(in crate::app) fn paint_compare_page(
        &mut self,
        ctx: &egui::Context,
        painter: &egui::Painter,
        viewport: Rect,
        page_rect: Rect,
        visual: &PageVisual,
    ) -> Option<bool> {
        if !self.debug_compare.enabled {
            return None;
        }
        let PageVisual::ReadyGpu {
            source_key,
            image_size,
            pixels,
            effects,
            ..
        } = visual
        else {
            return None;
        };
        let reference_size = physical_reference_size(page_rect, ctx.pixels_per_point());
        let pointer = ctx
            .pointer_hover_pos()
            .filter(|pos| viewport.contains(*pos));
        let split = self.debug_compare.side_by_side;
        let panes = comparison_panes(viewport, self.debug_compare.wipe, split);
        let offsets = comparison_offsets(viewport, split, ctx.pixels_per_point());
        let source_pointer = pointer.and_then(|pos| {
            (0..2).find_map(|side| {
                (panes[side].contains(pos) && page_rect.translate(offsets[side]).contains(pos))
                    .then_some(pos - offsets[side])
            })
        });
        let inspect = self.debug_compare.loupe
            && source_pointer.is_some()
            && GpuInspection::supports_size(reference_size);
        let mut sides = [self.debug_compare.left, self.debug_compare.right]
            .map(|side| self.effective_comparison_side(side));
        // Rotation and flips define a shared coordinate system, never side-local
        // correction controls. Navigation can update them while comparison stays open.
        for side in &mut sides {
            side.effects.transform = effects.transform;
        }
        self.debug_compare.left.effects.transform = effects.transform;
        self.debug_compare.right.effects.transform = effects.transform;
        for (index, side) in sides.into_iter().enumerate() {
            let pane_painter = painter.with_clip_rect(panes[index].intersect(painter.clip_rect()));
            let rect = page_rect.translate(offsets[index]);
            if let Some(reason) = self.compare_unavailable_reason(side.target) {
                self.paint_placeholder(
                    &pane_painter,
                    rect,
                    &reason,
                    Color32::LIGHT_GRAY,
                    Color32::WHITE,
                );
                continue;
            }
            let inspection = inspect.then(|| GpuInspection {
                reference_size,
                reference_id: side.reference_id(
                    *source_key,
                    side.target.resolved(self.debug_compare.snapshot),
                    self.settings.fixed_2x_sr_min_scale_pct,
                ),
                uv: full_uv(),
            });
            self.paint_comparison_request(
                &pane_painter,
                rect,
                (index + 10) as u8,
                *source_key,
                *image_size,
                pixels.clone(),
                side,
                inspection,
            );
        }
        if let Some(pointer) = source_pointer.filter(|_| self.debug_compare.loupe) {
            self.debug_compare.loupe_page = Some(LoupePage {
                source: *source_key,
                image_size: *image_size,
                pixels: pixels.clone(),
                reference_size,
                center_uv: display_pointer_uv(pointer, page_rect, ctx.pixels_per_point()),
            });
        }
        Some(true)
    }

    pub(in crate::app) fn paint_debug_compare_overlay(
        &mut self,
        ctx: &egui::Context,
        painter: &egui::Painter,
        viewport: Rect,
    ) {
        if !self.debug_compare.enabled {
            return;
        }
        let panes = comparison_panes(
            viewport,
            self.debug_compare.wipe,
            self.debug_compare.side_by_side,
        );
        let x = panes[0].right();
        painter.line_segment(
            [
                egui::pos2(x, viewport.top()),
                egui::pos2(x, viewport.bottom()),
            ],
            egui::Stroke::new(1.0, Color32::WHITE),
        );
        let sides = [self.debug_compare.left, self.debug_compare.right]
            .map(|side| self.effective_comparison_side(side));
        for (index, side) in sides.into_iter().enumerate() {
            let label = format!(
                "{}: {}",
                if index == 0 { "A" } else { "B" },
                side.target.label(self.debug_compare.snapshot, self.i18n())
            );
            label_at(
                painter,
                panes[index].left_top() + egui::vec2(8.0, 8.0),
                &label,
            );
        }
        if !self.can_paint_wgsl_effects() {
            label_at(
                painter,
                viewport.center(),
                &self.i18n().text("compare.gpu_unavailable"),
            );
            return;
        }
        let Some(page) = self.debug_compare.loupe_page.take() else {
            return;
        };
        if !GpuInspection::supports_size(page.reference_size) {
            label_at(
                painter,
                viewport.left_bottom() + egui::vec2(8.0, -32.0),
                &self.i18n().text("compare.loupe_limit"),
            );
            return;
        }
        let requested_side = 220.0_f32
            .min((viewport.width() - 36.0) / 2.0)
            .min(viewport.height() - 36.0)
            .max(32.0);
        let ppp = ctx.pixels_per_point();
        let side_points = (requested_side * ppp).floor() / ppp;
        let gap = (8.0 * ppp).round() / ppp;
        let size = Vec2::splat(side_points);
        let start =
            viewport.right_bottom() - egui::vec2(side_points * 2.0 + 24.0, side_points + 12.0);
        let uv = loupe_uv(
            page.center_uv,
            page.reference_size,
            size * ctx.pixels_per_point(),
            self.debug_compare.loupe_zoom,
        );
        for (index, side) in sides.into_iter().enumerate() {
            let rect = Rect::from_min_size(
                start + egui::vec2(index as f32 * (side_points + gap), 0.0),
                size,
            );
            painter.rect_filled(rect.expand(3.0), 2.0, Color32::BLACK);
            self.paint_comparison_request(
                painter,
                rect,
                (index + 12) as u8,
                page.source,
                page.image_size,
                page.pixels.clone(),
                side,
                Some(GpuInspection {
                    reference_size: page.reference_size,
                    uv,
                    reference_id: side.reference_id(
                        page.source,
                        side.target.resolved(self.debug_compare.snapshot),
                        self.settings.fixed_2x_sr_min_scale_pct,
                    ),
                }),
            );
            label_at(
                painter,
                rect.left_top() + egui::vec2(5.0, 5.0),
                if index == 0 { "A" } else { "B" },
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_comparison_request(
        &self,
        painter: &egui::Painter,
        rect: Rect,
        slot: u8,
        source_key: GpuPaintSourceKey,
        image_size: [usize; 2],
        pixels: PagePixels,
        side: CompareSide,
        inspection: Option<GpuInspection>,
    ) {
        let side = self.effective_comparison_side(side);
        self.paint_wgsl_effects(
            painter,
            GpuPaintRequest {
                rect,
                slot,
                source_key,
                image_size,
                pixels,
                inspection,
                effects: side.effects.for_render(),
                wgpu_upscale_method: side.target.resolved(self.debug_compare.snapshot),
                wgpu_downscale_method: side.downscale.method(),
                fixed_2x_sr_min_scale_pct: self.settings.fixed_2x_sr_min_scale_pct,
                linear_downscale: side.linear,
                deband: side.resolved_deband(),
                opacity: 1.0,
                zoom_in_motion: false,
            },
        );
    }

    fn effective_comparison_side(&self, side: CompareSide) -> CompareSide {
        if self.adjustments.uncorrected {
            CompareSide {
                target: super::DebugCompareTarget::Model(
                    crate::core::state::WgpuUpscaleMethod::None,
                ),
                effects: side.effects.uncorrected(),
                deband: crate::core::deband::DebandStrength::Off,
                ..side
            }
        } else {
            side
        }
    }
}

fn label_at(painter: &egui::Painter, pos: Pos2, text: &str) {
    let galley = painter.layout_no_wrap(
        text.to_owned(),
        egui::FontId::proportional(13.0),
        Color32::WHITE,
    );
    painter.rect_filled(
        Rect::from_min_size(pos, galley.size()).expand(4.0),
        3.0,
        Color32::from_black_alpha(190),
    );
    painter.text(
        pos,
        Align2::LEFT_TOP,
        text,
        egui::FontId::proportional(13.0),
        Color32::WHITE,
    );
}

fn full_uv() -> Rect {
    Rect::from_min_max(Pos2::ZERO, egui::pos2(1.0, 1.0))
}

fn physical_reference_size(rect: Rect, ppp: f32) -> [u32; 2] {
    [
        ((rect.right() * ppp).round() - (rect.left() * ppp).round()).max(1.0) as u32,
        ((rect.bottom() * ppp).round() - (rect.top() * ppp).round()).max(1.0) as u32,
    ]
}

fn display_pointer_uv(pointer: Pos2, rect: Rect, ppp: f32) -> Pos2 {
    let size = physical_reference_size(rect, ppp);
    egui::pos2(
        ((pointer.x * ppp - (rect.left() * ppp).round()) / size[0] as f32).clamp(0.0, 1.0),
        ((pointer.y * ppp - (rect.top() * ppp).round()) / size[1] as f32).clamp(0.0, 1.0),
    )
}

fn comparison_panes(viewport: Rect, wipe: f32, side_by_side: bool) -> [Rect; 2] {
    let x = viewport.left()
        + viewport.width()
            * if side_by_side {
                0.5
            } else {
                wipe.clamp(0.0, 1.0)
            };
    [
        Rect::from_min_max(viewport.min, egui::pos2(x, viewport.bottom())),
        Rect::from_min_max(egui::pos2(x, viewport.top()), viewport.max),
    ]
}

fn comparison_offsets(viewport: Rect, side_by_side: bool, ppp: f32) -> [Vec2; 2] {
    let shift = if side_by_side {
        (viewport.width() * 0.25 * ppp).round() / ppp
    } else {
        0.0
    };
    [egui::vec2(-shift, 0.0), egui::vec2(shift, 0.0)]
}

fn loupe_uv(center: Pos2, reference_size: [u32; 2], destination_pixels: Vec2, zoom: f32) -> Rect {
    let extent = egui::vec2(
        destination_pixels.x / (reference_size[0] as f32 * zoom),
        destination_pixels.y / (reference_size[1] as f32 * zoom),
    )
    .min(Vec2::splat(1.0));
    let min = (center.to_vec2() - extent * 0.5)
        .max(Vec2::ZERO)
        .min(Vec2::splat(1.0) - extent);
    Rect::from_min_size(min.to_pos2(), extent)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comparison_panes_keep_full_viewer_scale_at_fractional_dpi() {
        let viewport = Rect::from_min_size(egui::pos2(2.3, 5.7), egui::vec2(1001.0, 801.0));
        let page = Rect::from_min_size(egui::pos2(-50.1, 36.2), egui::vec2(1250.3, 901.7));
        for ppp in [1.0, 1.25, 1.5, 2.0] {
            for offset in comparison_offsets(viewport, true, ppp) {
                assert_eq!(
                    physical_reference_size(page, ppp),
                    physical_reference_size(page.translate(offset), ppp)
                );
                assert_eq!(page.size(), page.translate(offset).size());
            }
        }
        assert_eq!(comparison_offsets(viewport, false, 1.25), [Vec2::ZERO; 2]);
    }

    #[test]
    fn loupe_uses_display_pixels_and_clamps_edges_without_changing_reference() {
        let uv = loupe_uv(
            egui::pos2(0.5, 0.5),
            [1000, 2000],
            egui::vec2(200.0, 200.0),
            4.0,
        );
        assert!((uv.width() - 0.05).abs() < 0.00001);
        assert!((uv.height() - 0.025).abs() < 0.00001);
        let edge = loupe_uv(Pos2::ZERO, [1000, 2000], egui::vec2(200.0, 200.0), 4.0);
        assert_eq!(edge.min, Pos2::ZERO);
        assert!((edge.size() - uv.size()).length() < 0.00001);
    }

    #[test]
    fn loupe_pointer_tracks_rounded_gpu_pixels_at_fractional_dpi() {
        let rect = Rect::from_min_size(egui::pos2(7.3, -13.7), egui::vec2(1001.3, 801.7));
        for ppp in [1.0, 1.25, 1.5, 2.0] {
            let size = physical_reference_size(rect, ppp);
            let pointer = egui::pos2(
                ((rect.left() * ppp).round() + 75.5) / ppp,
                ((rect.top() * ppp).round() + 111.5) / ppp,
            );
            let uv = display_pointer_uv(pointer, rect, ppp);
            assert!((uv.x * size[0] as f32 - 75.5).abs() < 0.0001);
            assert!((uv.y * size[1] as f32 - 111.5).abs() < 0.0001);
        }
    }
}
