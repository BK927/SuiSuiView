use crate::core::effects::ImageFilter;
use crate::core::state::{FitMode, ReadingDirection};
use eframe::egui::{self, Key};

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
    UpscaleCurrentPage,
    ToggleCurrentPageBookmark,
    ToggleBookmarkPopover,
    Unsupported(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DeleteMode {
    Recycle,
    Permanent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Shortcut {
    pub(super) key: ShortcutKey,
    pub(super) ctrl: bool,
    pub(super) alt: bool,
    pub(super) shift: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ShortcutKey {
    Egui(Key),
}

impl Shortcut {
    fn egui(key: Key, modifiers: egui::Modifiers) -> Self {
        Self {
            key: ShortcutKey::Egui(key),
            ctrl: modifiers.ctrl,
            alt: modifiers.alt,
            shift: modifiers.shift,
        }
    }

    #[cfg(test)]
    pub(super) fn key(key: Key) -> Self {
        Self {
            key: ShortcutKey::Egui(key),
            ctrl: false,
            alt: false,
            shift: false,
        }
    }

    #[cfg(test)]
    pub(super) fn ctrl(key: Key) -> Self {
        Self {
            key: ShortcutKey::Egui(key),
            ctrl: true,
            alt: false,
            shift: false,
        }
    }

    #[cfg(test)]
    pub(super) fn alt(key: Key) -> Self {
        Self {
            key: ShortcutKey::Egui(key),
            ctrl: false,
            alt: true,
            shift: false,
        }
    }
}

pub(super) fn collect_keyboard_commands(input: &egui::InputState) -> Vec<AppCommand> {
    let mut commands = Vec::new();
    for key in TRACKED_KEYS {
        if input.key_pressed(*key) {
            let shortcut = Shortcut::egui(*key, input.modifiers);
            if let Some(command) = command_for_shortcut(shortcut) {
                commands.push(command);
            }
        }
    }
    for event in &input.events {
        if let egui::Event::Text(text) = event {
            for ch in text.chars() {
                if ch == '*' {
                    commands.push(AppCommand::SetFitMode(FitMode::Original));
                }
            }
        }
    }
    commands
}

const TRACKED_KEYS: &[Key] = &[
    Key::F1,
    Key::F2,
    Key::F3,
    Key::F4,
    Key::F5,
    Key::F11,
    Key::O,
    Key::F,
    Key::Escape,
    Key::X,
    Key::W,
    Key::Enter,
    Key::N,
    Key::M,
    Key::Q,
    Key::PageDown,
    Key::PageUp,
    Key::ArrowDown,
    Key::ArrowRight,
    Key::ArrowUp,
    Key::ArrowLeft,
    Key::Space,
    Key::Backspace,
    Key::Home,
    Key::End,
    Key::OpenBracket,
    Key::CloseBracket,
    Key::Num0,
    Key::Num1,
    Key::Num2,
    Key::Num3,
    Key::Num4,
    Key::Num5,
    Key::Num6,
    Key::Num7,
    Key::Num8,
    Key::Num9,
    Key::Z,
    Key::Plus,
    Key::Equals,
    Key::Minus,
    Key::I,
    Key::G,
    Key::L,
    Key::R,
    Key::U,
    Key::S,
    Key::Delete,
    Key::C,
    Key::Tab,
    Key::Insert,
    Key::E,
    Key::P,
    Key::K,
    Key::Slash,
    Key::B,
    Key::A,
    Key::Backtick,
    Key::T,
];

pub(super) fn command_for_shortcut(shortcut: Shortcut) -> Option<AppCommand> {
    let Shortcut {
        key,
        ctrl,
        alt,
        shift,
    } = shortcut;
    let plain = !ctrl && !alt && !shift;

    match key {
        ShortcutKey::Egui(Key::F2) if plain => Some(AppCommand::OpenFile),
        ShortcutKey::Egui(Key::O) if ctrl && !alt && !shift => Some(AppCommand::OpenFile),
        ShortcutKey::Egui(Key::F) if plain => Some(AppCommand::OpenFolder),
        ShortcutKey::Egui(Key::F4) if plain => Some(AppCommand::CloseBook),
        ShortcutKey::Egui(Key::Escape) if plain => Some(AppCommand::QuitFromEsc),
        ShortcutKey::Egui(Key::X) if plain => Some(AppCommand::Quit),
        ShortcutKey::Egui(Key::W) if ctrl && !alt && !shift => Some(AppCommand::Quit),
        ShortcutKey::Egui(Key::F11) if plain => Some(AppCommand::ToggleFullscreen),
        ShortcutKey::Egui(Key::Enter) if alt && !ctrl && !shift => {
            Some(AppCommand::ToggleFullscreen)
        }
        ShortcutKey::Egui(Key::N) if plain => Some(AppCommand::ToggleFullscreen),
        ShortcutKey::Egui(Key::M) if plain => Some(AppCommand::ToggleMaximized),
        ShortcutKey::Egui(Key::Q) if plain => Some(AppCommand::Minimize),
        ShortcutKey::Egui(Key::Enter) if ctrl && !alt && !shift => Some(AppCommand::OpenExplorer),
        ShortcutKey::Egui(Key::Enter) if plain => Some(AppCommand::Unsupported("Image selection")),

        ShortcutKey::Egui(Key::PageDown) if ctrl && alt && !shift => {
            Some(AppCommand::RandomForward)
        }
        ShortcutKey::Egui(Key::PageUp) if ctrl && alt && !shift => Some(AppCommand::RandomBackward),
        ShortcutKey::Egui(Key::ArrowRight) if ctrl && shift && !alt => {
            Some(AppCommand::MovePages(100))
        }
        ShortcutKey::Egui(Key::ArrowLeft) if ctrl && shift && !alt => {
            Some(AppCommand::MovePages(-100))
        }
        ShortcutKey::Egui(Key::PageDown) if ctrl && !alt && !shift => {
            Some(AppCommand::MovePages(10))
        }
        ShortcutKey::Egui(Key::PageUp) if ctrl && !alt && !shift => {
            Some(AppCommand::MovePages(-10))
        }
        ShortcutKey::Egui(Key::PageDown) if shift && !ctrl && !alt => {
            Some(AppCommand::ForceMovePages(1))
        }
        ShortcutKey::Egui(Key::PageUp) if shift && !ctrl && !alt => {
            Some(AppCommand::ForceMovePages(-1))
        }
        ShortcutKey::Egui(Key::PageDown | Key::ArrowDown | Key::ArrowRight) if plain => {
            Some(AppCommand::NextPage)
        }
        ShortcutKey::Egui(Key::Space) if shift && !ctrl && !alt => Some(AppCommand::PreviousPage),
        ShortcutKey::Egui(Key::Space) if plain => Some(AppCommand::NextPage),
        ShortcutKey::Egui(Key::PageUp | Key::ArrowUp | Key::ArrowLeft | Key::Backspace)
            if plain =>
        {
            Some(AppCommand::PreviousPage)
        }
        ShortcutKey::Egui(Key::Home) if plain => Some(AppCommand::Home),
        ShortcutKey::Egui(Key::End) if plain => Some(AppCommand::End),
        ShortcutKey::Egui(Key::CloseBracket) if plain => Some(AppCommand::NextBook),
        ShortcutKey::Egui(Key::OpenBracket) if plain => Some(AppCommand::PreviousBook),

        ShortcutKey::Egui(Key::Num0) if plain => Some(AppCommand::SetFitMode(FitMode::Original)),
        ShortcutKey::Egui(Key::Num1 | Key::Num9 | Key::Z) if plain => {
            Some(AppCommand::SetFitMode(FitMode::FitPage))
        }
        ShortcutKey::Egui(Key::Num8) if plain => Some(AppCommand::SetFitMode(FitMode::FitWidth)),
        ShortcutKey::Egui(Key::Num7) if plain => {
            Some(AppCommand::SetDouble(ReadingDirection::LeftToRight))
        }
        ShortcutKey::Egui(Key::Num6) if plain => {
            Some(AppCommand::SetDouble(ReadingDirection::RightToLeft))
        }
        ShortcutKey::Egui(Key::Num2) if plain => Some(AppCommand::ToggleDouble),
        ShortcutKey::Egui(Key::Plus | Key::Equals) if ctrl && !alt && !shift => {
            Some(AppCommand::ZoomFine(0.01))
        }
        ShortcutKey::Egui(Key::Minus) if ctrl && !alt && !shift => {
            Some(AppCommand::ZoomFine(-0.01))
        }
        ShortcutKey::Egui(Key::Plus | Key::Equals) if plain || shift && !ctrl && !alt => {
            Some(AppCommand::Zoom(1.1))
        }
        ShortcutKey::Egui(Key::Minus) if plain => Some(AppCommand::Zoom(0.9)),
        ShortcutKey::Egui(Key::I) if ctrl && !alt && !shift => Some(AppCommand::ToggleInvert),
        ShortcutKey::Egui(Key::M) if ctrl && !alt && !shift => {
            Some(AppCommand::ToggleFlipHorizontal)
        }
        ShortcutKey::Egui(Key::F) if ctrl && !alt && !shift => Some(AppCommand::ToggleFlipVertical),
        ShortcutKey::Egui(Key::L) if ctrl && !alt && !shift => {
            Some(AppCommand::RotateCounterClockwise)
        }
        ShortcutKey::Egui(Key::R) if ctrl && !alt && !shift => Some(AppCommand::RotateClockwise),
        ShortcutKey::Egui(Key::ArrowUp) if alt && !ctrl && !shift => {
            Some(AppCommand::SetRotation(0))
        }
        ShortcutKey::Egui(Key::ArrowLeft) if alt && !ctrl && !shift => {
            Some(AppCommand::SetRotation(3))
        }
        ShortcutKey::Egui(Key::ArrowRight) if alt && !ctrl && !shift => {
            Some(AppCommand::SetRotation(1))
        }
        ShortcutKey::Egui(Key::ArrowDown) if alt && !ctrl && !shift => {
            Some(AppCommand::SetRotation(2))
        }
        ShortcutKey::Egui(Key::U) if plain => Some(AppCommand::SetFilter(ImageFilter::None)),
        ShortcutKey::Egui(Key::I) if plain => Some(AppCommand::SetFilter(ImageFilter::Smooth)),
        ShortcutKey::Egui(Key::S) if plain => {
            Some(AppCommand::SetFilter(ImageFilter::SmoothSharpen))
        }
        ShortcutKey::Egui(Key::G) if ctrl && !alt && !shift => Some(AppCommand::ToggleGamma),

        ShortcutKey::Egui(Key::Delete) if shift && !ctrl && !alt => {
            Some(AppCommand::Delete(DeleteMode::Permanent))
        }
        ShortcutKey::Egui(Key::Delete) if plain => Some(AppCommand::Delete(DeleteMode::Recycle)),
        ShortcutKey::Egui(Key::C) if ctrl && alt && shift => Some(AppCommand::CopyPath),
        ShortcutKey::Egui(Key::C) if ctrl && alt && !shift => Some(AppCommand::CopyDisplayImage),
        ShortcutKey::Egui(Key::C) if ctrl && !alt && !shift => Some(AppCommand::CopyPageImage),

        ShortcutKey::Egui(Key::Tab) if plain => Some(AppCommand::Unsupported("EXIF/file info")),
        ShortcutKey::Egui(Key::W) if ctrl && alt && !shift => {
            Some(AppCommand::Unsupported("Wallpaper"))
        }
        ShortcutKey::Egui(Key::Insert) if plain => {
            Some(AppCommand::Unsupported("Photo storage copy"))
        }
        ShortcutKey::Egui(Key::Insert) if ctrl && !alt && !shift => {
            Some(AppCommand::Unsupported("Photo storage move"))
        }
        ShortcutKey::Egui(Key::Insert) if shift && !ctrl && !alt => {
            Some(AppCommand::Unsupported("Secondary photo storage copy"))
        }
        ShortcutKey::Egui(Key::E) if ctrl && !alt && !shift => {
            Some(AppCommand::Unsupported("External editor"))
        }
        ShortcutKey::Egui(Key::P) if ctrl && !alt && !shift => {
            Some(AppCommand::Unsupported("Print"))
        }
        ShortcutKey::Egui(Key::F5) if plain => Some(AppCommand::OpenSettings),
        ShortcutKey::Egui(Key::F1) if plain => Some(AppCommand::OpenAbout),
        ShortcutKey::Egui(Key::K) if ctrl && !alt && !shift => {
            Some(AppCommand::Unsupported("Zoom lock"))
        }
        ShortcutKey::Egui(Key::Slash) if plain => Some(AppCommand::Unsupported("Zoom preview")),
        ShortcutKey::Egui(Key::PageDown) if alt && !ctrl && !shift => {
            Some(AppCommand::Unsupported("Next image in multi-image file"))
        }
        ShortcutKey::Egui(Key::PageUp) if alt && !ctrl && !shift => Some(AppCommand::Unsupported(
            "Previous image in multi-image file",
        )),
        ShortcutKey::Egui(Key::Num0) if ctrl && !alt && !shift => {
            Some(AppCommand::Unsupported("Stop slideshow"))
        }
        ShortcutKey::Egui(
            Key::Num1
            | Key::Num2
            | Key::Num3
            | Key::Num4
            | Key::Num5
            | Key::Num6
            | Key::Num7
            | Key::Num8
            | Key::Num9,
        ) if ctrl && !alt && !shift => Some(AppCommand::Unsupported("Slideshow")),
        ShortcutKey::Egui(Key::B) if plain => Some(AppCommand::ToggleCurrentPageBookmark),
        ShortcutKey::Egui(Key::F3) if plain => Some(AppCommand::Unsupported("Bookmark edit")),
        ShortcutKey::Egui(Key::B) if ctrl && !alt && !shift => {
            Some(AppCommand::ToggleBookmarkPopover)
        }
        ShortcutKey::Egui(Key::A) if ctrl && !alt && !shift => Some(AppCommand::ToggleAlwaysOnTop),
        ShortcutKey::Egui(Key::Backtick) if plain => Some(AppCommand::Unsupported("Pin top menu")),
        ShortcutKey::Egui(Key::Backtick) if shift && !ctrl && !alt => {
            Some(AppCommand::Unsupported("Pin bottom menu"))
        }
        ShortcutKey::Egui(Key::F5) if ctrl && !alt && !shift => {
            Some(AppCommand::Unsupported("Reload skin"))
        }
        ShortcutKey::Egui(Key::N) if ctrl && !alt && !shift => {
            Some(AppCommand::Unsupported("New window"))
        }
        ShortcutKey::Egui(Key::F5) if shift && !ctrl && !alt => {
            Some(AppCommand::Unsupported("Window border"))
        }
        ShortcutKey::Egui(Key::C) if ctrl && shift && !alt => {
            Some(AppCommand::Unsupported("Copy EXIF"))
        }
        ShortcutKey::Egui(Key::G) if ctrl && shift && !alt => {
            Some(AppCommand::Unsupported("Open EXIF map"))
        }
        ShortcutKey::Egui(Key::T) if ctrl && !alt && !shift => {
            Some(AppCommand::Unsupported("Image conversion"))
        }
        _ => None,
    }
}
