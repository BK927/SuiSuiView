use super::commands::{collect_keyboard_actions, AppCommand, KeyboardAction, NavigationRelease};
use super::{SuiSuiViewApp, ViewMode};
use crate::core::formats::OPENABLE_FILE_EXTENSIONS;
use crate::core::worker::NavigationDirection;
use rfd::FileDialog;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app) enum KeyboardLayer {
    FastStartFailure,
    FileDeleteConfirmation,
    GpuConfirmation,
    BookmarkDeleteConfirmation,
    EdgePrompt,
    BookmarkPopover,
    Viewer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum KeyboardRoute {
    Block,
    DelegateToOverlay,
    DismissOverlay,
    PassToBookmarkPopover,
    PassToViewer,
}

impl SuiSuiViewApp {
    pub(in crate::app) fn handle_keyboard(&mut self, ctx: &egui::Context) {
        let layer = self.keyboard_layer();
        let escape_pressed = ctx.input(|input| input.key_pressed(egui::Key::Escape));
        let enter_pressed = ctx.input(|input| input.key_pressed(egui::Key::Enter));
        let route = keyboard_route_for(
            layer,
            escape_pressed,
            enter_pressed,
            ctx.wants_keyboard_input(),
        );
        match route {
            KeyboardRoute::Block | KeyboardRoute::DelegateToOverlay => return,
            KeyboardRoute::DismissOverlay => match layer {
                KeyboardLayer::FastStartFailure => {
                    self.dismiss_fast_start_failure_notice();
                }
                KeyboardLayer::GpuConfirmation => {
                    self.pending_gpu_acceleration = None;
                }
                KeyboardLayer::BookmarkDeleteConfirmation => {
                    self.bookmark_delete_dialog = None;
                }
                KeyboardLayer::EdgePrompt => {
                    self.edge_prompt = None;
                }
                KeyboardLayer::BookmarkPopover => {
                    self.close_bookmark_popover();
                }
                KeyboardLayer::FileDeleteConfirmation | KeyboardLayer::Viewer => unreachable!(),
            },
            KeyboardRoute::PassToBookmarkPopover | KeyboardRoute::PassToViewer => {
                let actions = ctx.input(|input| collect_keyboard_actions(input, &self.settings));
                for action in actions {
                    if route == KeyboardRoute::PassToBookmarkPopover
                        && !focused_bookmark_popover_allows_action(action)
                    {
                        continue;
                    }
                    match action {
                        KeyboardAction::Command(command) => self.apply_command(ctx, command),
                        KeyboardAction::Release(release) => {
                            self.apply_navigation_key_release(release)
                        }
                    }
                    // A command can open or close an overlay. Once keyboard
                    // ownership changes, later events from this frame belong to
                    // the new layer rather than the old one.
                    if self.keyboard_layer() != layer {
                        break;
                    }
                }
            }
        }
    }

    pub(in crate::app) fn keyboard_layer(&self) -> KeyboardLayer {
        keyboard_layer_for(
            self.fast_start_failure_notice
                .as_ref()
                .is_some_and(|notice| !notice.shown),
            self.pending_delete_dialog.is_some(),
            self.pending_gpu_acceleration.is_some(),
            self.bookmark_delete_dialog.is_some(),
            self.edge_prompt.is_some(),
            self.bookmark_popover_open,
        )
    }

    fn apply_navigation_key_release(&mut self, release: NavigationRelease) {
        match release {
            NavigationRelease::PageTurn => self.clear_queued_page_turns(),
            NavigationRelease::SiblingBook => self.clear_queued_sibling_book_turns(),
        }
    }

    pub(in crate::app) fn apply_command(&mut self, ctx: &egui::Context, command: AppCommand) {
        // Strip mode reinterprets navigation/zoom commands as scrolls/jumps before
        // the paged handlers; unhandled commands fall through unchanged.
        if self.view_mode == ViewMode::VerticalStrip && self.apply_strip_keyboard_override(command)
        {
            return;
        }
        match command {
            AppCommand::OpenFile => self.open_file_dialog(),
            AppCommand::OpenFolder => self.open_folder_dialog(),
            AppCommand::CloseBook => {
                self.close_book("Closed current book.");
            }
            AppCommand::Quit => {
                self.close_requested = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            AppCommand::QuitFromEsc => {
                if self.settings.esc_to_quit {
                    self.close_requested = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                } else {
                    self.notify("ESC exit is disabled in settings.");
                }
            }
            AppCommand::ToggleFullscreen => self.toggle_fullscreen(ctx),
            AppCommand::ToggleMaximized => self.toggle_maximized(ctx),
            AppCommand::Minimize => ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true)),
            AppCommand::OpenSettings => {
                self.settings_open = true;
            }
            AppCommand::OpenAbout => self.open_about_window(),
            AppCommand::ToggleAlwaysOnTop => {
                let mut settings = self.settings.clone();
                settings.always_on_top = !settings.always_on_top;
                self.apply_settings(ctx, settings);
            }
            AppCommand::NextPage => self.next_page(),
            AppCommand::PreviousPage => self.previous_page(),
            AppCommand::MovePages(delta) => self.move_pages(delta),
            AppCommand::ForceMovePages(delta) => self.force_move_pages(delta),
            AppCommand::Home => self.set_page(0, NavigationDirection::Backward),
            AppCommand::End => {
                if let Some(source) = self.source.as_ref() {
                    self.set_page(
                        source.page_count().saturating_sub(1),
                        NavigationDirection::Forward,
                    );
                }
            }
            AppCommand::RandomForward => self.random_page(NavigationDirection::Forward),
            AppCommand::RandomBackward => self.random_page(NavigationDirection::Backward),
            AppCommand::NextBook => self.open_sibling_book(1),
            AppCommand::PreviousBook => self.open_sibling_book(-1),
            AppCommand::SetFitMode(mode) => self.set_fit_mode(mode),
            AppCommand::SetDouble(direction) => self.set_double_mode(direction),
            AppCommand::ToggleDouble => self.toggle_double_mode(),
            AppCommand::ToggleVerticalStrip => {
                let mode = if self.view_mode == ViewMode::VerticalStrip {
                    ViewMode::Single
                } else {
                    ViewMode::VerticalStrip
                };
                self.set_view_mode(mode);
            }
            AppCommand::Zoom(factor) => self.adjust_zoom(factor),
            AppCommand::ZoomFine(delta) => self.adjust_zoom_by_delta(delta),
            AppCommand::RotateClockwise => self.update_effects(|effects| {
                effects.transform = effects.transform.rotated_cw();
            }),
            AppCommand::RotateCounterClockwise => self.update_effects(|effects| {
                effects.transform = effects.transform.rotated_ccw();
            }),
            AppCommand::SetRotation(rotation) => self.update_effects(|effects| {
                effects.transform = effects.transform.with_rotation(rotation);
            }),
            AppCommand::ToggleFlipHorizontal => self.update_effects(|effects| {
                effects.transform.flip_horizontal = !effects.transform.flip_horizontal;
            }),
            AppCommand::ToggleFlipVertical => self.update_effects(|effects| {
                effects.transform.flip_vertical = !effects.transform.flip_vertical;
            }),
            AppCommand::ToggleInvert => self.update_effects(|effects| {
                effects.invert_colors = !effects.invert_colors;
            }),
            AppCommand::SetFilter(filter) => self.update_effects(|effects| {
                effects.filter = filter;
            }),
            AppCommand::ToggleGamma => self.update_effects(|effects| {
                effects.gamma = !effects.gamma;
            }),
            AppCommand::Delete(mode) => self.delete_current_file(mode),
            AppCommand::OpenExplorer => self.open_current_in_file_manager(),
            AppCommand::CopyPageImage => self.copy_current_page_image(),
            AppCommand::CopyDisplayImage => self.copy_current_spread_image(),
            AppCommand::CopyPath => self.copy_current_path(),
            AppCommand::ToggleCurrentPageBookmark => self.toggle_current_page_bookmark(),
            AppCommand::ToggleBookmarkPopover => self.toggle_bookmark_popover(ctx),
        }
    }

    pub(in crate::app) fn open_file_dialog(&mut self) {
        if let Some(path) = FileDialog::new()
            .add_filter("Images and comics", OPENABLE_FILE_EXTENSIONS)
            .pick_file()
        {
            self.open_path(path);
        }
    }

    pub(in crate::app) fn open_folder_dialog(&mut self) {
        if let Some(path) = FileDialog::new().pick_folder() {
            self.open_path(path);
        }
    }
}

