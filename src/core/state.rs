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
mod fast_start;
mod input;
mod rendering;
mod scalers;
#[cfg(test)]
mod tests;
pub use crate::core::i18n::Language;
use bookmarks::path_key;
pub use bookmarks::{BookRecord, BookRecordInput, PageBookmark, PageBookmarkEntry, ReadingPosition};
pub use decoders::{DecodeMode, DecoderPreference, DecoderPreferences};
pub use display::{GpuEffectMode, WgpuUpscaleMethod};
pub use fast_start::FastStartFailureNotice;
pub use input::{
    default_key_bindings, default_mouse_bindings, CommandId, KeyBinding, KeyCode, KeyShortcut,
    MouseBinding, MouseGesture,
};
pub use rendering::RendererMode;
pub use scalers::{
    CpuScaleFilter, ResizeFilter, WgpuDownscaleMethod, WgpuScaleDirection, WgpuScalePlan,
};

pub const DEFAULT_TOP_BAR_CPU_SCALE_FILTERS: [CpuScaleFilter; 5] = [
    CpuScaleFilter::Nearest,
    CpuScaleFilter::Bilinear,
    CpuScaleFilter::Hamming,
    CpuScaleFilter::CatmullRom,
    CpuScaleFilter::Lanczos3,
];
pub const DEFAULT_TOP_BAR_WGPU_UPSCALE_METHODS: [WgpuUpscaleMethod; 4] = [
    WgpuUpscaleMethod::Auto,
    WgpuUpscaleMethod::WgslBilinear,
    WgpuUpscaleMethod::WgslFsr1EasuRcas,
    WgpuUpscaleMethod::WgslAnime4kV32CnnX2M,
];
pub const DEFAULT_TOP_BAR_WGPU_DOWNSCALE_METHODS: [WgpuDownscaleMethod; 6] = [
    WgpuDownscaleMethod::Bilinear,
    WgpuDownscaleMethod::Hamming,
    WgpuDownscaleMethod::Lanczos3,
    WgpuDownscaleMethod::CatmullRom,
    WgpuDownscaleMethod::PyramidHamming,
    WgpuDownscaleMethod::PyramidLanczos3,
];

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopBarItems {
    #[serde(default = "default_true")]
    pub open: bool,
    #[serde(default = "default_true")]
    pub page: bool,
    #[serde(default = "default_true")]
    pub view: bool,
    #[serde(default = "default_true")]
    pub adjust: bool,
    #[serde(default = "default_true")]
    pub compare: bool,
    #[serde(default = "default_true")]
    pub bookmarks: bool,
}

