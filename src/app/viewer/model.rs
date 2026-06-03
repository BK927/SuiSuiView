use super::super::gpu_paint::GpuPaintSourceKey;
use crate::core::effects::ViewEffects;
use crate::core::state::{DisplayUpscaler, PageTransitionStyle, ReadingDirection, WgpuDownscaler};
use crate::core::worker::PreparedPage;
use eframe::egui::{TextureHandle, Vec2};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

const SMART_WIDE_ASPECT: f32 = 1.20;
const SMART_TALL_ASPECT: f32 = 2.40;
const SMART_HEIGHT_MISMATCH: f32 = 0.25;
const SMART_ASPECT_MISMATCH: f32 = 0.30;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app) enum ViewMode {
    Single,
    DoubleLeftToRight,
    DoubleRightToLeft,
    SmartDoubleLeftToRight,
    SmartDoubleRightToLeft,
}

impl ViewMode {
    pub(in crate::app) fn step(self) -> usize {
        match self {
            Self::Single => 1,
            Self::DoubleLeftToRight
            | Self::DoubleRightToLeft
            | Self::SmartDoubleLeftToRight
            | Self::SmartDoubleRightToLeft => 2,
        }
    }

    pub(in crate::app) fn is_smart(self) -> bool {
        matches!(
            self,
            Self::SmartDoubleLeftToRight | Self::SmartDoubleRightToLeft
        )
    }

    pub(in crate::app) fn reading_direction(self) -> Option<ReadingDirection> {
        match self {
            Self::Single => None,
            Self::DoubleLeftToRight | Self::SmartDoubleLeftToRight => {
                Some(ReadingDirection::LeftToRight)
            }
            Self::DoubleRightToLeft | Self::SmartDoubleRightToLeft => {
                Some(ReadingDirection::RightToLeft)
            }
        }
    }

    pub(in crate::app) fn is_right_to_left(self, fallback: ReadingDirection) -> bool {
        self.reading_direction().unwrap_or(fallback) == ReadingDirection::RightToLeft
    }

    pub(in crate::app) fn with_reading_direction(self, direction: ReadingDirection) -> Self {
        match self {
            Self::Single => Self::Single,
            Self::DoubleLeftToRight | Self::DoubleRightToLeft => match direction {
                ReadingDirection::LeftToRight => Self::DoubleLeftToRight,
                ReadingDirection::RightToLeft => Self::DoubleRightToLeft,
            },
            Self::SmartDoubleLeftToRight | Self::SmartDoubleRightToLeft => match direction {
                ReadingDirection::LeftToRight => Self::SmartDoubleLeftToRight,
                ReadingDirection::RightToLeft => Self::SmartDoubleRightToLeft,
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(in crate::app) struct PageMetrics {
    pub(in crate::app) width: f32,
    pub(in crate::app) height: f32,
}

impl PageMetrics {
    pub(in crate::app) fn from_page(page: &PreparedPage) -> Self {
        Self {
            width: page.original_width.max(1) as f32,
            height: page.original_height.max(1) as f32,
        }
    }

    pub(in crate::app) fn aspect(self) -> f32 {
        self.width / self.height
    }

    pub(in crate::app) fn is_standalone(self) -> bool {
        self.aspect() >= SMART_WIDE_ASPECT || self.height / self.width >= SMART_TALL_ASPECT
    }

    pub(in crate::app) fn can_pair_with(self, other: Self) -> bool {
        if self.is_standalone() || other.is_standalone() {
            return false;
        }
        relative_difference(self.height, other.height) <= SMART_HEIGHT_MISMATCH
            && relative_difference(self.aspect(), other.aspect()) <= SMART_ASPECT_MISMATCH
    }
}

pub(in crate::app) fn ordered_spread_indices(
    mut indices: Vec<usize>,
    mode: ViewMode,
    fallback: ReadingDirection,
) -> Vec<usize> {
    if mode.is_right_to_left(fallback) {
        indices.reverse();
    }
    indices
}

pub(in crate::app) fn double_spread_indices(page: usize, page_count: usize) -> Vec<usize> {
    if page_count == 0 {
        return Vec::new();
    }
    let page = page.min(page_count - 1);
    let mut indices = vec![page];
    if let Some(next) = page.checked_add(1).filter(|next| *next < page_count) {
        indices.push(next);
    }
    indices
}

pub(in crate::app) fn smart_spread_indices_for_metrics(
    page: usize,
    page_count: usize,
    metrics: &HashMap<usize, PageMetrics>,
) -> Vec<usize> {
    if page_count == 0 {
        return Vec::new();
    }
    let page = page.min(page_count - 1);
    let anchor = page - (page % 2);
    let Some(next) = anchor.checked_add(1).filter(|next| *next < page_count) else {
        return vec![page];
    };
    let Some(anchor_metrics) = metrics.get(&anchor).copied() else {
        return vec![page];
    };
    let Some(next_metrics) = metrics.get(&next).copied() else {
        return vec![page];
    };
    if anchor_metrics.can_pair_with(next_metrics) {
        vec![anchor, next]
    } else {
        vec![page]
    }
}

pub(in crate::app) fn relative_difference(left: f32, right: f32) -> f32 {
    let base = left.max(right).max(1.0);
    (left - right).abs() / base
}

pub(in crate::app) fn worker_center_page_for_mode(current_page: usize, mode: ViewMode) -> usize {
    if mode.is_smart() {
        current_page - (current_page % 2)
    } else {
        current_page
    }
}

pub(in crate::app) struct Transition {
    pub(in crate::app) from_indices: Vec<usize>,
    pub(in crate::app) target_long_edge: u32,
    pub(in crate::app) started_at: Instant,
    pub(in crate::app) screen_sign: f32,
    pub(in crate::app) style: PageTransitionStyle,
}

pub(in crate::app) enum PageVisual {
    Ready {
        texture: TextureHandle,
        size: Vec2,
    },
    ReadyGpu {
        source_key: GpuPaintSourceKey,
        image_size: [usize; 2],
        rgba: Arc<[u8]>,
        size: Vec2,
        effects: ViewEffects,
        display_upscaler: DisplayUpscaler,
        wgpu_downscaler: WgpuDownscaler,
    },
    Loading {
        index: usize,
    },
    Failed {
        index: usize,
        message: String,
    },
}

pub(in crate::app) fn page_visual_size(visual: &PageVisual) -> Vec2 {
    match visual {
        PageVisual::Ready { size, .. } => *size,
        PageVisual::ReadyGpu { size, .. } => *size,
        PageVisual::Loading { .. } | PageVisual::Failed { .. } => Vec2::new(900.0, 1300.0),
    }
}
