use super::{ui, SuiSuiViewApp};
use crate::core::state::CommandId;
use crate::core::worker::NavigationDirection;
use eframe::egui::{self, Color32, Pos2, RichText, Stroke, Vec2};
use std::time::{Duration, Instant};

const EDGE_PROMPT_AUTO_DISMISS: Duration = Duration::from_millis(1200);

#[derive(Debug, Clone, Copy)]
pub(in crate::app) struct EdgePrompt {
    pub(in crate::app) direction: NavigationDirection,
    opened_at: Instant,
}

impl EdgePrompt {
    pub(in crate::app) fn new(direction: NavigationDirection) -> Self {
        Self {
            direction,
            opened_at: Instant::now(),
        }
    }
}

impl SuiSuiViewApp {
    pub(in crate::app) fn show_edge_prompt(&mut self, ctx: &egui::Context) {
        let Some(prompt) = self.edge_prompt else {
            return;
        };
        if self.source.is_none() {
            self.edge_prompt = None;
            return;
        }
        if self.settings_open || self.about_open || self.bookmark_popover_open {
            self.edge_prompt = None;
            return;
        }

        let screen = ctx.screen_rect();
        let available_width = (screen.width() - 32.0).max(280.0);
        let width = available_width.min(560.0).max(available_width.min(360.0));
        let pos = Pos2::new(
            screen.center().x - width * 0.5,
            (screen.bottom() - 164.0).max(screen.top() + 80.0),
        );
        let response = egui::Area::new("edge_page_prompt".into())
            .order(egui::Order::Foreground)
            .fixed_pos(pos)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(Color32::from_rgb(8, 9, 11))
                    .stroke(Stroke::new(1.2, ui::theme::SUBTLE_STROKE))
                    .corner_radius(egui::CornerRadius::same(14))
                    .shadow(egui::epaint::Shadow {
                        offset: [0, 10],
                        blur: 22,
                        spread: 0,
                        color: Color32::from_black_alpha(150),
                    })
                    .inner_margin(egui::Margin::symmetric(22, 20))
                    .show(ui, |ui| {
                        ui.set_width(width - 44.0);
                        ui.vertical_centered(|ui| {
                            let i18n = self.i18n();
                            let title = match prompt.direction {
                                NavigationDirection::Forward => i18n.text("navigation.edge.last"),
                                NavigationDirection::Backward => i18n.text("navigation.edge.first"),
                            };
                            ui.label(
                                RichText::new(title)
                                    .size(24.0)
                                    .color(ui::theme::TEXT_PRIMARY)
                                    .strong(),
                            );
                            ui.add_space(18.0);
                            ui.horizontal_centered(|ui| {
                                let previous_file = i18n.text("navigation.edge.previous_file");
                                let previous_label = self.edge_action_button_text(
                                    &previous_file,
                                    CommandId::PreviousBook,
                                );
                                if edge_prompt_button(ui, &previous_label).clicked() {
                                    self.edge_prompt = None;
                                    self.open_sibling_book(-1);
                                }

                                let next_file = i18n.text("navigation.edge.next_file");
                                let next_label =
                                    self.edge_action_button_text(&next_file, CommandId::NextBook);
                                if edge_prompt_button(ui, &next_label).clicked() {
                                    self.edge_prompt = None;
                                    self.open_sibling_book(1);
                                }
                            });
                        });
                    });
            });

        let prompt_rect = response.response.rect;
        let (clicked_outside, hovered) = ctx.input(|input| {
            let clicked_outside = input.pointer.any_click()
                && input
                    .pointer
                    .interact_pos()
                    .is_some_and(|pos| !prompt_rect.contains(pos));
            let hovered = input
                .pointer
                .hover_pos()
                .is_some_and(|pos| prompt_rect.contains(pos));
            (clicked_outside, hovered)
        });
        if clicked_outside {
            self.edge_prompt = None;
            return;
        }

        if edge_prompt_should_auto_dismiss(prompt, hovered, Instant::now()) {
            self.edge_prompt = None;
        } else {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    fn edge_action_button_text(&self, label: &str, command: CommandId) -> String {
        self.shortcut_hint_for_command(command).map_or_else(
            || label.to_owned(),
            |shortcut| format!("{label} ({shortcut})"),
        )
    }

    fn shortcut_hint_for_command(&self, command: CommandId) -> Option<String> {
        self.settings
            .key_bindings
            .iter()
            .find(|binding| binding.command == command)
            .map(|binding| binding.shortcut.label())
    }
}

fn edge_prompt_should_auto_dismiss(prompt: EdgePrompt, hovered: bool, now: Instant) -> bool {
    !hovered && now.saturating_duration_since(prompt.opened_at) >= EDGE_PROMPT_AUTO_DISMISS
}

fn edge_prompt_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new(
            RichText::new(label)
                .size(16.0)
                .color(ui::theme::TEXT_PRIMARY)
                .strong(),
        )
        .min_size(Vec2::new(220.0, 34.0))
        .fill(Color32::from_rgb(5, 6, 8))
        .stroke(Stroke::new(1.0, Color32::from_rgb(52, 55, 60))),
    )
}

#[cfg(test)]
mod tests {
    use super::{edge_prompt_should_auto_dismiss, EdgePrompt, EDGE_PROMPT_AUTO_DISMISS};
    use crate::core::worker::NavigationDirection;
    use std::time::Instant;

    #[test]
    fn edge_prompt_auto_dismisses_after_timeout() {
        let opened_at = Instant::now();
        let prompt = EdgePrompt {
            direction: NavigationDirection::Forward,
            opened_at,
        };

        assert!(!edge_prompt_should_auto_dismiss(
            prompt,
            false,
            opened_at + EDGE_PROMPT_AUTO_DISMISS / 2
        ));
        assert!(edge_prompt_should_auto_dismiss(
            prompt,
            false,
            opened_at + EDGE_PROMPT_AUTO_DISMISS
        ));
    }

    #[test]
    fn edge_prompt_stays_visible_while_hovered() {
        let opened_at = Instant::now();
        let prompt = EdgePrompt {
            direction: NavigationDirection::Forward,
            opened_at,
        };

        assert!(!edge_prompt_should_auto_dismiss(
            prompt,
            true,
            opened_at + EDGE_PROMPT_AUTO_DISMISS
        ));
    }
}
