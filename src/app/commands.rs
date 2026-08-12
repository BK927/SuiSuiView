use crate::core::effects::ImageFilter;
use crate::core::state::{
    AppSettings, CommandId, FitMode, KeyCode, KeyShortcut, MouseGesture, ReadingDirection,
};
use egui::{self, Key};

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum AppCommand {
    OpenFile,
    OpenFolder,
    CloseBook,
    Quit,
    QuitFromEsc,
    ToggleFullscreen,
    ToggleMaximized,
    Minimize,
    OpenSettings,
    OpenAbout,
    ToggleAlwaysOnTop,
    NextPage,
    PreviousPage,
    MovePages(isize),
    ForceMovePages(isize),
    Home,
    End,
    RandomForward,
    RandomBackward,
    NextBook,
    PreviousBook,
    SetFitMode(FitMode),
    SetDouble(ReadingDirection),
    ToggleDouble,
    ToggleVerticalStrip,
    Zoom(f32),
    ZoomFine(f32),
    RotateClockwise,
    RotateCounterClockwise,
    SetRotation(u8),
    ToggleFlipHorizontal,
    ToggleFlipVertical,
    ToggleInvert,
    SetFilter(ImageFilter),
    ToggleGamma,
    Delete(DeleteMode),
    OpenExplorer,
    CopyPageImage,
    CopyDisplayImage,
    CopyPath,
    ToggleCurrentPageBookmark,
    ToggleBookmarkPopover,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DeleteMode {
    Recycle,
    Permanent,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum KeyboardAction {
    Command(AppCommand),
    Release(NavigationRelease),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum NavigationRelease {
    PageTurn,
    SiblingBook,
}

pub(super) fn collect_keyboard_actions(
    input: &egui::InputState,
    settings: &AppSettings,
) -> Vec<KeyboardAction> {
    let mut actions = Vec::new();
    let mut shifted_asterisk_key = false;
    for event in &input.events {
        match event {
            egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } => {
                shifted_asterisk_key = false;
                let Some(shortcut) = shortcut_from_input_event(event, *modifiers) else {
                    continue;
                };
                shifted_asterisk_key = is_shifted_asterisk_key(*key, *modifiers);
                collect_shortcut_commands(shortcut, settings, &mut actions);
            }
            egui::Event::Key {
                key,
                pressed: false,
                ..
            } => {
                shifted_asterisk_key = false;
                let Some(key) = key_code_from_egui(*key) else {
                    continue;
                };
                collect_release_actions(key, settings, &mut actions);
            }
            egui::Event::Text(text) if text == "*" => {
                // egui emits both Shift+8 and Text("*") for the main keyboard,
                // but only Text("*") for numpad multiply. The key event already
                // dispatched the canonical asterisk shortcut, so suppress exactly
                // its immediately-following text event without suppressing the
                // numpad's text-only input.
                if std::mem::take(&mut shifted_asterisk_key) {
                    continue;
                }
                if let Some(shortcut) = shortcut_from_input_event(event, input.modifiers) {
                    collect_shortcut_commands(shortcut, settings, &mut actions);
                }
            }
            _ => shifted_asterisk_key = false,
        }
    }
    actions
}

fn collect_shortcut_commands(
    shortcut: KeyShortcut,
    settings: &AppSettings,
    actions: &mut Vec<KeyboardAction>,
) {
    for binding in &settings.key_bindings {
        if binding.shortcut == shortcut {
            if let Some(command) = app_command_for_id(binding.command) {
                actions.push(KeyboardAction::Command(command));
            }
        }
    }
}

fn collect_release_actions(
    key: KeyCode,
    settings: &AppSettings,
    actions: &mut Vec<KeyboardAction>,
) {
    let mut page_turn = false;
    let mut sibling_book = false;
    for binding in &settings.key_bindings {
        if binding.shortcut.key != key {
            continue;
        }
        match release_for_command(binding.command) {
            Some(NavigationRelease::PageTurn) => page_turn = true,
            Some(NavigationRelease::SiblingBook) => sibling_book = true,
            None => {}
        }
    }

    if page_turn {
        actions.push(KeyboardAction::Release(NavigationRelease::PageTurn));
    }
    if sibling_book {
        actions.push(KeyboardAction::Release(NavigationRelease::SiblingBook));
    }
}

fn release_for_command(command: CommandId) -> Option<NavigationRelease> {
    Some(match command {
        CommandId::NextPage
        | CommandId::PreviousPage
        | CommandId::MoveForward10
        | CommandId::MoveBackward10
        | CommandId::MoveForward100
        | CommandId::MoveBackward100
        | CommandId::ForceNextPage
        | CommandId::ForcePreviousPage
        | CommandId::Home
        | CommandId::End
        | CommandId::RandomForward
        | CommandId::RandomBackward => NavigationRelease::PageTurn,
        CommandId::NextBook | CommandId::PreviousBook => NavigationRelease::SiblingBook,
        _ => return None,
    })
}

#[cfg(test)]
pub(super) fn command_for_shortcut(
    shortcut: KeyShortcut,
    settings: &AppSettings,
) -> Option<AppCommand> {
    settings
        .key_bindings
        .iter()
        .find(|binding| binding.shortcut == shortcut)
        .and_then(|binding| app_command_for_id(binding.command))
}

pub(super) fn command_for_mouse_gesture(
    gesture: MouseGesture,
    settings: &AppSettings,
) -> Option<AppCommand> {
    settings
        .mouse_bindings
        .iter()
        .find(|binding| binding.gesture == gesture)
        .and_then(|binding| app_command_for_id(binding.command))
}

pub(super) fn app_command_for_id(command: CommandId) -> Option<AppCommand> {
    Some(match command {
        CommandId::OpenFile => AppCommand::OpenFile,
        CommandId::OpenFolder => AppCommand::OpenFolder,
        CommandId::CloseBook => AppCommand::CloseBook,
        CommandId::Quit => AppCommand::Quit,
        CommandId::QuitFromEsc => AppCommand::QuitFromEsc,
        CommandId::ToggleFullscreen => AppCommand::ToggleFullscreen,
        CommandId::ToggleMaximized => AppCommand::ToggleMaximized,
        CommandId::Minimize => AppCommand::Minimize,
        CommandId::OpenSettings => AppCommand::OpenSettings,
        CommandId::OpenAbout => AppCommand::OpenAbout,
        CommandId::ToggleAlwaysOnTop => AppCommand::ToggleAlwaysOnTop,
        CommandId::NextPage => AppCommand::NextPage,
        CommandId::PreviousPage => AppCommand::PreviousPage,
        CommandId::MoveForward10 => AppCommand::MovePages(10),
        CommandId::MoveBackward10 => AppCommand::MovePages(-10),
        CommandId::MoveForward100 => AppCommand::MovePages(100),
        CommandId::MoveBackward100 => AppCommand::MovePages(-100),
        CommandId::ForceNextPage => AppCommand::ForceMovePages(1),
        CommandId::ForcePreviousPage => AppCommand::ForceMovePages(-1),
        CommandId::Home => AppCommand::Home,
        CommandId::End => AppCommand::End,
        CommandId::RandomForward => AppCommand::RandomForward,
        CommandId::RandomBackward => AppCommand::RandomBackward,
        CommandId::NextBook => AppCommand::NextBook,
        CommandId::PreviousBook => AppCommand::PreviousBook,
        CommandId::FitOriginal => AppCommand::SetFitMode(FitMode::Original),
        CommandId::FitPage => AppCommand::SetFitMode(FitMode::FitPage),
        CommandId::FitWidth => AppCommand::SetFitMode(FitMode::FitWidth),
        CommandId::FitHeight => AppCommand::SetFitMode(FitMode::FitHeight),
        CommandId::SetDoubleLeftToRight => AppCommand::SetDouble(ReadingDirection::LeftToRight),
        CommandId::SetDoubleRightToLeft => AppCommand::SetDouble(ReadingDirection::RightToLeft),
        CommandId::ToggleDouble => AppCommand::ToggleDouble,
        CommandId::ToggleVerticalStrip => AppCommand::ToggleVerticalStrip,
        CommandId::ZoomIn => AppCommand::Zoom(1.1),
        CommandId::ZoomOut => AppCommand::Zoom(0.9),
        CommandId::ZoomFineIn => AppCommand::ZoomFine(0.01),
        CommandId::ZoomFineOut => AppCommand::ZoomFine(-0.01),
        CommandId::RotateClockwise => AppCommand::RotateClockwise,
        CommandId::RotateCounterClockwise => AppCommand::RotateCounterClockwise,
        CommandId::Rotate0 => AppCommand::SetRotation(0),
        CommandId::Rotate90 => AppCommand::SetRotation(1),
        CommandId::Rotate180 => AppCommand::SetRotation(2),
        CommandId::Rotate270 => AppCommand::SetRotation(3),
        CommandId::FlipHorizontal => AppCommand::ToggleFlipHorizontal,
        CommandId::FlipVertical => AppCommand::ToggleFlipVertical,
        CommandId::ToggleInvert => AppCommand::ToggleInvert,
        CommandId::FilterNone => AppCommand::SetFilter(ImageFilter::None),
        CommandId::FilterSmooth => AppCommand::SetFilter(ImageFilter::Smooth),
        CommandId::FilterSmoothSharpen => AppCommand::SetFilter(ImageFilter::SmoothSharpen),
        CommandId::ToggleGamma => AppCommand::ToggleGamma,
        CommandId::DeleteRecycle => AppCommand::Delete(DeleteMode::Recycle),
        CommandId::DeletePermanent => AppCommand::Delete(DeleteMode::Permanent),
        CommandId::OpenExplorer => AppCommand::OpenExplorer,
        CommandId::CopyPageImage => AppCommand::CopyPageImage,
        CommandId::CopyDisplayImage => AppCommand::CopyDisplayImage,
        CommandId::CopyPath => AppCommand::CopyPath,
        CommandId::ToggleCurrentPageBookmark => AppCommand::ToggleCurrentPageBookmark,
        CommandId::ToggleBookmarkPopover => AppCommand::ToggleBookmarkPopover,
    })
}

pub(super) fn shortcut_from_input_event(
    event: &egui::Event,
    modifiers: egui::Modifiers,
) -> Option<KeyShortcut> {
    match event {
        egui::Event::Key {
            key, pressed: true, ..
        } => key_code_from_egui(*key).map(|key| shortcut_from_parts(key, modifiers)),
        egui::Event::Text(text) if text == "*" => Some(asterisk_shortcut(modifiers)),
        _ => None,
    }
}

pub(super) fn key_code_from_egui(key: Key) -> Option<KeyCode> {
    Some(match key {
        Key::F1 => KeyCode::F1,
        Key::F2 => KeyCode::F2,
        Key::F3 => KeyCode::F3,
        Key::F4 => KeyCode::F4,
        Key::F5 => KeyCode::F5,
        Key::F11 => KeyCode::F11,
        Key::A => KeyCode::A,
        Key::B => KeyCode::B,
        Key::C => KeyCode::C,
        Key::E => KeyCode::E,
        Key::F => KeyCode::F,
        Key::G => KeyCode::G,
        Key::I => KeyCode::I,
        Key::K => KeyCode::K,
        Key::L => KeyCode::L,
        Key::M => KeyCode::M,
        Key::N => KeyCode::N,
        Key::O => KeyCode::O,
        Key::P => KeyCode::P,
        Key::Q => KeyCode::Q,
        Key::R => KeyCode::R,
        Key::S => KeyCode::S,
        Key::T => KeyCode::T,
        Key::U => KeyCode::U,
        Key::W => KeyCode::W,
        Key::X => KeyCode::X,
        Key::Z => KeyCode::Z,
        Key::Escape => KeyCode::Escape,
        Key::Enter => KeyCode::Enter,
        Key::Space => KeyCode::Space,
        Key::Backspace => KeyCode::Backspace,
        Key::Delete => KeyCode::Delete,
        Key::Insert => KeyCode::Insert,
        Key::Tab => KeyCode::Tab,
        Key::PageDown => KeyCode::PageDown,
        Key::PageUp => KeyCode::PageUp,
        Key::ArrowDown => KeyCode::ArrowDown,
        Key::ArrowLeft => KeyCode::ArrowLeft,
        Key::ArrowRight => KeyCode::ArrowRight,
        Key::ArrowUp => KeyCode::ArrowUp,
        Key::Home => KeyCode::Home,
        Key::End => KeyCode::End,
        Key::OpenBracket => KeyCode::OpenBracket,
        Key::CloseBracket => KeyCode::CloseBracket,
        Key::Backtick => KeyCode::Backtick,
        Key::Slash => KeyCode::Slash,
        Key::Plus => KeyCode::Plus,
        Key::Equals => KeyCode::Equals,
        Key::Minus => KeyCode::Minus,
        Key::Num0 => KeyCode::Num0,
        Key::Num1 => KeyCode::Num1,
        Key::Num2 => KeyCode::Num2,
        Key::Num3 => KeyCode::Num3,
        Key::Num4 => KeyCode::Num4,
        Key::Num5 => KeyCode::Num5,
        Key::Num6 => KeyCode::Num6,
        Key::Num7 => KeyCode::Num7,
        Key::Num8 => KeyCode::Num8,
        Key::Num9 => KeyCode::Num9,
        _ => return None,
    })
}

fn shortcut_from_parts(key: KeyCode, modifiers: egui::Modifiers) -> KeyShortcut {
    if key == KeyCode::Num8 && modifiers.shift {
        return asterisk_shortcut(modifiers);
    }
    KeyShortcut {
        key,
        ctrl: modifiers.ctrl,
        alt: modifiers.alt,
        shift: modifiers.shift,
    }
}

fn asterisk_shortcut(modifiers: egui::Modifiers) -> KeyShortcut {
    KeyShortcut {
        key: KeyCode::Asterisk,
        ctrl: modifiers.ctrl,
        alt: modifiers.alt,
        // Shift produces the `*` character on the main keyboard; it is not a
        // separate modifier in the persisted logical shortcut.
        shift: false,
    }
}

fn is_shifted_asterisk_key(key: Key, modifiers: egui::Modifiers) -> bool {
    key == Key::Num8 && modifiers.shift
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::state::{default_key_bindings, default_mouse_bindings, KeyBinding};

    #[test]
    fn default_shortcuts_match_existing_core_commands() {
        let settings = AppSettings {
            key_bindings: default_key_bindings(),
            ..AppSettings::default()
        };

        assert_eq!(
            command_for_shortcut(KeyShortcut::new(KeyCode::F2), &settings),
            Some(AppCommand::OpenFile)
        );
        assert_eq!(
            command_for_shortcut(KeyShortcut::new(KeyCode::PageDown), &settings),
            Some(AppCommand::NextPage)
        );
        assert_eq!(
            command_for_shortcut(KeyShortcut::ctrl(KeyCode::B), &settings),
            Some(AppCommand::ToggleBookmarkPopover)
        );
        assert_eq!(
            command_for_shortcut(KeyShortcut::ctrl(KeyCode::A), &settings),
            Some(AppCommand::ToggleAlwaysOnTop)
        );
    }

    #[test]
    fn default_mouse_bindings_cover_primary_viewer_gestures() {
        let settings = AppSettings {
            mouse_bindings: default_mouse_bindings(),
            ..AppSettings::default()
        };

        assert_eq!(
            command_for_mouse_gesture(MouseGesture::DoubleClick, &settings),
            Some(AppCommand::ToggleMaximized)
        );
        assert_eq!(
            command_for_mouse_gesture(MouseGesture::WheelDown, &settings),
            Some(AppCommand::NextPage)
        );
    }

    #[test]
    fn keyboard_actions_keep_repeat_page_turn_commands() {
        let settings = AppSettings {
            key_bindings: default_key_bindings(),
            ..AppSettings::default()
        };
        let input = input_with_events(vec![key_event(
            Key::ArrowRight,
            true,
            true,
            egui::Modifiers::default(),
        )]);

        assert_eq!(
            collect_keyboard_actions(&input, &settings),
            vec![KeyboardAction::Command(AppCommand::NextPage)]
        );
    }

    #[test]
    fn keyboard_actions_release_page_turn_keys() {
        let settings = AppSettings {
            key_bindings: default_key_bindings(),
            ..AppSettings::default()
        };
        let input = input_with_events(vec![key_event(
            Key::ArrowRight,
            false,
            false,
            egui::Modifiers::default(),
        )]);

        assert_eq!(
            collect_keyboard_actions(&input, &settings),
            vec![KeyboardAction::Release(NavigationRelease::PageTurn)]
        );
    }

    #[test]
    fn keyboard_actions_release_navigation_keys_without_matching_modifiers() {
        let settings = AppSettings {
            key_bindings: vec![KeyBinding {
                command: CommandId::MoveForward10,
                shortcut: KeyShortcut::ctrl(KeyCode::PageDown),
            }],
            ..AppSettings::default()
        };
        let input = input_with_events(vec![key_event(
            Key::PageDown,
            false,
            false,
            egui::Modifiers::default(),
        )]);

        assert_eq!(
            collect_keyboard_actions(&input, &settings),
            vec![KeyboardAction::Release(NavigationRelease::PageTurn)]
        );
    }

    #[test]
    fn keyboard_actions_release_sibling_book_keys() {
        let settings = AppSettings {
            key_bindings: default_key_bindings(),
            ..AppSettings::default()
        };
        let input = input_with_events(vec![key_event(
            Key::CloseBracket,
            false,
            false,
            egui::Modifiers::default(),
        )]);

        assert_eq!(
            collect_keyboard_actions(&input, &settings),
            vec![KeyboardAction::Release(NavigationRelease::SiblingBook)]
        );
    }

    #[test]
    fn keyboard_actions_ignore_unbound_key_release() {
        let settings = AppSettings {
            key_bindings: default_key_bindings(),
            ..AppSettings::default()
        };
        let input = input_with_events(vec![key_event(
            Key::G,
            false,
            false,
            egui::Modifiers::default(),
        )]);

        assert!(collect_keyboard_actions(&input, &settings).is_empty());
    }

    #[test]
    fn shift_8_key_and_text_pair_dispatches_asterisk_once() {
        let settings = settings_with_binding(KeyShortcut::new(KeyCode::Asterisk));
        let shift = modifiers(false, false, true);
        let input = input_with_events_and_modifiers(
            vec![
                key_event(Key::Num8, true, false, shift),
                egui::Event::Text("*".to_owned()),
            ],
            shift,
        );

        assert_eq!(
            collect_keyboard_actions(&input, &settings),
            vec![KeyboardAction::Command(AppCommand::SetFitMode(
                FitMode::Original
            ))]
        );
    }

    #[test]
    fn text_only_numpad_multiply_dispatches_asterisk() {
        let settings = settings_with_binding(KeyShortcut::new(KeyCode::Asterisk));
        let input = input_with_events(vec![egui::Event::Text("*".to_owned())]);

        assert_eq!(
            collect_keyboard_actions(&input, &settings),
            vec![KeyboardAction::Command(AppCommand::SetFitMode(
                FitMode::Original
            ))]
        );
    }

    #[test]
    fn shifted_asterisk_keeps_ctrl_and_alt_but_not_character_shift() {
        let ctrl_alt_shift = modifiers(true, true, true);
        let key = key_event(Key::Num8, true, false, ctrl_alt_shift);
        let text = egui::Event::Text("*".to_owned());
        let expected = KeyShortcut::ctrl_alt(KeyCode::Asterisk);

        assert_eq!(
            shortcut_from_input_event(&key, ctrl_alt_shift),
            Some(expected)
        );
        assert_eq!(
            shortcut_from_input_event(&text, ctrl_alt_shift),
            Some(expected)
        );
    }

    #[test]
    fn ctrl_and_alt_non_asterisk_shortcuts_stay_unchanged() {
        let ctrl = modifiers(true, false, false);
        let alt = modifiers(false, true, false);

        assert_eq!(
            shortcut_from_input_event(&key_event(Key::B, true, false, ctrl), ctrl),
            Some(KeyShortcut::ctrl(KeyCode::B))
        );
        assert_eq!(
            shortcut_from_input_event(&key_event(Key::Num8, true, false, alt), alt),
            Some(KeyShortcut::alt(KeyCode::Num8))
        );
    }

    #[test]
    fn ctrl_asterisk_key_event_works_without_a_text_event() {
        let settings = settings_with_binding(KeyShortcut::ctrl(KeyCode::Asterisk));
        let ctrl_shift = modifiers(true, false, true);
        // egui-winit intentionally omits Text events while Ctrl is held.
        let input = input_with_events(vec![key_event(Key::Num8, true, false, ctrl_shift)]);

        assert_eq!(
            collect_keyboard_actions(&input, &settings),
            vec![KeyboardAction::Command(AppCommand::SetFitMode(
                FitMode::Original
            ))]
        );
    }

    fn settings_with_binding(shortcut: KeyShortcut) -> AppSettings {
        AppSettings {
            key_bindings: vec![KeyBinding {
                command: CommandId::FitOriginal,
                shortcut,
            }],
            ..AppSettings::default()
        }
    }

    fn modifiers(ctrl: bool, alt: bool, shift: bool) -> egui::Modifiers {
        egui::Modifiers {
            ctrl,
            alt,
            shift,
            ..egui::Modifiers::default()
        }
    }

    fn input_with_events(events: Vec<egui::Event>) -> egui::InputState {
        input_with_events_and_modifiers(events, egui::Modifiers::default())
    }

    fn input_with_events_and_modifiers(
        events: Vec<egui::Event>,
        modifiers: egui::Modifiers,
    ) -> egui::InputState {
        let mut input = egui::InputState::default();
        input.events = events;
        input.modifiers = modifiers;
        input
    }

    fn key_event(key: Key, pressed: bool, repeat: bool, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat,
            modifiers,
        }
    }
}