impl Default for TopBarItems {
    fn default() -> Self {
        Self {
            open: true,
            page: true,
            view: true,
            adjust: true,
            compare: true,
            bookmarks: true,
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CacheMemoryMode {
    #[default]
    Auto,
    Saver,
    Standard,
    Ample,
    Manual,
}

pub const MANUAL_CACHE_MB_MIN: u32 = 64;
pub const MANUAL_CACHE_MB_MAX: u32 = 2048;
pub const DEFAULT_MANUAL_CACHE_MB: u32 = 160;

/// Fixed total-memory budgets (bytes) for the preset modes. These caps dominate every cache
/// pool; the current-page working set is always exempt so display never breaks.
pub const SAVER_TOTAL_BUDGET_BYTES: usize = 128 * 1024 * 1024;
pub const STANDARD_TOTAL_BUDGET_BYTES: usize = 256 * 1024 * 1024;
pub const AMPLE_TOTAL_BUDGET_BYTES: usize = 768 * 1024 * 1024;

impl CacheMemoryMode {
    pub fn label_i18n(self, i18n: I18n) -> String {
        match self {
            Self::Auto => i18n.text("state.auto"),
            Self::Saver => i18n.text("state.cache.saver"),
            Self::Standard => i18n.text("state.cache.standard"),
            Self::Ample => i18n.text("state.cache.ample"),
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
    pub top_bar_items: TopBarItems,
    #[serde(default = "default_top_bar_cpu_scale_filters")]
    pub top_bar_cpu_scale_filters: Vec<CpuScaleFilter>,
    #[serde(default = "default_top_bar_wgpu_upscale_methods")]
    pub top_bar_wgpu_upscale_methods: Vec<WgpuUpscaleMethod>,
    #[serde(default = "default_top_bar_wgpu_downscale_methods")]
    pub top_bar_wgpu_downscale_methods: Vec<WgpuDownscaleMethod>,
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
    #[serde(default = "default_true")]
    pub fast_sampled_scaled_decode: bool,
    #[serde(default = "default_cpu_upscale_filter")]
    pub cpu_upscale_filter: CpuScaleFilter,
    #[serde(default = "default_cpu_downscale_filter")]
    pub cpu_downscale_filter: CpuScaleFilter,
    #[serde(default)]
    pub gpu_effect_mode: GpuEffectMode,
    #[serde(default)]
    pub renderer_mode: RendererMode,
    #[serde(default)]
    pub wgpu_upscale_method: WgpuUpscaleMethod,
    #[serde(default = "default_wgpu_downscale_method")]
    pub wgpu_downscale_method: WgpuDownscaleMethod,
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
    pub remember_archive_page_name: bool,
    #[serde(default = "default_key_bindings")]
    pub key_bindings: Vec<KeyBinding>,
    #[serde(default = "default_mouse_bindings")]
    pub mouse_bindings: Vec<MouseBinding>,
}

impl AppSettings {
    pub fn normalize_product_choices(&mut self) {
        if !self.wgpu_upscale_method.user_selectable() {
            self.wgpu_upscale_method = WgpuUpscaleMethod::Auto;
        }

        self.wgpu_downscale_method = self.wgpu_downscale_method.selectable_fallback();

        let mut seen = Vec::new();
        self.top_bar_wgpu_downscale_methods.retain_mut(|method| {
            *method = method.selectable_fallback();
            if seen.contains(method) {
                false
            } else {
                seen.push(*method);
                true
            }
        });
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
            top_bar_items: TopBarItems::default(),
            top_bar_cpu_scale_filters: default_top_bar_cpu_scale_filters(),
            top_bar_wgpu_upscale_methods: default_top_bar_wgpu_upscale_methods(),
            top_bar_wgpu_downscale_methods: default_top_bar_wgpu_downscale_methods(),
            show_filename_overlay: false,
            show_main_border: true,
            show_page_arrows: true,
            edge_page_action: EdgePageAction::Stop,
            image_edge_page_action: EdgePageAction::Wrap,
            archive_edge_page_action: default_archive_edge_page_action(),
            decode_mode: DecodeMode::AutoFast,
            decoder_preferences: DecoderPreferences::default(),
            fast_sampled_scaled_decode: true,
            cpu_upscale_filter: default_cpu_upscale_filter(),
            cpu_downscale_filter: default_cpu_downscale_filter(),
            gpu_effect_mode: GpuEffectMode::Auto,
            renderer_mode: RendererMode::LowMemoryGlow,
            wgpu_upscale_method: WgpuUpscaleMethod::None,
            wgpu_downscale_method: default_wgpu_downscale_method(),
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
            remember_archive_page_name: true,
            key_bindings: default_key_bindings(),
            mouse_bindings: default_mouse_bindings(),
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fast_start_failure: Option<FastStartFailureNotice>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub books: BTreeMap<String, BookRecord>,
}

impl Default for PersistedState {
    fn default() -> Self {
        Self {
            version: 4,
            settings: AppSettings::default(),
            window: WindowPlacement::default(),
            fast_start_failure: None,
            books: BTreeMap::new(),
        }
    }
}

#[derive(Clone)]
pub struct StateStore {
    path: PathBuf,
    books_dir: PathBuf,
    state: PersistedState,
    pending_book: Option<BookRecord>,
    state_dirty: bool,
}

impl StateStore {
    pub fn load() -> Self {
        let path = state_file_path();
        let books_dir = books_dir_path();
        let mut state = fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<PersistedState>(&text).ok())
            .unwrap_or_default();
        state.settings.normalize_product_choices();

        let mut store = Self {
            path,
            books_dir,
            state,
            pending_book: None,
            state_dirty: false,
        };
        store.import_legacy_bookmarks();
        store
    }

    fn read_book_record(&self, book_id: &str) -> Option<BookRecord> {
        if let Some(pending) = &self.pending_book {
            if pending.book_id == book_id {
                return Some(pending.clone());
            }
        }
        let text = fs::read_to_string(book_file_path(&self.books_dir, book_id)).ok()?;
        serde_json::from_str::<BookRecord>(&text).ok()
    }

    pub fn book_record(&self, book_id: &str) -> Option<BookRecord> {
        self.read_book_record(book_id)
    }

    pub fn reading_position(
        &self,
        book_id: &str,
        path: &Path,
        allow_identity_match: bool,
    ) -> Option<ReadingPosition> {
        let record = self.read_book_record(book_id)?;
        if allow_identity_match {
            return Some(ReadingPosition::from_record(&record));
        }
        record
            .path_positions
            .get(path_key(path).as_str())
            .cloned()
    }

    pub fn settings(&self) -> &AppSettings {
        &self.state.settings
    }

    pub fn fast_start_failure_notice(&self) -> Option<&FastStartFailureNotice> {
        self.state.fast_start_failure.as_ref()
    }

    pub fn update_settings(&mut self, mut settings: AppSettings) {
        settings.normalize_product_choices();
        self.state.settings = settings;
        self.state.version = 4;
        let _ = self.save();
    }

    pub fn record_fast_start_failure(&mut self, notice: FastStartFailureNotice) {
        self.state.fast_start_failure = Some(notice);
        self.state.version = 4;
        let _ = self.save();
    }

    pub fn mark_fast_start_failure_notice_shown(&mut self) {
        let Some(notice) = self.state.fast_start_failure.as_mut() else {
            return;
        };
        if notice.shown {
            return;
        }
        notice.shown = true;
        self.state.version = 4;
        let _ = self.save();
    }

    pub fn clear_fast_start_failure_notice(&mut self) {
        if self.state.fast_start_failure.take().is_some() {
            self.state.version = 4;
            let _ = self.save();
        }
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
        self.state_dirty = true;
        true
    }

    pub fn upsert_book_record(&mut self, input: BookRecordInput<'_>) {
        self.flush_pending_book_if_other(input.book_id);
        let (record, _changed) = self.compute_record_update(input, true);
        let _ = self.write_book_record(&record);
    }

    pub fn upsert_book_record_deferred(&mut self, input: BookRecordInput<'_>) -> bool {
        self.flush_pending_book_if_other(input.book_id);
        let (record, changed) = self.compute_record_update(input, false);
        if changed {
            self.pending_book = Some(record);
        }
        changed
    }

    pub fn clear_archive_page_names(&mut self) -> usize {
        self.flush_pending_book();
        let mut cleared = 0;
        for mut record in self.load_all_book_records() {
            if !looks_like_archive_book(&record) {
                continue;
            }
            let mut record_cleared = false;
            if record.last_page_name.take().is_some() {
                record_cleared = true;
                cleared += 1;
            }
            for position in record.path_positions.values_mut() {
                if position.last_page_name.take().is_some() {
                    record_cleared = true;
                    cleared += 1;
                }
            }
            if record_cleared {
                record.updated_at = now_unix_seconds();
                let _ = self.write_book_record(&record);
            }
        }
        cleared
    }

    fn compute_record_update(&self, input: BookRecordInput<'_>, touch: bool) -> (BookRecord, bool) {
        let path_text = input.path.to_string_lossy().to_string();
        let now = now_unix_seconds();
        let existing = self.read_book_record(input.book_id);
        let is_new = existing.is_none();
        let mut record = existing.unwrap_or_else(|| BookRecord {
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
        let path_position_changed = record
            .path_positions
            .get(path_text.as_str())
            .is_none_or(|position| !position.matches_input(&input));
        let mut changed = is_new
            || record.title != title
            || record.last_page != last_page
            || record.last_page_name != last_page_name
            || record.total_pages != input.total_pages
            || record.reading_direction != input.reading_direction
            || record.fit_mode != input.fit_mode
            || record.manual_zoom != input.manual_zoom
            || path_position_changed;

        record.title = title;
        record.last_page = last_page;
        record.last_page_name = last_page_name;
        record.total_pages = input.total_pages;
        record.reading_direction = input.reading_direction;
        record.fit_mode = input.fit_mode;
        record.manual_zoom = input.manual_zoom;
        if path_position_changed || touch {
            record
                .path_positions
                .insert(path_text.clone(), ReadingPosition::from_input(&input, now));
        }

        if !record.known_paths.iter().any(|known| known == &path_text) {
            record.known_paths.push(path_text);
            changed = true;
        }
        if record.known_paths.len() > 8 {
            let extra = record.known_paths.len() - 8;
            record.known_paths.drain(0..extra);
            changed = true;
        }

        if changed || touch {
            record.updated_at = now;
        }
        (record, changed || touch)
    }

    pub fn save(&mut self) -> std::io::Result<()> {
        self.write_state_file()
    }

    pub fn flush(&mut self) -> std::io::Result<()> {
        let mut result = Ok(());
        if let Some(record) = self.pending_book.take() {
            result = self.write_book_record(&record);
        }
        if self.state_dirty {
            let state_result = self.write_state_file();
            if result.is_ok() {
                result = state_result;
            }
        }
        result
    }

    fn write_state_file(&mut self) -> std::io::Result<()> {
        let started = Instant::now();
        let result = (|| {
            let text = serde_json::to_string_pretty(&self.state)?;
            write_atomic(&self.path, &text)
        })();
        if result.is_ok() {
            self.state_dirty = false;
        }
        perf_trace::record_duration_if_at_least(
            "state_save",
            started.elapsed(),
            Duration::from_millis(20),
            &[PerfField::Bool("success", result.is_ok())],
        );
        result
    }

    fn write_book_record(&mut self, record: &BookRecord) -> std::io::Result<()> {
        if self
            .pending_book
            .as_ref()
            .is_some_and(|pending| pending.book_id == record.book_id)
        {
            self.pending_book = None;
        }
        let text = serde_json::to_string_pretty(record)?;
        write_atomic(&book_file_path(&self.books_dir, &record.book_id), &text)
    }

    fn load_all_book_records(&self) -> Vec<BookRecord> {
        let mut records: Vec<BookRecord> = Vec::new();
        if let Ok(entries) = fs::read_dir(&self.books_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                    continue;
                }
                let Ok(text) = fs::read_to_string(&path) else {
                    continue;
                };
                if let Ok(record) = serde_json::from_str::<BookRecord>(&text) {
                    records.push(record);
                }
            }
        }
        if let Some(pending) = &self.pending_book {
            match records
                .iter_mut()
                .find(|record| record.book_id == pending.book_id)
            {
                Some(slot) => *slot = pending.clone(),
                None => records.push(pending.clone()),
            }
        }
        records
    }

    fn flush_pending_book(&mut self) {
        if let Some(record) = self.pending_book.take() {
            let _ = self.write_book_record(&record);
        }
    }

    // The single pending buffer only ever holds the current book; if a write for
    // a different book arrives, persist the buffered one first so it is not lost.
    fn flush_pending_book_if_other(&mut self, book_id: &str) {
        if self
            .pending_book
            .as_ref()
            .is_some_and(|pending| pending.book_id != book_id)
        {
            self.flush_pending_book();
        }
    }

    // One-time import from the old monolithic state.json. During beta the resume
    // history is disposable, so keep only the manual page bookmarks and discard
    // the reading positions; books without bookmarks are dropped entirely.
    fn import_legacy_bookmarks(&mut self) {
        if self.state.books.is_empty() {
            return;
        }
        let books = std::mem::take(&mut self.state.books);
        for record in books.into_values() {
            if record.page_bookmarks.is_empty() {
                continue;
            }
            let rescued = BookRecord {
                book_id: record.book_id,
                title: record.title,
                last_page: 0,
                last_page_name: None,
                total_pages: record.total_pages,
                known_paths: record.known_paths,
                reading_direction: record.reading_direction,
                fit_mode: record.fit_mode,
                manual_zoom: None,
                path_positions: BTreeMap::new(),
                page_bookmarks: record.page_bookmarks,
                updated_at: record.updated_at,
            };
            let _ = self.write_book_record(&rescued);
        }
        let _ = self.write_state_file();
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

fn looks_like_archive_book(record: &BookRecord) -> bool {
    record.known_paths.iter().any(|path| {
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

fn books_dir_path() -> PathBuf {
    ProjectDirs::from("", "", "SuiSuiView")
        .map(|dirs| dirs.data_dir().join("books"))
        .unwrap_or_else(|| PathBuf::from("SuiSuiView-books"))
}

fn book_file_path(books_dir: &Path, book_id: &str) -> PathBuf {
    books_dir.join(format!("{}.json", sanitize_book_id(book_id)))
}

// book_id is always "<kind>:<hex>"; ':' is invalid in Windows file names, so map
// any non-portable character to '_'. The kind prefix keeps ids collision-free.
fn sanitize_book_id(book_id: &str) -> String {
    book_id
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
                character
            } else {
                '_'
            }
        })
        .collect()
}

fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("state");
    let tmp = path.with_file_name(format!("{file_name}.{}.tmp", std::process::id()));
    fs::write(&tmp, text)?;
    fs::rename(&tmp, path)
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
    DEFAULT_MANUAL_CACHE_MB
}

fn default_cpu_upscale_filter() -> CpuScaleFilter {
    CpuScaleFilter::CatmullRom
}

fn default_cpu_downscale_filter() -> CpuScaleFilter {
    CpuScaleFilter::Hamming
}

fn default_wgpu_downscale_method() -> WgpuDownscaleMethod {
    WgpuDownscaleMethod::PyramidLanczos3
}

pub fn default_top_bar_cpu_scale_filters() -> Vec<CpuScaleFilter> {
    DEFAULT_TOP_BAR_CPU_SCALE_FILTERS.to_vec()
}

pub fn default_top_bar_wgpu_upscale_methods() -> Vec<WgpuUpscaleMethod> {
    DEFAULT_TOP_BAR_WGPU_UPSCALE_METHODS.to_vec()
}

pub fn default_top_bar_wgpu_downscale_methods() -> Vec<WgpuDownscaleMethod> {
    DEFAULT_TOP_BAR_WGPU_DOWNSCALE_METHODS.to_vec()
}
