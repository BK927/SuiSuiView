use crate::core::i18n::I18n;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

mod book_files;
mod bookmarks;
mod decoders;
mod display;
mod fast_start;
mod input;
mod rendering;
#[cfg(test)]
mod scale_plan_tests;
mod scalers;
#[cfg(test)]
mod settings_tests;
pub use crate::core::deband::DebandStrength;
pub use crate::core::i18n::Language;
use bookmarks::path_key;
pub use bookmarks::{
    BookRecord, BookRecordInput, PageBookmark, PageBookmarkEntry, ReadingPosition,
    UpscaleProbeRecord, UPSCALE_PROBE_VERSION,
};
pub use decoders::{DecodeMode, DecoderPreference, DecoderPreferences};
pub use display::{GpuEffectMode, RefineUpscaler, WgpuUpscaleMethod};
pub use fast_start::FastStartFailureNotice;
pub use input::{
    adopt_default_bindings_for_new_commands, default_key_bindings, default_mouse_bindings,
    CommandId, KeyBinding, KeyCode, KeyShortcut, MouseBinding, MouseGesture,
};
pub use rendering::RendererMode;
pub use scalers::{
    CpuScaleFilter, ResizeFilter, WgpuDownscaleMethod, WgpuScaleDirection, WgpuScalePlan,
    CPU_DOWNSCALE_FILTER, WGPU_DOWNSCALE_METHOD,
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

pub const FIXED_2X_SR_MIN_SCALE_PCT_MIN: u32 = 100;
pub const FIXED_2X_SR_MIN_SCALE_PCT_MAX: u32 = 200;

pub const PIXEL_GRID_MIN_ZOOM_PCT_MIN: u32 = 200;
pub const PIXEL_GRID_MIN_ZOOM_PCT_MAX: u32 = 6400;

/// Vertical-strip scroll sensitivity, percent of the raw input delta. A wheel
/// notch is ~40 egui points, far too little for webtoon strips, hence the
/// boosted default; drag stays closer to direct manipulation.
pub const STRIP_WHEEL_SCROLL_PCT_MIN: u32 = 100;
pub const STRIP_WHEEL_SCROLL_PCT_MAX: u32 = 1200;
pub const STRIP_DRAG_SCROLL_PCT_MIN: u32 = 100;
pub const STRIP_DRAG_SCROLL_PCT_MAX: u32 = 400;

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
    #[serde(default)]
    pub show_filename_overlay: bool,
    #[serde(default = "default_true")]
    pub show_main_border: bool,
    #[serde(default = "default_true")]
    pub show_page_arrows: bool,
    #[serde(default)]
    pub pixel_grid_enabled: bool,
    #[serde(default = "default_pixel_grid_min_zoom_pct")]
    pub pixel_grid_min_zoom_pct: u32,
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
    #[serde(default)]
    pub gpu_effect_mode: GpuEffectMode,
    #[serde(default)]
    pub renderer_mode: RendererMode,
    #[serde(default)]
    pub wgpu_upscale_method: WgpuUpscaleMethod,
    /// Idle-scheduled heavy "refine" (정련) upscaler; `Off` by default. Renders
    /// once per page while idle into the realtime-SR stage cache.
    #[serde(default)]
    pub refine_upscaler: RefineUpscaler,
    /// Debanding strength for the WGPU display path; `Off` by default.
    #[serde(default)]
    pub deband: DebandStrength,
    /// Linear-light averaging for the WGPU downscale legs. Off (gamma) by
    /// default: measured stroke washout on line art outweighed mean-luminance
    /// correctness for comic content; photos benefit from turning it on.
    #[serde(default)]
    pub linear_light_downscale: bool,
    #[serde(default = "default_fixed_2x_sr_min_scale_pct")]
    pub fixed_2x_sr_min_scale_pct: u32,
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
    #[serde(default = "default_strip_wheel_scroll_pct")]
    pub strip_wheel_scroll_pct: u32,
    #[serde(default = "default_strip_drag_scroll_pct")]
    pub strip_drag_scroll_pct: u32,
    /// When set, keyboard viewport-steps in vertical-strip mode snap to panel
    /// boundaries (gutters) instead of taking a fixed fraction-of-a-viewport jump.
    #[serde(default = "default_true")]
    pub strip_panel_snap: bool,

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
    /// Commands this profile has been offered default shortcuts for; commands
    /// missing here get theirs appended on load (see
    /// `adopt_default_bindings_for_new_commands`). Empty on legacy profiles.
    #[serde(default)]
    pub seen_commands: Vec<CommandId>,
}

