use super::{SPREAD_GAP_POINTS, TARGET_EDGE_HYSTERESIS};
use crate::app::commands::command_for_mouse_gesture;
use crate::app::{ui, SuiSuiViewApp};
use crate::core::state::{FitMode, MouseGesture, WheelMode};
use crate::core::worker::{
    clamp_navigation_target_long_edge, clamp_target_long_edge, NavigationDirection,
    MAX_TARGET_LONG_EDGE,
};
use eframe::egui::{self, Align2, Color32, Rect, Vec2};

impl SuiSuiViewApp {
    pub(super) fn paint_page_arrows(
        &mut self,
        ctx: &egui::Context,
        painter: &egui::Painter,
        rect: Rect,
    ) {
        if !self.settings.show_page_arrows || self.source.is_none() {
            return;
        }
        let Some(pointer_pos) = ctx.pointer_hover_pos() else {
            return;
        };
        if !rect.contains(pointer_pos) {
            return;
        }
        let zone_width = rect.width().clamp(56.0, 96.0);
        let side = if pointer_pos.x <= rect.left() + zone_width {
            Some((ui::icons::CHEVRON_LEFT, NavigationDirection::Backward))
        } else if pointer_pos.x >= rect.right() - zone_width {
            Some((ui::icons::CHEVRON_RIGHT, NavigationDirection::Forward))
        } else {
            None
        };
        let Some((icon, direction)) = side else {
            return;
        };
        let button_rect = Rect::from_center_size(
            egui::pos2(
                if direction == NavigationDirection::Backward {
                    rect.left() + 42.0
                } else {
                    rect.right() - 42.0
                },
                rect.center().y,
            ),
            egui::vec2(52.0, 76.0),
        );
        painter.rect_filled(
            button_rect,
            8.0,
            Color32::from_rgba_unmultiplied(18, 20, 24, 180),
        );
        painter.text(
            button_rect.center(),
            Align2::CENTER_CENTER,
            icon,
            ui::icons::icon_font(ui::icons::IconStyle::Regular, 32.0),
            ui::theme::TEXT_PRIMARY,
        );
        let clicked_arrow_zone = ctx.input(|input| {
            input.pointer.primary_released()
                && input
                    .pointer
                    .press_origin()
                    .is_some_and(|origin| button_rect.contains(origin))
        });
        if clicked_arrow_zone {
            match direction {
                NavigationDirection::Forward => self.next_page(),
                NavigationDirection::Backward => self.previous_page(),
            }
        }
    }

    pub(super) fn update_target_long_edge(&mut self, ctx: &egui::Context, viewport: Vec2) {
        if self.source.is_none() {
            return;
        }

        let next = self.target_long_edge_for(ctx, viewport);
        let original_inspection_target =
            next > MAX_TARGET_LONG_EDGE || self.target_long_edge > MAX_TARGET_LONG_EDGE;
        if next == self.target_long_edge
            || (!original_inspection_target
                && next.abs_diff(self.target_long_edge) < TARGET_EDGE_HYSTERESIS)
        {
            return;
        }

        self.target_long_edge = next;
        self.clear_pending_page_turns();
        self.worker.set_page(
            self.worker_center_page(),
            self.last_nav_direction,
            next,
            self.visible_page_count(),
            self.worker_options(),
        );
        self.refresh_ai_prefetch_queue();
        self.request_adjacent_seed_prefetch();
        ctx.request_repaint();
    }

    fn target_long_edge_for(&self, ctx: &egui::Context, viewport: Vec2) -> u32 {
        let page_viewport = if self.page_viewport_count_for_target() <= 1 {
            viewport
        } else {
            Vec2::new((viewport.x - SPREAD_GAP_POINTS).max(1.0) * 0.5, viewport.y)
        };
        target_long_edge_for_view(
            self.fit_mode,
            self.manual_zoom,
            page_viewport,
            ctx.pixels_per_point(),
            self.visible_original_long_edge(),
        )
    }

    fn visible_original_long_edge(&self) -> Option<u32> {
        let mut longest = 0.0_f32;
        for index in self.spread_indices() {
            let metrics = self.page_metrics.get(&index)?;
            longest = longest.max(metrics.width.max(metrics.height));
        }
        (longest > 0.0).then(|| longest.ceil() as u32)
    }

    fn page_viewport_count_for_target(&self) -> usize {
        if !self.view_mode.is_smart() {
            return self.view_mode.step();
        }
        let Some(source) = self.source.as_ref() else {
            return 2;
        };
        let page_count = source.page_count();
        if page_count == 0 {
            return 1;
        }
        let page = self.current_page.min(page_count - 1);
        let anchor = page - (page % 2);
        let Some(next) = anchor.checked_add(1).filter(|next| *next < page_count) else {
            return 1;
        };
        if self.page_metrics.contains_key(&anchor) && self.page_metrics.contains_key(&next) {
            self.smart_spread_indices_for(page, page_count).len()
        } else {
            2
        }
    }