pub(super) fn keyboard_layer_for(
    fast_start_failure: bool,
    file_delete_confirmation: bool,
    gpu_confirmation: bool,
    bookmark_delete_confirmation: bool,
    edge_prompt: bool,
    bookmark_popover: bool,
) -> KeyboardLayer {
    if fast_start_failure {
        KeyboardLayer::FastStartFailure
    } else if file_delete_confirmation {
        KeyboardLayer::FileDeleteConfirmation
    } else if gpu_confirmation {
        KeyboardLayer::GpuConfirmation
    } else if bookmark_delete_confirmation {
        KeyboardLayer::BookmarkDeleteConfirmation
    } else if edge_prompt {
        KeyboardLayer::EdgePrompt
    } else if bookmark_popover {
        KeyboardLayer::BookmarkPopover
    } else {
        KeyboardLayer::Viewer
    }
}

pub(super) fn keyboard_route_for(
    layer: KeyboardLayer,
    escape_pressed: bool,
    enter_pressed: bool,
    wants_keyboard_input: bool,
) -> KeyboardRoute {
    match layer {
        KeyboardLayer::FastStartFailure => {
            if escape_pressed || enter_pressed {
                KeyboardRoute::DismissOverlay
            } else {
                KeyboardRoute::Block
            }
        }
        // The file-delete dialog reads the same egui frame later and owns its
        // Escape, arrows, Tab, and Enter handling. Global shortcuts stop here.
        KeyboardLayer::FileDeleteConfirmation => KeyboardRoute::DelegateToOverlay,
        // Both dim the screen behind a full-area click blocker, so they own the
        // keyboard for as long as they are up.
        KeyboardLayer::GpuConfirmation | KeyboardLayer::BookmarkDeleteConfirmation => {
            if escape_pressed {
                KeyboardRoute::DismissOverlay
            } else {
                KeyboardRoute::Block
            }
        }
        // The edge prompt is a toast, not a dialog: it paints no scrim, blocks
        // no clicks, and dismisses itself after a timeout. Blocking the keyboard
        // for it stalled every page-turn and next-book key at the first and last
        // page -- and the prompt labels its own buttons with those very
        // shortcuts. Keyboard ownership follows pointer ownership, so only
        // Escape is consumed and the rest reaches the viewer, which clears the
        // prompt as it acts.
        KeyboardLayer::EdgePrompt => {
            if escape_pressed {
                KeyboardRoute::DismissOverlay
            } else {
                KeyboardRoute::PassToViewer
            }
        }
        KeyboardLayer::BookmarkPopover => {
            if escape_pressed {
                KeyboardRoute::DismissOverlay
            } else if wants_keyboard_input {
                KeyboardRoute::PassToBookmarkPopover
            } else {
                KeyboardRoute::PassToViewer
            }
        }
        KeyboardLayer::Viewer => KeyboardRoute::PassToViewer,
    }
}

