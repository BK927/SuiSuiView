use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u16)]
#[serde(rename_all = "snake_case")]
pub enum CommandId {
    OpenFile = 800,
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
    MoveForward10,
    MoveBackward10,
    MoveForward100,
    MoveBackward100,
    ForceNextPage,
    ForcePreviousPage,
    Home,
    End,
    RandomForward,
    RandomBackward,
    NextBook,
    PreviousBook,
    FitOriginal,
    FitPage,
    FitWidth,
    FitHeight,
    SetDoubleLeftToRight,
    SetDoubleRightToLeft,
    ToggleDouble,
    ZoomIn,
    ZoomOut,
    ZoomFineIn,
    ZoomFineOut,
    RotateClockwise,
    RotateCounterClockwise,
    Rotate0,
    Rotate90,
    Rotate180,
    Rotate270,
    FlipHorizontal,
    FlipVertical,
    ToggleInvert,
    FilterNone,
    FilterSmooth,
    FilterSmoothSharpen,
    ToggleGamma,
    DeleteRecycle,
    DeletePermanent,
    OpenExplorer,
    CopyPageImage,
    CopyDisplayImage,
    CopyPath,
    UpscaleCurrentPage,
    ToggleCurrentPageBookmark,
    ToggleBookmarkPopover,
}

