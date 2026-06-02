use crate::core::i18n::I18n;
use crate::core::perf_trace::{self, PerfField};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

mod bookmarks;
mod decoders;
mod display;
mod input;
mod rendering;
#[cfg(test)]
mod tests;
pub use crate::core::i18n::Language;
use bookmarks::path_key;
pub use bookmarks::{Bookmark, BookmarkInput, PageBookmark, PageBookmarkEntry, ReadingPosition};
pub use decoders::{DecodeMode, DecoderPreference, DecoderPreferences};
pub use display::{DisplayUpscaler, GpuEffectMode, ResizeFilter};
pub use input::{
    default_key_bindings, default_mouse_bindings, CommandId, KeyBinding, KeyCode, KeyShortcut,
    MouseBinding, MouseGesture,
};
pub use rendering::RendererMode;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReadingDirection {
    LeftToRight,
    #[default]
    RightToLeft,
}

impl ReadingDirection {
    pub fn label(self) -> &'static str {
        match self {
            Self::LeftToRight => "L -> R",
            Self::RightToLeft => "R -> L",
        }
    }

    pub fn label_i18n(self, i18n: I18n) -> String {
        match self {
            Self::LeftToRight => i18n.text("label.reading.ltr"),
            Self::RightToLeft => i18n.text("label.reading.rtl"),
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FitMode {
    #[default]
    FitPage,
    FitWidth,
    FitHeight,
    Original,
    Manual,
}

impl FitMode {
    pub const ALL: [Self; 5] = [
        Self::FitPage,
        Self::FitWidth,
        Self::FitHeight,
        Self::Original,
        Self::Manual,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::FitPage => "Fit",
            Self::FitWidth => "Width",
            Self::FitHeight => "Height",
            Self::Original => "1:1",
            Self::Manual => "Zoom",
        }
    }

    pub fn label_i18n(self, i18n: I18n) -> String {
        match self {
            Self::FitPage => i18n.text("label.fit.page"),
            Self::FitWidth => i18n.text("label.fit.width"),
            Self::FitHeight => i18n.text("label.fit.height"),
            Self::Original => i18n.text("label.fit.original"),
            Self::Manual => i18n.text("label.fit.manual"),
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EdgePageAction {
    #[default]
    Stop,
    Ask,
    Wrap,
    NextBook,
}

impl EdgePageAction {
    pub const ALL: [Self; 4] = [Self::Stop, Self::Ask, Self::Wrap, Self::NextBook];

    pub fn label(self) -> &'static str {
        match self {
            Self::Stop => "아무것도 하지 않음",
            Self::Ask => "무엇을 할지 물어보기",
            Self::Wrap => "다시 처음으로 돌아가기",
            Self::NextBook => "다음/이전 폴더/파일로 넘어가기",
        }
    }

    pub fn label_i18n(self, i18n: I18n) -> String {
        match self {
            Self::Stop => i18n.text("label.edge.stop"),
            Self::Ask => i18n.text("label.edge.ask"),
            Self::Wrap => i18n.text("label.edge.wrap"),
            Self::NextBook => i18n.text("label.edge.next_book"),
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CacheMemoryMode {
    #[default]
    Auto,
    Manual,
}

impl CacheMemoryMode {
    pub fn label_i18n(self, i18n: I18n) -> String {
        match self {
            Self::Auto => i18n.text("state.auto"),
            Self::Manual => i18n.text("state.manual"),
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LargeImageAnchor {
    #[default]
    Center,
    Top,
    TopLeft,
}

impl LargeImageAnchor {
    pub const ALL: [Self; 3] = [Self::Center, Self::Top, Self::TopLeft];

    pub fn label(self) -> &'static str {
        match self {
            Self::Center => "Center",
            Self::Top => "Top",
            Self::TopLeft => "Top left",
        }
    }

    pub fn label_i18n(self, i18n: I18n) -> String {
        match self {
            Self::Center => i18n.text("label.anchor.center"),
            Self::Top => i18n.text("label.anchor.top"),
            Self::TopLeft => i18n.text("label.anchor.top_left"),
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WheelMode {
    #[default]
    PageTurn,
    ScrollWhenZoomed,
}

impl WheelMode {
    pub const ALL: [Self; 2] = [Self::PageTurn, Self::ScrollWhenZoomed];

    pub fn label(self) -> &'static str {
        match self {
            Self::PageTurn => "Page turn",
            Self::ScrollWhenZoomed => "Scroll when zoomed",
        }
    }

    pub fn label_i18n(self, i18n: I18n) -> String {
        match self {
            Self::PageTurn => i18n.text("label.wheel.page_turn"),
            Self::ScrollWhenZoomed => i18n.text("label.wheel.scroll_when_zoomed"),
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AiUpscaleBackend {
    #[default]
    Off,
    RealEsrganNcnn,
}

impl AiUpscaleBackend {
    pub const ALL: [Self; 2] = [Self::Off, Self::RealEsrganNcnn];

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "사용 안 함",
            Self::RealEsrganNcnn => "Real-ESRGAN ncnn-vulkan",
        }
    }

    pub fn label_i18n(self, i18n: I18n) -> String {
        match self {
            Self::Off => i18n.text("state.off"),
            Self::RealEsrganNcnn => self.label().to_owned(),
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AiUpscalePrefetchMode {
    #[default]
    Off,
    CurrentOnly,
    CurrentAndNext,
}

impl AiUpscalePrefetchMode {
    pub const ALL: [Self; 3] = [Self::Off, Self::CurrentOnly, Self::CurrentAndNext];

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "사용 안 함",
            Self::CurrentOnly => "현재 페이지만",
            Self::CurrentAndNext => "현재+다음 1장",
        }
    }

    pub fn label_i18n(self, i18n: I18n) -> String {
        match self {
            Self::Off => i18n.text("state.off"),
            Self::CurrentOnly => i18n.text("state.current_only"),
            Self::CurrentAndNext => i18n.text("state.current_and_next"),
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PageTransitionStyle {
    None,
    #[default]
    SlideFade,
    Fade,
    Push,
    ZoomFade,
    BookFlip2d,
}

impl PageTransitionStyle {
    pub const ALL: [Self; 6] = [
        Self::None,
        Self::SlideFade,
        Self::Fade,
        Self::Push,
        Self::ZoomFade,
        Self::BookFlip2d,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::None => "없음",
            Self::SlideFade => "슬라이드 + 페이드",
            Self::Fade => "페이드",
            Self::Push => "밀기",
            Self::ZoomFade => "줌 페이드",
            Self::BookFlip2d => "책장 넘김",
        }
    }

    pub fn label_i18n(self, i18n: I18n) -> String {
        match self {
            Self::None => i18n.text("label.transition.none"),
            Self::SlideFade => i18n.text("label.transition.slide_fade"),
            Self::Fade => i18n.text("label.transition.fade"),
            Self::Push => i18n.text("label.transition.push"),
            Self::ZoomFade => i18n.text("label.transition.zoom_fade"),
            Self::BookFlip2d => i18n.text("label.transition.book_flip"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NcnnRealEsrganSettings {
    #[serde(default)]
    pub executable_path: String,
    #[serde(default = "default_realesrgan_model_name")]
    pub model_name: String,
    #[serde(default)]
    pub model_path: String,
    #[serde(default = "default_realesrgan_scale")]
    pub scale: u32,
    #[serde(default)]
    pub tile_size: u32,
    #[serde(default = "default_realesrgan_output_format")]
    pub output_format: String,
}

impl Default for NcnnRealEsrganSettings {
    fn default() -> Self {
        Self {
            executable_path: String::new(),
            model_name: default_realesrgan_model_name(),
            model_path: String::new(),
            scale: default_realesrgan_scale(),
            tile_size: 0,
            output_format: default_realesrgan_output_format(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiUpscaleSettings {
    #[serde(default)]
    pub backend: AiUpscaleBackend,
    #[serde(default)]
    pub prefetch_mode: AiUpscalePrefetchMode,
    #[serde(default)]
    pub ncnn: NcnnRealEsrganSettings,
}

impl Default for AiUpscaleSettings {
    fn default() -> Self {
        Self {
            backend: AiUpscaleBackend::Off,
            prefetch_mode: AiUpscalePrefetchMode::Off,
            ncnn: NcnnRealEsrganSettings::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppSettings {
    #[serde(default)]
    pub language: Language,
    #[serde(default = "default_true")]
    pub confirm_delete: bool,
    #[serde(default = "default_true")]
    pub esc_to_quit: bool,
    #[serde(default)]
    pub always_on_top: bool,
    #[serde(default = "default_true")]
    pub show_toasts: bool,
    #[serde(default = "default_true")]
    pub remember_recent_locations: bool,
    #[serde(default)]
    pub single_instance: bool,
    #[serde(default)]
    pub show_status_bar: bool,
    #[serde(default = "default_true")]
    pub top_bar_pinned: bool,
    #[serde(default)]
    pub show_filename_overlay: bool,
    #[serde(default = "default_true")]
    pub show_main_border: bool,
    #[serde(default = "default_true")]
    pub show_page_arrows: bool,
    #[serde(default)]
    pub edge_page_action: EdgePageAction,
    #[serde(default = "default_image_edge_page_action")]
    pub image_edge_page_action: EdgePageAction,
    #[serde(default = "default_archive_edge_page_action")]
    pub archive_edge_page_action: EdgePageAction,

    #[serde(default)]
    pub decode_mode: DecodeMode,
    #[serde(default)]
    pub decoder_preferences: DecoderPreferences,
    #[serde(default)]
    pub resize_filter: ResizeFilter,
    #[serde(default)]
    pub gpu_effect_mode: GpuEffectMode,
    #[serde(default)]
    pub renderer_mode: RendererMode,
    #[serde(default)]
    pub display_upscaler: DisplayUpscaler,
    #[serde(default = "default_true")]
    pub prefetch_enabled: bool,
    #[serde(default)]
    pub progressive_preview_enabled: bool,
    #[serde(default)]
    pub cache_memory_mode: CacheMemoryMode,
    #[serde(default = "default_manual_cache_mb")]
    pub manual_cache_mb: u32,

    #[serde(default)]
    pub transition_effect: bool,
    #[serde(default)]
    pub page_transition_style: PageTransitionStyle,
    #[serde(default)]
    pub large_image_anchor: LargeImageAnchor,
    #[serde(default)]
    pub remember_zoom_per_book: bool,
    #[serde(default = "default_true")]
    pub double_click_maximize: bool,
    #[serde(default = "default_true")]
    pub middle_click_fullscreen: bool,
    #[serde(default)]
    pub wheel_mode: WheelMode,

    #[serde(default = "default_true")]
    pub apply_exif_orientation: bool,
    #[serde(default)]
    pub apply_embedded_icc: bool,

    #[serde(default = "default_true")]
    pub auto_save_reading_position: bool,
    #[serde(default = "default_true")]
    pub resume_by_file_identity: bool,
    #[serde(default = "default_true")]
    pub share_state_between_instances: bool,
    #[serde(default = "default_max_remembered_books")]
    pub max_remembered_books: usize,
    #[serde(default = "default_true")]
    pub remember_archive_page_name: bool,
    #[serde(default = "default_key_bindings")]
    pub key_bindings: Vec<KeyBinding>,
    #[serde(default = "default_mouse_bindings")]
    pub mouse_bindings: Vec<MouseBinding>,

    #[serde(default)]
    pub ai_upscale: AiUpscaleSettings,
}

impl AppSettings {
    pub fn normalize_product_choices(&mut self) {
        if !self.display_upscaler.user_selectable() {
            self.display_upscaler = DisplayUpscaler::Auto;
        }
    }

    pub fn effective_page_transition_style(&self) -> PageTransitionStyle {
        if self.transition_effect {
            self.page_transition_style
        } else {
            PageTransitionStyle::None
        }
    }

    pub fn set_page_transition_style(&mut self, style: PageTransitionStyle) {
        self.transition_effect = style != PageTransitionStyle::None;
        if self.transition_effect {
            self.page_transition_style = style;
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct WindowPlacement {
    #[serde(default)]
    pub inner_size: Option<[f32; 2]>,
    #[serde(default)]
    pub outer_position: Option<[f32; 2]>,
    #[serde(default)]
    pub maximized: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            language: Language::Auto,
            confirm_delete: true,
            esc_to_quit: true,
            always_on_top: false,
            show_toasts: true,
            remember_recent_locations: true,
            single_instance: false,
            show_status_bar: false,
            top_bar_pinned: true,
            show_filename_overlay: false,
            show_main_border: true,
            show_page_arrows: true,
            edge_page_action: EdgePageAction::Stop,
            image_edge_page_action: EdgePageAction::Wrap,
            archive_edge_page_action: default_archive_edge_page_action(),
            decode_mode: DecodeMode::AutoFast,
            decoder_preferences: DecoderPreferences::default(),
            resize_filter: ResizeFilter::Bicubic,
            gpu_effect_mode: GpuEffectMode::Auto,
            renderer_mode: RendererMode::LowMemoryGlow,
            display_upscaler: DisplayUpscaler::None,
            prefetch_enabled: true,
            progressive_preview_enabled: false,
            cache_memory_mode: CacheMemoryMode::Auto,
            manual_cache_mb: default_manual_cache_mb(),
            transition_effect: false,
            page_transition_style: PageTransitionStyle::SlideFade,
            large_image_anchor: LargeImageAnchor::Center,
            remember_zoom_per_book: false,
            double_click_maximize: true,
            middle_click_fullscreen: true,
            wheel_mode: WheelMode::PageTurn,
            apply_exif_orientation: true,
            apply_embedded_icc: false,
            auto_save_reading_position: true,
            resume_by_file_identity: true,
            share_state_between_instances: true,
            max_remembered_books: default_max_remembered_books(),
            remember_archive_page_name: true,
            key_bindings: default_key_bindings(),
            mouse_bindings: default_mouse_bindings(),
            ai_upscale: AiUpscaleSettings::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedState {
    pub version: u32,
    #[serde(default)]
    pub settings: AppSettings,
    #[serde(default)]
    pub window: WindowPlacement,
    pub books: BTreeMap<String, Bookmark>,
}

impl Default for PersistedState {
    fn default() -> Self {
        Self {
            version: 4,
            settings: AppSettings::default(),
            window: WindowPlacement::default(),
            books: BTreeMap::new(),
        }
    }
}

#[derive(Clone)]
pub struct StateStore {
    path: PathBuf,
    state: PersistedState,
}

impl StateStore {
    pub fn load() -> Self {
        let path = state_file_path();
        let mut state = fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<PersistedState>(&text).ok())
            .unwrap_or_default();
        state.settings.normalize_product_choices();

        Self { path, state }
    }

    pub fn bookmark(&self, book_id: &str) -> Option<&Bookmark> {
        self.state.books.get(book_id)
    }

    pub fn reading_position(
        &self,
        book_id: &str,
        path: &Path,
        allow_identity_match: bool,
    ) -> Option<ReadingPosition> {
        let bookmark = self.bookmark(book_id)?;
        if allow_identity_match {
            return Some(ReadingPosition::from_bookmark(bookmark));
        }
        bookmark
            .path_positions
            .get(path_key(path).as_str())
            .cloned()
    }

    pub fn settings(&self) -> &AppSettings {
        &self.state.settings
    }

    pub fn update_settings(&mut self, mut settings: AppSettings) {
        settings.normalize_product_choices();
        self.state.settings = settings;
        self.state.version = 4;
        let _ = self.save();
    }

    pub fn reload_from_disk(&mut self) -> bool {
        let Ok(text) = fs::read_to_string(&self.path) else {
            return false;
        };
        let Ok(state) = serde_json::from_str::<PersistedState>(&text) else {
            return false;
        };
        let mut state = state;
        state.settings.normalize_product_choices();
        self.state = state;
        true
    }

    pub fn reload_books_from_disk(&mut self) -> bool {
        let Ok(text) = fs::read_to_string(&self.path) else {
            return false;
        };
        let Ok(state) = serde_json::from_str::<PersistedState>(&text) else {
            return false;
        };
        self.state.books = state.books;
        true
    }

    pub fn window_placement(&self) -> &WindowPlacement {
        &self.state.window
    }

    pub fn update_window_placement_deferred(&mut self, placement: WindowPlacement) -> bool {
        if self.state.window == placement {
            return false;
        }
        self.state.window = placement;
        self.state.version = 4;
        true
    }

    pub fn upsert_bookmark(&mut self, input: BookmarkInput<'_>) {
        if self.update_bookmark(input, true) {
            let _ = self.save();
        }
    }

    pub fn upsert_bookmark_deferred(&mut self, input: BookmarkInput<'_>) -> bool {
        self.update_bookmark(input, false)
    }

    pub fn prune_auto_bookmarks(&mut self, max_books: usize) -> usize {
        if max_books == 0 {
            return 0;
        }

        let mut removable: Vec<_> = self
            .state
            .books
            .values()
            .filter(|book| {
                book.page_bookmarks
                    .iter()
                    .all(|bookmark| bookmark.source_path.is_empty())
            })
            .map(|book| (book.book_id.clone(), book.updated_at))
            .collect();
        let keep_non_removable = self.state.books.len().saturating_sub(removable.len());
        let removable_to_keep = max_books.saturating_sub(keep_non_removable);
        if removable.len() <= removable_to_keep {
            return 0;
        }

        removable.sort_by_key(|(_, updated_at)| *updated_at);
        let remove_count = removable.len() - removable_to_keep;
        for (book_id, _) in removable.into_iter().take(remove_count) {
            self.state.books.remove(&book_id);
        }
        self.state.version = 4;
        let _ = self.save();
        remove_count
    }

    pub fn clear_archive_page_names(&mut self) -> usize {
        let mut cleared = 0;
        for bookmark in self.state.books.values_mut() {
            if !looks_like_archive_book(bookmark) {
                continue;
            }
            let mut bookmark_cleared = false;
            if bookmark.last_page_name.take().is_some() {
                bookmark_cleared = true;
                cleared += 1;
            }
            for position in bookmark.path_positions.values_mut() {
                if position.last_page_name.take().is_some() {
                    bookmark_cleared = true;
                    cleared += 1;
                }
            }
            if bookmark_cleared {
                bookmark.updated_at = now_unix_seconds();
            }
        }
        if cleared > 0 {
            self.state.version = 4;
            let _ = self.save();
        }
        cleared
    }

    fn update_bookmark(&mut self, input: BookmarkInput<'_>, touch: bool) -> bool {
        self.state.version = 4;
        let path_text = input.path.to_string_lossy().to_string();
        let now = now_unix_seconds();
        let is_new = !self.state.books.contains_key(input.book_id);
        let entry = self
            .state
            .books
            .entry(input.book_id.to_owned())
            .or_insert_with(|| Bookmark {
                book_id: input.book_id.to_owned(),
                title: input.title.to_owned(),
                last_page: 0,
                last_page_name: None,
                total_pages: input.total_pages,
                known_paths: Vec::new(),
                reading_direction: input.reading_direction,
                fit_mode: input.fit_mode,
                manual_zoom: None,
                path_positions: BTreeMap::new(),
                page_bookmarks: Vec::new(),
                updated_at: now,
            });

        let title = input.title.to_owned();
        let last_page = input.last_page.min(input.total_pages.saturating_sub(1));
        let last_page_name = input.last_page_name.map(ToOwned::to_owned);
        let path_position_changed = entry
            .path_positions
            .get(path_text.as_str())
            .is_none_or(|position| !position.matches_input(&input));
        let mut changed = is_new
            || entry.title != title
            || entry.last_page != last_page
            || entry.last_page_name != last_page_name
            || entry.total_pages != input.total_pages
            || entry.reading_direction != input.reading_direction
            || entry.fit_mode != input.fit_mode
            || entry.manual_zoom != input.manual_zoom
            || path_position_changed;

        entry.title = title;
        entry.last_page = last_page;
        entry.last_page_name = last_page_name;
        entry.total_pages = input.total_pages;
        entry.reading_direction = input.reading_direction;
        entry.fit_mode = input.fit_mode;
        entry.manual_zoom = input.manual_zoom;
        if path_position_changed || touch {
            entry
                .path_positions
                .insert(path_text.clone(), ReadingPosition::from_input(&input, now));
        }

        if !entry.known_paths.iter().any(|known| known == &path_text) {
            entry.known_paths.push(path_text);
            changed = true;
        }
        if entry.known_paths.len() > 8 {
            let extra = entry.known_paths.len() - 8;
            entry.known_paths.drain(0..extra);
            changed = true;
        }

        if changed || touch {
            entry.updated_at = now;
        }
        changed || touch
    }

    pub fn save(&self) -> std::io::Result<()> {
        let started = Instant::now();
        let result = (|| {
            if let Some(parent) = self.path.parent() {
                fs::create_dir_all(parent)?;
            }
            let text = serde_json::to_string_pretty(&self.state)?;
            fs::write(&self.path, text)
        })();
        perf_trace::record_duration_if_at_least(
            "state_save",
            started.elapsed(),
            Duration::from_millis(20),
            &[PerfField::Bool("success", result.is_ok())],
        );
        result
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

fn looks_like_archive_book(bookmark: &Bookmark) -> bool {
    bookmark.known_paths.iter().any(|path| {
        Path::new(path)
            .extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| matches!(extension.to_ascii_lowercase().as_str(), "zip" | "cbz"))
            .unwrap_or(false)
    })
}

fn state_file_path() -> PathBuf {
    ProjectDirs::from("", "", "SuiSuiView")
        .map(|dirs| dirs.data_dir().join("state.json"))
        .unwrap_or_else(|| PathBuf::from("SuiSuiView-state.json"))
}

fn now_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

fn default_true() -> bool {
    true
}

fn default_image_edge_page_action() -> EdgePageAction {
    EdgePageAction::Wrap
}

fn default_archive_edge_page_action() -> EdgePageAction {
    EdgePageAction::Ask
}

fn default_manual_cache_mb() -> u32 {
    160
}

fn default_max_remembered_books() -> usize {
    30
}

fn default_realesrgan_model_name() -> String {
    "realesrgan-x4plus-anime".to_owned()
}

fn default_realesrgan_scale() -> u32 {
    4
}

fn default_realesrgan_output_format() -> String {
    "png".to_owned()
}
