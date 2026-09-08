use super::{
    adjacent_seed_matches_successor, should_skip_memory_aware_adjacent_seed,
    should_skip_memory_aware_adjacent_seed_source, ADJACENT_SEED_LARGE_SOURCE_BYTES,
    ADJACENT_SEED_LARGE_SOURCE_LONG_EDGE,
};
use crate::core::source::{BookSource, SourceError};
use image::{codecs::png::PngEncoder, ColorType, ImageEncoder};
use std::path::{Path, PathBuf};

#[test]
fn saved_fit_width_requires_a_larger_seed_than_inherited_fit_page() {
    let view = super::SeedTargetView {
        fit_mode: crate::core::state::FitMode::FitPage,
        manual_zoom: 1.0,
        page_viewport: egui::vec2(1600.0, 1000.0),
        pixels_per_point: 1.0,
    };
    let inherited = super::seed_target_long_edge_from_dimensions(4096, 6144, 2048, Some(view));
    let destination = super::seed_target_long_edge_from_dimensions(
        4096,
        6144,
        2048,
        Some(super::SeedTargetView {
            fit_mode: crate::core::state::FitMode::FitWidth,
            ..view
        }),
    );
    assert!(destination > inherited);
    assert_eq!(
        destination,
        super::target_long_edge_for_view(
            crate::core::state::FitMode::FitWidth,
            1.0,
            view.page_viewport,
            1.0,
            &[super::OriginalPageSize {
                width: 4096.0,
                height: 6144.0
            }],
        )
    );
}

#[test]
fn adjacent_seed_successor_match_requires_path_and_direction() {
    let current = Path::new("book-1.cbz");
    let successor = Path::new("book-2.cbz");

    assert!(adjacent_seed_matches_successor(
        current,
        successor,
        1,
        current,
        Some(successor),
        1,
    ));
    assert!(!adjacent_seed_matches_successor(
        current,
        Path::new("book-3.cbz"),
        1,
        current,
        Some(successor),
        1,
    ));
    assert!(!adjacent_seed_matches_successor(
        current,
        successor,
        -1,
        current,
        Some(successor),
        1,
    ));
}

#[test]
fn adjacent_seed_direction_match_keeps_existing_sibling_behavior() {
    assert!(adjacent_seed_matches_successor(
        Path::new("book-1.cbz"),
        Path::new("book-2.cbz"),
        1,
        Path::new("book-1.cbz"),
        None,
        1,
    ));
}

#[test]
fn memory_aware_adjacent_seed_skips_8192px_sources() {
    let bytes = png_bytes(ADJACENT_SEED_LARGE_SOURCE_LONG_EDGE, 1);

    assert!(should_skip_memory_aware_adjacent_seed(&bytes));
}

#[test]
fn memory_aware_adjacent_seed_keeps_smaller_sources() {
    let bytes = png_bytes(ADJACENT_SEED_LARGE_SOURCE_LONG_EDGE - 1, 1);

    assert!(!should_skip_memory_aware_adjacent_seed(&bytes));
}

#[test]
fn memory_aware_adjacent_seed_keeps_unknown_dimensions() {
    assert!(!should_skip_memory_aware_adjacent_seed(b"not an image"));
}

#[test]
fn memory_aware_adjacent_seed_skips_large_known_source_bytes() {
    let source = TestSource {
        byte_size: Some(ADJACENT_SEED_LARGE_SOURCE_BYTES),
        bytes: Vec::new(),
    };

    assert!(should_skip_memory_aware_adjacent_seed_source(&source, 0));
}

#[test]
fn memory_aware_adjacent_seed_keeps_large_bytes_with_smaller_dimensions() {
    let source = TestSource {
        byte_size: Some(ADJACENT_SEED_LARGE_SOURCE_BYTES),
        bytes: png_bytes(ADJACENT_SEED_LARGE_SOURCE_LONG_EDGE - 1, 1),
    };

    assert!(!should_skip_memory_aware_adjacent_seed_source(&source, 0));
}

#[test]
fn memory_aware_adjacent_seed_keeps_small_or_unknown_source_bytes() {
    let small = TestSource {
        byte_size: Some(ADJACENT_SEED_LARGE_SOURCE_BYTES - 1),
        bytes: Vec::new(),
    };
    let unknown = TestSource {
        byte_size: None,
        bytes: Vec::new(),
    };

    assert!(!should_skip_memory_aware_adjacent_seed_source(&small, 0));
    assert!(!should_skip_memory_aware_adjacent_seed_source(&unknown, 0));
}

fn png_bytes(width: u32, height: u32) -> Vec<u8> {
    let pixels = vec![0; width as usize * height as usize * 4];
    let mut bytes = Vec::new();
    PngEncoder::new(&mut bytes)
        .write_image(&pixels, width, height, ColorType::Rgba8.into())
        .expect("test PNG should encode");
    bytes
}

struct TestSource {
    byte_size: Option<u64>,
    bytes: Vec<u8>,
}

impl BookSource for TestSource {
    fn title(&self) -> &str {
        "test"
    }

    fn source_path(&self) -> &Path {
        Path::new("test")
    }

    fn book_id(&self) -> &str {
        "test"
    }

    fn page_count(&self) -> usize {
        1
    }

    fn page_name(&self, _index: usize) -> Option<&str> {
        Some("page.png")
    }

    fn page_file_path(&self, _index: usize) -> Option<PathBuf> {
        None
    }

    fn page_byte_size(&self, _index: usize) -> Option<u64> {
        self.byte_size
    }

    fn read_page(&self, _index: usize) -> Result<Vec<u8>, SourceError> {
        Ok(self.bytes.clone())
    }

    fn read_page_prefix(&self, _index: usize, max_bytes: usize) -> Result<Vec<u8>, SourceError> {
        Ok(self.bytes[..self.bytes.len().min(max_bytes)].to_vec())
    }
}
