use super::{
    open_seed_target_long_edge, resolve_open_view, selected_open_page,
    startup_seed_target_long_edge, OpenViewFallback, ViewMode,
};
use crate::core::source::{BookSource, SourceError};
use crate::core::state::{FitMode, ReadingDirection, ReadingPosition, WindowPlacement};
use crate::core::worker::{DEFAULT_TARGET_LONG_EDGE, MAX_TARGET_LONG_EDGE, MIN_TARGET_LONG_EDGE};
use std::path::{Path, PathBuf};

#[test]
fn startup_seed_target_keeps_default_floor_for_normal_windows() {
    let placement = WindowPlacement {
        inner_size: Some([1280.0, 820.0]),
        outer_position: None,
        outer_position_px: None,
        normal_rect_px: None,
        maximized: false,
    };

    assert_eq!(
        startup_seed_target_long_edge(&placement),
        DEFAULT_TARGET_LONG_EDGE
    );
}

#[test]
fn startup_seed_target_uses_larger_floor_for_maximized_windows() {
    let placement = WindowPlacement {
        inner_size: Some([1280.0, 820.0]),
        outer_position: None,
        outer_position_px: None,
        normal_rect_px: None,
        maximized: true,
    };

    assert_eq!(startup_seed_target_long_edge(&placement), 2304);
}

#[test]
fn startup_seed_target_uses_default_without_stored_size() {
    let placement = WindowPlacement {
        inner_size: None,
        outer_position: None,
        outer_position_px: None,
        normal_rect_px: None,
        maximized: false,
    };

    assert_eq!(
        startup_seed_target_long_edge(&placement),
        DEFAULT_TARGET_LONG_EDGE
    );
}

#[test]
fn open_seed_target_uses_current_navigation_target() {
    assert_eq!(open_seed_target_long_edge(3072), 3072);
}

#[test]
fn open_seed_target_stays_in_navigation_range() {
    assert_eq!(open_seed_target_long_edge(512), MIN_TARGET_LONG_EDGE);
    assert_eq!(
        open_seed_target_long_edge(MAX_TARGET_LONG_EDGE + 2048),
        MAX_TARGET_LONG_EDGE
    );
}

#[test]
fn sibling_open_view_fallback_preserves_fit_width_without_saved_position() {
    let resolved = resolve_open_view(
        None,
        Some(OpenViewFallback {
            reading_direction: ReadingDirection::LeftToRight,
            fit_mode: FitMode::FitWidth,
            manual_zoom: 1.75,
            view_mode: ViewMode::VerticalStrip,
        }),
        true,
    );

    assert_eq!(resolved.reading_direction, ReadingDirection::LeftToRight);
    assert_eq!(resolved.fit_mode, FitMode::FitWidth);
    assert_eq!(resolved.manual_zoom, 1.75);
    // A sibling book without its own record inherits the current mode.
    assert_eq!(resolved.view_mode, Some(ViewMode::VerticalStrip));
    assert_eq!(resolved.strip_offset_frac, None);
}

#[test]
fn saved_reading_position_wins_over_sibling_view_fallback() {
    let saved = ReadingPosition {
        last_page: 3,
        last_page_name: None,
        reading_direction: ReadingDirection::RightToLeft,
        fit_mode: FitMode::FitPage,
        manual_zoom: Some(1.25),
        view_mode: None,
        strip_offset_frac: None,
        updated_at: 10,
    };

    let resolved = resolve_open_view(
        Some(&saved),
        Some(OpenViewFallback {
            reading_direction: ReadingDirection::LeftToRight,
            fit_mode: FitMode::FitWidth,
            manual_zoom: 2.0,
            view_mode: ViewMode::VerticalStrip,
        }),
        true,
    );

    assert_eq!(resolved.reading_direction, ReadingDirection::RightToLeft);
    assert_eq!(resolved.fit_mode, FitMode::FitPage);
    assert_eq!(resolved.manual_zoom, 1.25);
    // Saved position with no token wins; the fallback mode is ignored.
    assert_eq!(resolved.view_mode, None);
}