pub(super) fn focused_bookmark_popover_allows_action(action: KeyboardAction) -> bool {
    matches!(
        action,
        KeyboardAction::Command(AppCommand::ToggleBookmarkPopover)
            | KeyboardAction::Release(NavigationRelease::PageTurn)
            | KeyboardAction::Release(NavigationRelease::SiblingBook)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlays_preempt_viewer_shortcuts_in_modal_order() {
        assert_eq!(
            keyboard_layer_for(true, true, true, true, true, true),
            KeyboardLayer::FastStartFailure
        );
        assert_eq!(
            keyboard_layer_for(false, true, true, true, true, true),
            KeyboardLayer::FileDeleteConfirmation
        );
        assert_eq!(
            keyboard_layer_for(false, false, true, true, true, true),
            KeyboardLayer::GpuConfirmation
        );
        assert_eq!(
            keyboard_layer_for(false, false, false, true, true, true),
            KeyboardLayer::BookmarkDeleteConfirmation
        );
        assert_eq!(
            keyboard_layer_for(false, false, false, false, true, true),
            KeyboardLayer::EdgePrompt
        );
        assert_eq!(
            keyboard_layer_for(false, false, false, false, false, true),
            KeyboardLayer::BookmarkPopover
        );
        assert_eq!(
            keyboard_layer_for(false, false, false, false, false, false),
            KeyboardLayer::Viewer
        );

        assert_eq!(
            keyboard_route_for(
                KeyboardLayer::BookmarkDeleteConfirmation,
                false,
                false,
                false,
            ),
            KeyboardRoute::Block
        );
        assert_eq!(
            keyboard_route_for(
                KeyboardLayer::BookmarkDeleteConfirmation,
                true,
                false,
                false,
            ),
            KeyboardRoute::DismissOverlay
        );
        assert_eq!(
            keyboard_route_for(KeyboardLayer::GpuConfirmation, true, false, false),
            KeyboardRoute::DismissOverlay
        );
        assert_eq!(
            keyboard_route_for(KeyboardLayer::FileDeleteConfirmation, false, false, false),
            KeyboardRoute::DelegateToOverlay
        );
        assert_eq!(
            keyboard_route_for(KeyboardLayer::BookmarkPopover, true, false, true),
            KeyboardRoute::DismissOverlay
        );
        assert_eq!(
            keyboard_route_for(KeyboardLayer::BookmarkPopover, false, false, true),
            KeyboardRoute::PassToBookmarkPopover
        );
        assert_eq!(
            keyboard_route_for(KeyboardLayer::BookmarkPopover, false, false, false),
            KeyboardRoute::PassToViewer
        );
        assert_eq!(
            keyboard_route_for(KeyboardLayer::Viewer, true, false, false),
            KeyboardRoute::PassToViewer
        );
    }

    /// The edge prompt paints no scrim and blocks no clicks, so it must not
    /// block keys either. It once routed every non-Escape key to `Block`, which
    /// froze page turns and next-book shortcuts for as long as the prompt was
    /// up -- and hovering it suspends the auto-dismiss, so that was unbounded.
    #[test]
    fn edge_prompt_consumes_escape_but_never_blocks_the_viewer() {
        assert_eq!(
            keyboard_route_for(KeyboardLayer::EdgePrompt, true, false, false),
            KeyboardRoute::DismissOverlay
        );
        for wants_keyboard_input in [false, true] {
            for enter_pressed in [false, true] {
                assert_eq!(
                    keyboard_route_for(
                        KeyboardLayer::EdgePrompt,
                        false,
                        enter_pressed,
                        wants_keyboard_input,
                    ),
                    KeyboardRoute::PassToViewer,
                    "the edge prompt must not swallow viewer keys",
                );
            }
        }
    }

    #[test]
    fn scrimmed_dialogs_still_own_the_keyboard() {
        for layer in [
            KeyboardLayer::GpuConfirmation,
            KeyboardLayer::BookmarkDeleteConfirmation,
        ] {
            assert_eq!(
                keyboard_route_for(layer, false, false, false),
                KeyboardRoute::Block
            );
            assert_eq!(
                keyboard_route_for(layer, true, false, false),
                KeyboardRoute::DismissOverlay
            );
        }
        assert_eq!(
            keyboard_route_for(KeyboardLayer::FastStartFailure, false, false, false),
            KeyboardRoute::Block
        );
    }

    #[test]
    fn focused_bookmark_popover_allows_only_navigation_and_its_own_toggle() {
        assert!(focused_bookmark_popover_allows_action(
            KeyboardAction::Command(AppCommand::ToggleBookmarkPopover)
        ));
        assert!(focused_bookmark_popover_allows_action(
            KeyboardAction::Release(NavigationRelease::PageTurn)
        ));
        assert!(focused_bookmark_popover_allows_action(
            KeyboardAction::Release(NavigationRelease::SiblingBook)
        ));
        assert!(!focused_bookmark_popover_allows_action(
            KeyboardAction::Command(AppCommand::ToggleCurrentPageBookmark)
        ));
    }
}