impl CommandId {
    pub const ALL: [Self; 58] = [
        Self::OpenFile,
        Self::OpenFolder,
        Self::CloseBook,
        Self::Quit,
        Self::QuitFromEsc,
        Self::ToggleFullscreen,
        Self::ToggleMaximized,
        Self::Minimize,
        Self::OpenSettings,
        Self::OpenAbout,
        Self::ToggleAlwaysOnTop,
        Self::NextPage,
        Self::PreviousPage,
        Self::MoveForward10,
        Self::MoveBackward10,
        Self::MoveForward100,
        Self::MoveBackward100,
        Self::ForceNextPage,
        Self::ForcePreviousPage,
        Self::Home,
        Self::End,
        Self::RandomForward,
        Self::RandomBackward,
        Self::NextBook,
        Self::PreviousBook,
        Self::FitOriginal,
        Self::FitPage,
        Self::FitWidth,
        Self::FitHeight,
        Self::SetDoubleLeftToRight,
        Self::SetDoubleRightToLeft,
        Self::ToggleDouble,
        Self::ZoomIn,
        Self::ZoomOut,
        Self::ZoomFineIn,
        Self::ZoomFineOut,
        Self::RotateClockwise,
        Self::RotateCounterClockwise,
        Self::Rotate0,
        Self::Rotate90,
        Self::Rotate180,
        Self::Rotate270,
        Self::FlipHorizontal,
        Self::FlipVertical,
        Self::ToggleInvert,
        Self::FilterNone,
        Self::FilterSmooth,
        Self::FilterSmoothSharpen,
        Self::ToggleGamma,
        Self::DeleteRecycle,
        Self::DeletePermanent,
        Self::OpenExplorer,
        Self::CopyPageImage,
        Self::CopyDisplayImage,
        Self::CopyPath,
        Self::UpscaleCurrentPage,
        Self::ToggleCurrentPageBookmark,
        Self::ToggleBookmarkPopover,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::OpenFile => "파일 열기",
            Self::OpenFolder => "폴더 열기",
            Self::CloseBook => "현재 책 닫기",
            Self::Quit => "프로그램 종료",
            Self::QuitFromEsc => "ESC로 종료",
            Self::ToggleFullscreen => "전체화면 전환",
            Self::ToggleMaximized => "최대화/복원",
            Self::Minimize => "최소화",
            Self::OpenSettings => "환경설정",
            Self::OpenAbout => "정보",
            Self::ToggleAlwaysOnTop => "항상 위 전환",
            Self::NextPage => "다음 페이지",
            Self::PreviousPage => "이전 페이지",
            Self::MoveForward10 => "10페이지 앞으로",
            Self::MoveBackward10 => "10페이지 뒤로",
            Self::MoveForward100 => "100페이지 앞으로",
            Self::MoveBackward100 => "100페이지 뒤로",
            Self::ForceNextPage => "다음 페이지 강제 이동",
            Self::ForcePreviousPage => "이전 페이지 강제 이동",
            Self::Home => "첫 페이지",
            Self::End => "마지막 페이지",
            Self::RandomForward => "임의 페이지 앞으로",
            Self::RandomBackward => "임의 페이지 뒤로",
            Self::NextBook => "다음 책",
            Self::PreviousBook => "이전 책",
            Self::FitOriginal => "원본 크기",
            Self::FitPage => "화면 맞춤",
            Self::FitWidth => "너비 맞춤",
            Self::FitHeight => "높이 맞춤",
            Self::SetDoubleLeftToRight => "2장 보기 L -> R",
            Self::SetDoubleRightToLeft => "2장 보기 R -> L",
            Self::ToggleDouble => "1장/2장 전환",
            Self::ZoomIn => "확대",
            Self::ZoomOut => "축소",
            Self::ZoomFineIn => "미세 확대",
            Self::ZoomFineOut => "미세 축소",
            Self::RotateClockwise => "오른쪽 회전",
            Self::RotateCounterClockwise => "왼쪽 회전",
            Self::Rotate0 => "회전 0도",
            Self::Rotate90 => "회전 90도",
            Self::Rotate180 => "회전 180도",
            Self::Rotate270 => "회전 270도",
            Self::FlipHorizontal => "좌우 반전",
            Self::FlipVertical => "상하 반전",
            Self::ToggleInvert => "색 반전",
            Self::FilterNone => "필터 없음",
            Self::FilterSmooth => "부드럽게",
            Self::FilterSmoothSharpen => "부드럽게+선명하게",
            Self::ToggleGamma => "감마 보정",
            Self::DeleteRecycle => "휴지통으로 삭제",
            Self::DeletePermanent => "완전 삭제",
            Self::OpenExplorer => "탐색기에서 보기",
            Self::CopyPageImage => "현재 페이지 복사",
            Self::CopyDisplayImage => "현재 표시 복사",
            Self::CopyPath => "현재 경로 복사",
            Self::UpscaleCurrentPage => "AI 업스케일",
            Self::ToggleCurrentPageBookmark => "현재 페이지 북마크",
            Self::ToggleBookmarkPopover => "북마크 열기",
        }
    }

    pub fn group(self) -> &'static str {
        match self {
            Self::OpenFile
            | Self::OpenFolder
            | Self::CloseBook
            | Self::Quit
            | Self::QuitFromEsc
            | Self::OpenSettings
            | Self::OpenAbout
            | Self::OpenExplorer
            | Self::CopyPath => "파일 / 앱",
            Self::ToggleFullscreen
            | Self::ToggleMaximized
            | Self::Minimize
            | Self::ToggleAlwaysOnTop => "창",
            Self::NextPage
            | Self::PreviousPage
            | Self::MoveForward10
            | Self::MoveBackward10
            | Self::MoveForward100
            | Self::MoveBackward100
            | Self::ForceNextPage
            | Self::ForcePreviousPage
            | Self::Home
            | Self::End
            | Self::RandomForward
            | Self::RandomBackward
            | Self::NextBook
            | Self::PreviousBook => "이동",
            Self::FitOriginal
            | Self::FitPage
            | Self::FitWidth
            | Self::FitHeight
            | Self::SetDoubleLeftToRight
            | Self::SetDoubleRightToLeft
            | Self::ToggleDouble
            | Self::ZoomIn
            | Self::ZoomOut
            | Self::ZoomFineIn
            | Self::ZoomFineOut => "보기",
            Self::RotateClockwise
            | Self::RotateCounterClockwise
            | Self::Rotate0
            | Self::Rotate90
            | Self::Rotate180
            | Self::Rotate270
            | Self::FlipHorizontal
            | Self::FlipVertical
            | Self::ToggleInvert
            | Self::FilterNone
            | Self::FilterSmooth
            | Self::FilterSmoothSharpen
            | Self::ToggleGamma
            | Self::UpscaleCurrentPage => "영상 처리",
            Self::DeleteRecycle
            | Self::DeletePermanent
            | Self::CopyPageImage
            | Self::CopyDisplayImage
            | Self::ToggleCurrentPageBookmark
            | Self::ToggleBookmarkPopover => "작업",
        }
    }

    pub fn id(self) -> u16 {
        self as u16
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyCode {
    F1,
    F2,
    F3,
    F4,
    F5,
    F11,
    A,
    B,
    C,
    E,
    F,
    G,
    I,
    K,
    L,
    M,
    N,
    O,
    P,
    Q,
    R,
    S,
    T,
    U,
    W,
    X,
    Z,
    Escape,
    Enter,
    Space,
    Backspace,
    Delete,
    Insert,
    Tab,
    PageDown,
    PageUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    ArrowUp,
    Home,
    End,
    OpenBracket,
    CloseBracket,
    Backtick,
    Slash,
    Plus,
    Equals,
    Minus,
    Num0,
    Num1,
    Num2,
    Num3,
    Num4,
    Num5,
    Num6,
    Num7,
    Num8,
    Num9,
    Asterisk,
}

impl KeyCode {
    pub fn label(self) -> &'static str {
        match self {
            Self::F1 => "F1",
            Self::F2 => "F2",
            Self::F3 => "F3",
            Self::F4 => "F4",
            Self::F5 => "F5",
            Self::F11 => "F11",
            Self::A => "A",
            Self::B => "B",
            Self::C => "C",
            Self::E => "E",
            Self::F => "F",
            Self::G => "G",
            Self::I => "I",
            Self::K => "K",
            Self::L => "L",
            Self::M => "M",
            Self::N => "N",
            Self::O => "O",
            Self::P => "P",
            Self::Q => "Q",
            Self::R => "R",
            Self::S => "S",
            Self::T => "T",
            Self::U => "U",
            Self::W => "W",
            Self::X => "X",
            Self::Z => "Z",
            Self::Escape => "Esc",
            Self::Enter => "Enter",
            Self::Space => "Space",
            Self::Backspace => "Backspace",
            Self::Delete => "Delete",
            Self::Insert => "Insert",
            Self::Tab => "Tab",
            Self::PageDown => "PageDown",
            Self::PageUp => "PageUp",
            Self::ArrowDown => "Down",
            Self::ArrowLeft => "Left",
            Self::ArrowRight => "Right",
            Self::ArrowUp => "Up",
            Self::Home => "Home",
            Self::End => "End",
            Self::OpenBracket => "[",
            Self::CloseBracket => "]",
            Self::Backtick => "`",
            Self::Slash => "/",
            Self::Plus => "+",
            Self::Equals => "=",
            Self::Minus => "-",
            Self::Num0 => "0",
            Self::Num1 => "1",
            Self::Num2 => "2",
            Self::Num3 => "3",
            Self::Num4 => "4",
            Self::Num5 => "5",
            Self::Num6 => "6",
            Self::Num7 => "7",
            Self::Num8 => "8",
            Self::Num9 => "9",
            Self::Asterisk => "*",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct KeyShortcut {
    pub key: KeyCode,
    #[serde(default)]
    pub ctrl: bool,
    #[serde(default)]
    pub alt: bool,
    #[serde(default)]
    pub shift: bool,
}

impl KeyShortcut {
    pub const fn new(key: KeyCode) -> Self {
        Self {
            key,
            ctrl: false,
            alt: false,
            shift: false,
        }
    }

    pub const fn ctrl(key: KeyCode) -> Self {
        Self {
            key,
            ctrl: true,
            alt: false,
            shift: false,
        }
    }

    pub const fn alt(key: KeyCode) -> Self {
        Self {
            key,
            ctrl: false,
            alt: true,
            shift: false,
        }
    }

    pub const fn shift(key: KeyCode) -> Self {
        Self {
            key,
            ctrl: false,
            alt: false,
            shift: true,
        }
    }

    pub const fn ctrl_alt(key: KeyCode) -> Self {
        Self {
            key,
            ctrl: true,
            alt: true,
            shift: false,
        }
    }

    pub const fn ctrl_shift(key: KeyCode) -> Self {
        Self {
            key,
            ctrl: true,
            alt: false,
            shift: true,
        }
    }

    pub const fn ctrl_alt_shift(key: KeyCode) -> Self {
        Self {
            key,
            ctrl: true,
            alt: true,
            shift: true,
        }
    }

    pub fn label(self) -> String {
        let mut parts = Vec::new();
        if self.ctrl {
            parts.push("Ctrl");
        }
        if self.alt {
            parts.push("Alt");
        }
        if self.shift {
            parts.push("Shift");
        }
        parts.push(self.key.label());
        parts.join("+")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyBinding {
    pub command: CommandId,
    pub shortcut: KeyShortcut,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseGesture {
    DoubleClick,
    MiddleClick,
    CtrlMiddleClick,
    WheelUp,
    WheelDown,
    CtrlWheelUp,
    CtrlWheelDown,
}

impl MouseGesture {
    pub const ALL: [Self; 7] = [
        Self::DoubleClick,
        Self::MiddleClick,
        Self::CtrlMiddleClick,
        Self::WheelUp,
        Self::WheelDown,
        Self::CtrlWheelUp,
        Self::CtrlWheelDown,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::DoubleClick => "더블클릭",
            Self::MiddleClick => "가운데 버튼",
            Self::CtrlMiddleClick => "Ctrl+가운데 버튼",
            Self::WheelUp => "휠 위",
            Self::WheelDown => "휠 아래",
            Self::CtrlWheelUp => "Ctrl+휠 위",
            Self::CtrlWheelDown => "Ctrl+휠 아래",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MouseBinding {
    pub gesture: MouseGesture,
    pub command: CommandId,
}

pub fn default_key_bindings() -> Vec<KeyBinding> {
    use CommandId as C;
    use KeyCode as K;
    let pairs: &[(C, KeyShortcut)] = &[
        (C::OpenFile, KeyShortcut::new(K::F2)),
        (C::OpenFile, KeyShortcut::ctrl(K::O)),
        (C::OpenFolder, KeyShortcut::new(K::F)),
        (C::CloseBook, KeyShortcut::new(K::F4)),
        (C::QuitFromEsc, KeyShortcut::new(K::Escape)),
        (C::Quit, KeyShortcut::new(K::X)),
        (C::Quit, KeyShortcut::ctrl(K::W)),
        (C::ToggleFullscreen, KeyShortcut::new(K::F11)),
        (C::ToggleFullscreen, KeyShortcut::alt(K::Enter)),
        (C::ToggleFullscreen, KeyShortcut::new(K::N)),
        (C::ToggleMaximized, KeyShortcut::new(K::M)),
        (C::Minimize, KeyShortcut::new(K::Q)),
        (C::OpenExplorer, KeyShortcut::ctrl(K::Enter)),
        (C::RandomForward, KeyShortcut::ctrl_alt(K::PageDown)),
        (C::RandomBackward, KeyShortcut::ctrl_alt(K::PageUp)),
        (C::MoveForward100, KeyShortcut::ctrl_shift(K::ArrowRight)),
        (C::MoveBackward100, KeyShortcut::ctrl_shift(K::ArrowLeft)),
        (C::MoveForward10, KeyShortcut::ctrl(K::PageDown)),
        (C::MoveBackward10, KeyShortcut::ctrl(K::PageUp)),
        (C::ForceNextPage, KeyShortcut::shift(K::PageDown)),
        (C::ForcePreviousPage, KeyShortcut::shift(K::PageUp)),
        (C::NextPage, KeyShortcut::new(K::PageDown)),
        (C::NextPage, KeyShortcut::new(K::ArrowDown)),
        (C::NextPage, KeyShortcut::new(K::ArrowRight)),
        (C::NextPage, KeyShortcut::new(K::Space)),
        (C::PreviousPage, KeyShortcut::shift(K::Space)),
        (C::PreviousPage, KeyShortcut::new(K::PageUp)),
        (C::PreviousPage, KeyShortcut::new(K::ArrowUp)),
        (C::PreviousPage, KeyShortcut::new(K::ArrowLeft)),
        (C::PreviousPage, KeyShortcut::new(K::Backspace)),
        (C::Home, KeyShortcut::new(K::Home)),
        (C::End, KeyShortcut::new(K::End)),
        (C::NextBook, KeyShortcut::new(K::CloseBracket)),
        (C::PreviousBook, KeyShortcut::new(K::OpenBracket)),
        (C::FitOriginal, KeyShortcut::new(K::Num0)),
        (C::FitOriginal, KeyShortcut::new(K::Asterisk)),
        (C::FitPage, KeyShortcut::new(K::Num1)),
        (C::FitPage, KeyShortcut::new(K::Num9)),
        (C::FitPage, KeyShortcut::new(K::Z)),
        (C::FitWidth, KeyShortcut::new(K::Num8)),
        (C::SetDoubleLeftToRight, KeyShortcut::new(K::Num7)),
        (C::SetDoubleRightToLeft, KeyShortcut::new(K::Num6)),
        (C::ToggleDouble, KeyShortcut::new(K::Num2)),
        (C::ZoomFineIn, KeyShortcut::ctrl(K::Plus)),
        (C::ZoomFineIn, KeyShortcut::ctrl(K::Equals)),
        (C::ZoomFineOut, KeyShortcut::ctrl(K::Minus)),
        (C::ZoomIn, KeyShortcut::new(K::Plus)),
        (C::ZoomIn, KeyShortcut::new(K::Equals)),
        (C::ZoomIn, KeyShortcut::shift(K::Plus)),
        (C::ZoomIn, KeyShortcut::shift(K::Equals)),
        (C::ZoomOut, KeyShortcut::new(K::Minus)),
        (C::ToggleInvert, KeyShortcut::ctrl(K::I)),
        (C::FlipHorizontal, KeyShortcut::ctrl(K::M)),
        (C::FlipVertical, KeyShortcut::ctrl(K::F)),
        (C::RotateCounterClockwise, KeyShortcut::ctrl(K::L)),
        (C::RotateClockwise, KeyShortcut::ctrl(K::R)),
        (C::Rotate0, KeyShortcut::alt(K::ArrowUp)),
        (C::Rotate270, KeyShortcut::alt(K::ArrowLeft)),
        (C::Rotate90, KeyShortcut::alt(K::ArrowRight)),
        (C::Rotate180, KeyShortcut::alt(K::ArrowDown)),
        (C::FilterNone, KeyShortcut::new(K::U)),
        (C::FilterSmooth, KeyShortcut::new(K::I)),
        (C::FilterSmoothSharpen, KeyShortcut::new(K::S)),
        (C::ToggleGamma, KeyShortcut::ctrl(K::G)),
        (C::DeletePermanent, KeyShortcut::shift(K::Delete)),
        (C::DeleteRecycle, KeyShortcut::new(K::Delete)),
        (C::CopyPath, KeyShortcut::ctrl_alt_shift(K::C)),
        (C::CopyDisplayImage, KeyShortcut::ctrl_alt(K::C)),
        (C::CopyPageImage, KeyShortcut::ctrl(K::C)),
        (C::OpenSettings, KeyShortcut::new(K::F5)),
        (C::OpenAbout, KeyShortcut::new(K::F1)),
        (C::ToggleCurrentPageBookmark, KeyShortcut::new(K::B)),
        (C::ToggleBookmarkPopover, KeyShortcut::ctrl(K::B)),
        (C::ToggleAlwaysOnTop, KeyShortcut::ctrl(K::A)),
    ];
    pairs
        .iter()
        .map(|(command, shortcut)| KeyBinding {
            command: *command,
            shortcut: *shortcut,
        })
        .collect()
}

pub fn default_mouse_bindings() -> Vec<MouseBinding> {
    use CommandId as C;
    use MouseGesture as M;
    [
        (M::DoubleClick, C::ToggleMaximized),
        (M::MiddleClick, C::ToggleFullscreen),
        (M::CtrlMiddleClick, C::FitOriginal),
        (M::WheelUp, C::PreviousPage),
        (M::WheelDown, C::NextPage),
        (M::CtrlWheelUp, C::ZoomIn),
        (M::CtrlWheelDown, C::ZoomOut),
    ]
    .into_iter()
    .map(|(gesture, command)| MouseBinding { gesture, command })
    .collect()
}