    pub(super) fn handle_viewer_pointer(&mut self, ui: &egui::Ui, response: &egui::Response) {
        if response.double_clicked() {
            if let Some(command) =
                command_for_mouse_gesture(MouseGesture::DoubleClick, &self.settings)
            {
                self.apply_command(ui.ctx(), command);
            }
        }

        if response.middle_clicked() {
            let gesture = if ui.input(|input| input.modifiers.ctrl) {
                MouseGesture::CtrlMiddleClick
            } else {
                MouseGesture::MiddleClick
            };
            if let Some(command) = command_for_mouse_gesture(gesture, &self.settings) {
                self.apply_command(ui.ctx(), command);
            }
        }

        if response.dragged() {
            self.pan += ui.input(|input| input.pointer.delta());
        }

        if !response.hovered() {
            return;
        }

        let (scroll_y, ctrl) = ui.input(|input| (input.raw_scroll_delta.y, input.modifiers.ctrl));
        if scroll_y.abs() < 1.0 {
            return;
        }

        if ctrl {
            let gesture = if scroll_y > 0.0 {
                MouseGesture::CtrlWheelUp
            } else {
                MouseGesture::CtrlWheelDown
            };
            if let Some(command) = command_for_mouse_gesture(gesture, &self.settings) {
                self.apply_command(ui.ctx(), command);
            }
        } else if self.settings.wheel_mode == WheelMode::ScrollWhenZoomed
            && self.fit_mode == FitMode::Manual
            && self.manual_zoom > 1.01
        {
            self.pan.y += scroll_y;
        } else if scroll_y < -30.0 {
            if let Some(command) =
                command_for_mouse_gesture(MouseGesture::WheelDown, &self.settings)
            {
                self.apply_command(ui.ctx(), command);
            }
        } else if scroll_y > 30.0 {
            if let Some(command) = command_for_mouse_gesture(MouseGesture::WheelUp, &self.settings)
            {
                self.apply_command(ui.ctx(), command);
            }
        }
    }
}

fn target_long_edge_for_view(
    fit_mode: FitMode,
    manual_zoom: f32,
    page_viewport: Vec2,
    pixels_per_point: f32,
    original_long_edge: Option<u32>,
) -> u32 {
    let display_target =
        display_target_long_edge_for_view(fit_mode, manual_zoom, page_viewport, pixels_per_point);
    original_inspection_target_long_edge(fit_mode, manual_zoom, original_long_edge)
        .map_or(display_target, |original_target| {
            original_target.max(display_target)
        })
}

fn display_target_long_edge_for_view(
    fit_mode: FitMode,
    manual_zoom: f32,
    page_viewport: Vec2,
    pixels_per_point: f32,
) -> u32 {
    let base_points = match fit_mode {
        FitMode::FitWidth => page_viewport.x,
        FitMode::FitHeight => page_viewport.y,
        _ => page_viewport.x.max(page_viewport.y),
    };
    let viewport_pixels = base_points * pixels_per_point;
    let zoom_multiplier = match fit_mode {
        FitMode::Manual => manual_zoom.max(1.0),
        _ => 1.0,
    };
    let oversample = match fit_mode {
        FitMode::FitPage | FitMode::FitWidth | FitMode::FitHeight => 1.0,
        FitMode::Manual | FitMode::Original => 1.5,
    };
    let raw = viewport_pixels * oversample * zoom_multiplier;
    let quantized = ((raw / 256.0).ceil() * 256.0) as u32;
    clamp_navigation_target_long_edge(quantized)
}

fn original_inspection_target_long_edge(
    fit_mode: FitMode,
    manual_zoom: f32,
    original_long_edge: Option<u32>,
) -> Option<u32> {
    let needs_original_pixels = match fit_mode {
        FitMode::Original => true,
        FitMode::Manual => manual_zoom >= 1.0,
        FitMode::FitPage | FitMode::FitWidth | FitMode::FitHeight => false,
    };
    if !needs_original_pixels {
        return None;
    }
    original_long_edge.map(clamp_target_long_edge)
}

#[cfg(test)]
mod tests {
    use super::target_long_edge_for_view;
    use crate::core::state::FitMode;
    use crate::core::worker::MAX_TARGET_LONG_EDGE;
    use eframe::egui::Vec2;

    #[test]
    fn fit_modes_stay_on_navigation_target_cap() {
        assert_eq!(
            target_long_edge_for_view(
                FitMode::FitPage,
                1.0,
                Vec2::new(10_000.0, 10_000.0),
                1.0,
                Some(8192),
            ),
            MAX_TARGET_LONG_EDGE
        );
    }

    #[test]
    fn original_mode_requests_source_long_edge_when_known() {
        assert_eq!(
            target_long_edge_for_view(
                FitMode::Original,
                1.0,
                Vec2::new(1200.0, 1600.0),
                1.0,
                Some(8192),
            ),
            8192
        );
    }

    #[test]
    fn manual_zoom_at_original_scale_requests_source_long_edge() {
        assert_eq!(
            target_long_edge_for_view(
                FitMode::Manual,
                1.1,
                Vec2::new(1200.0, 1600.0),
                1.0,
                Some(8192),
            ),
            8192
        );
    }

    #[test]
    fn original_mode_without_metrics_uses_display_target() {
        assert_eq!(
            target_long_edge_for_view(FitMode::Original, 1.0, Vec2::new(1200.0, 1600.0), 1.0, None,),
            2560
        );
    }
}