#[test]
fn direct_open_without_saved_position_keeps_default_view() {
    let resolved = resolve_open_view(None, None, true);

    assert_eq!(resolved.reading_direction, ReadingDirection::default());
    assert_eq!(resolved.fit_mode, FitMode::default());
    assert_eq!(resolved.manual_zoom, 1.0);
    assert_eq!(resolved.view_mode, None);
    assert_eq!(resolved.strip_offset_frac, None);
}

#[test]
fn saved_vertical_strip_position_resolves_mode_and_offset() {
    let saved = ReadingPosition {
        last_page: 4,
        last_page_name: None,
        reading_direction: ReadingDirection::LeftToRight,
        fit_mode: FitMode::FitWidth,
        manual_zoom: None,
        view_mode: Some("vertical_strip".to_owned()),
        strip_offset_frac: Some(0.42),
        updated_at: 7,
    };

    let resolved = resolve_open_view(Some(&saved), None, true);

    assert_eq!(resolved.view_mode, Some(ViewMode::VerticalStrip));
    assert_eq!(resolved.strip_offset_frac, Some(0.42));
}

#[test]
fn saved_position_with_unknown_token_yields_no_mode() {
    let mut saved = reading_position(2, None);
    saved.view_mode = Some("webtoon".to_owned());

    let resolved = resolve_open_view(Some(&saved), None, true);

    assert_eq!(resolved.view_mode, None);
}

#[test]
fn selected_open_page_prefers_explicit_page_over_saved_position() {
    let source = TestSource::new(5);
    let saved = reading_position(3, None);

    assert_eq!(
        selected_open_page(&source, Some(1), None, Some(&saved), None),
        1
    );
}

#[test]
fn selected_open_page_clamps_explicit_page_to_page_count() {
    let source = TestSource::new(2);
    let saved = reading_position(0, None);

    assert_eq!(
        selected_open_page(&source, Some(9), None, Some(&saved), None),
        1
    );
}

#[test]
fn selected_open_page_keeps_bookmark_jump_before_forced_page() {
    let source = TestSource::with_names(vec!["001.jpg", "002.jpg", "003.jpg", "004.jpg"]);
    let saved = reading_position(0, Some("003.jpg"));

    assert_eq!(
        selected_open_page(&source, None, Some(1), Some(&saved), Some(3)),
        3
    );
}

#[test]
fn selected_open_page_keeps_forced_page_before_saved_position() {
    let source = TestSource::new(5);
    let saved = reading_position(4, None);

    assert_eq!(
        selected_open_page(&source, None, Some(2), Some(&saved), None),
        2
    );
}

fn reading_position(last_page: usize, last_page_name: Option<&str>) -> ReadingPosition {
    ReadingPosition {
        last_page,
        last_page_name: last_page_name.map(str::to_owned),
        reading_direction: ReadingDirection::default(),
        fit_mode: FitMode::default(),
        manual_zoom: None,
        view_mode: None,
        strip_offset_frac: None,
        updated_at: 0,
    }
}

struct TestSource {
    source_path: PathBuf,
    page_names: Vec<String>,
}

impl TestSource {
    fn new(page_count: usize) -> Self {
        Self::with_names(
            (0..page_count)
                .map(|index| format!("{index}.jpg"))
                .collect(),
        )
    }

    fn with_names(page_names: Vec<impl Into<String>>) -> Self {
        Self {
            source_path: PathBuf::from("book"),
            page_names: page_names.into_iter().map(Into::into).collect(),
        }
    }
}

impl BookSource for TestSource {
    fn title(&self) -> &str {
        "test"
    }

    fn source_path(&self) -> &Path {
        &self.source_path
    }

    fn book_id(&self) -> &str {
        "test"
    }

    fn page_count(&self) -> usize {
        self.page_names.len()
    }

    fn page_name(&self, index: usize) -> Option<&str> {
        self.page_names.get(index).map(String::as_str)
    }

    fn read_page(&self, _index: usize) -> Result<Vec<u8>, SourceError> {
        Ok(Vec::new())
    }
}