impl AppSettings {
    pub fn normalize_product_choices(&mut self) {
        if !self.wgpu_upscale_method.user_selectable() {
            self.wgpu_upscale_method = WgpuUpscaleMethod::Auto;
        }

        self.fixed_2x_sr_min_scale_pct = self
            .fixed_2x_sr_min_scale_pct
            .clamp(FIXED_2X_SR_MIN_SCALE_PCT_MIN, FIXED_2X_SR_MIN_SCALE_PCT_MAX);

        self.pixel_grid_min_zoom_pct = self
            .pixel_grid_min_zoom_pct
            .clamp(PIXEL_GRID_MIN_ZOOM_PCT_MIN, PIXEL_GRID_MIN_ZOOM_PCT_MAX);

        self.strip_wheel_scroll_pct = self
            .strip_wheel_scroll_pct
            .clamp(STRIP_WHEEL_SCROLL_PCT_MIN, STRIP_WHEEL_SCROLL_PCT_MAX);
        self.strip_drag_scroll_pct = self
            .strip_drag_scroll_pct
            .clamp(STRIP_DRAG_SCROLL_PCT_MIN, STRIP_DRAG_SCROLL_PCT_MAX);

        adopt_default_bindings_for_new_commands(&mut self.key_bindings, &mut self.seen_commands);
    }

    pub fn strip_wheel_scroll_multiplier(&self) -> f32 {
        self.strip_wheel_scroll_pct
            .clamp(STRIP_WHEEL_SCROLL_PCT_MIN, STRIP_WHEEL_SCROLL_PCT_MAX) as f32
            / 100.0
    }

    pub fn strip_drag_scroll_multiplier(&self) -> f32 {
        self.strip_drag_scroll_pct
            .clamp(STRIP_DRAG_SCROLL_PCT_MIN, STRIP_DRAG_SCROLL_PCT_MAX) as f32
            / 100.0
    }

    pub fn fixed_2x_sr_min_scale(&self) -> f32 {
        self.fixed_2x_sr_min_scale_pct
            .clamp(FIXED_2X_SR_MIN_SCALE_PCT_MIN, FIXED_2X_SR_MIN_SCALE_PCT_MAX) as f32
            / 100.0
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
    pub outer_position_px: Option<[i32; 2]>,
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
            show_filename_overlay: false,
            show_main_border: true,
            show_page_arrows: true,
            pixel_grid_enabled: false,
            pixel_grid_min_zoom_pct: default_pixel_grid_min_zoom_pct(),
            edge_page_action: EdgePageAction::Stop,
            image_edge_page_action: EdgePageAction::Wrap,
            archive_edge_page_action: default_archive_edge_page_action(),
            decode_mode: DecodeMode::AutoFast,
            decoder_preferences: DecoderPreferences::default(),
            fast_sampled_scaled_decode: true,
            cpu_upscale_filter: default_cpu_upscale_filter(),
            gpu_effect_mode: GpuEffectMode::Auto,
            renderer_mode: RendererMode::LowMemoryGlow,
            wgpu_upscale_method: WgpuUpscaleMethod::None,
            refine_upscaler: RefineUpscaler::Off,
            deband: DebandStrength::Off,
            linear_light_downscale: false,
            fixed_2x_sr_min_scale_pct: default_fixed_2x_sr_min_scale_pct(),
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
            strip_wheel_scroll_pct: default_strip_wheel_scroll_pct(),
            strip_drag_scroll_pct: default_strip_drag_scroll_pct(),
            strip_panel_snap: default_true(),
            apply_exif_orientation: true,
            apply_embedded_icc: false,
            auto_save_reading_position: true,
            resume_by_file_identity: true,
            remember_archive_page_name: true,
            key_bindings: default_key_bindings(),
            mouse_bindings: default_mouse_bindings(),
            seen_commands: CommandId::ALL.to_vec(),
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
        let path = book_files::state_file_path();
        let books_dir = book_files::books_dir_path();
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
        record.path_positions.get(path_key(path).as_str()).cloned()
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
            view_mode: None,
            strip_offset_frac: None,
            path_positions: BTreeMap::new(),
            page_bookmarks: Vec::new(),
            upscale_probe: None,
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
            || record.view_mode.as_deref() != input.view_mode
            || record.strip_offset_frac != input.strip_offset_frac
            || path_position_changed;

        record.title = title;
        record.last_page = last_page;
        record.last_page_name = last_page_name;
        record.total_pages = input.total_pages;
        record.reading_direction = input.reading_direction;
        record.fit_mode = input.fit_mode;
        record.manual_zoom = input.manual_zoom;
        record.view_mode = input.view_mode.map(ToOwned::to_owned);
        record.strip_offset_frac = input.strip_offset_frac;
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

fn default_fixed_2x_sr_min_scale_pct() -> u32 {
    (scalers::FIXED_2X_SR_SMALL_SCALE_MIN * 100.0) as u32
}

fn default_pixel_grid_min_zoom_pct() -> u32 {
    800
}

fn default_strip_wheel_scroll_pct() -> u32 {
    400
}

fn default_strip_drag_scroll_pct() -> u32 {
    150
}

fn default_cpu_upscale_filter() -> CpuScaleFilter {
    CpuScaleFilter::CatmullRom
}

pub fn default_top_bar_cpu_scale_filters() -> Vec<CpuScaleFilter> {
    DEFAULT_TOP_BAR_CPU_SCALE_FILTERS.to_vec()
}

pub fn default_top_bar_wgpu_upscale_methods() -> Vec<WgpuUpscaleMethod> {
    DEFAULT_TOP_BAR_WGPU_UPSCALE_METHODS.to_vec()
}
