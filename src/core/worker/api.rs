use super::MAX_IMAGE_DIMENSION;
use crate::core::source::{PageId, SharedSource};
use crate::core::state::{CpuScaleFilter, DecoderPreferences};
use crossbeam_channel::Sender;
use egui::{Color32, ColorImage};
use std::sync::Arc;

use super::clamp_target_long_edge;

const WORKER_CACHE_BYTES: usize = 48 * 1024 * 1024;

pub const DEFAULT_TARGET_LONG_EDGE: u32 = 2048;
pub const MIN_TARGET_LONG_EDGE: u32 = 1024;
pub const MAX_TARGET_LONG_EDGE: u32 = 4096;
pub const MAX_ORIGINAL_TARGET_LONG_EDGE: u32 = MAX_IMAGE_DIMENSION;
pub const PREVIEW_TARGET_LONG_EDGE: u32 = MIN_TARGET_LONG_EDGE;
pub const FULL_QUALITY_PREFETCH_FORWARD_PAGES: usize = 12;
pub const FULL_QUALITY_PREFETCH_BACKWARD_PAGES: usize = 1;
pub const PREVIEW_PREFETCH_FORWARD_PAGES: usize = 24;
pub const PREVIEW_PREFETCH_BACKWARD_PAGES: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavigationDirection {
    Forward,
    Backward,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PreparedTargetIntent {
    NormalNavigation,
    LargeFitDisplay,
    OriginalInspection,
}

impl PreparedTargetIntent {
    pub fn is_original_inspection(self) -> bool {
        matches!(self, Self::OriginalInspection)
    }

    pub fn keeps_exact_prefetch_lightweight(self) -> bool {
        matches!(self, Self::LargeFitDisplay | Self::OriginalInspection)
    }
}

pub fn preview_prefetch_indices(
    center: usize,
    page_count: usize,
    direction: NavigationDirection,
    visible_pages: usize,
) -> Vec<usize> {
    if page_count == 0 {
        return Vec::new();
    }

    let center = center.min(page_count - 1);
    let mut indices = Vec::with_capacity(
        visible_pages
            .max(1)
            .saturating_add(PREVIEW_PREFETCH_FORWARD_PAGES)
            .saturating_add(PREVIEW_PREFETCH_BACKWARD_PAGES),
    );
    for offset in 0..visible_pages.max(1) {
        if let Some(index) = center.checked_add(offset) {
            push_unique_prefetch_index(&mut indices, index, page_count);
        }
    }

    match direction {
        NavigationDirection::Forward => {
            for offset in 1..=PREVIEW_PREFETCH_FORWARD_PAGES {
                if let Some(index) = center.checked_add(offset) {
                    push_unique_prefetch_index(&mut indices, index, page_count);
                }
            }
            for offset in 1..=PREVIEW_PREFETCH_BACKWARD_PAGES {
                if let Some(index) = center.checked_sub(offset) {
                    push_unique_prefetch_index(&mut indices, index, page_count);
                }
            }
        }
        NavigationDirection::Backward => {
            for offset in 1..=PREVIEW_PREFETCH_FORWARD_PAGES {
                if let Some(index) = center.checked_sub(offset) {
                    push_unique_prefetch_index(&mut indices, index, page_count);
                }
            }
            for offset in 1..=PREVIEW_PREFETCH_BACKWARD_PAGES {
                if let Some(index) = center.checked_add(offset) {
                    push_unique_prefetch_index(&mut indices, index, page_count);
                }
            }
        }
    }
    indices
}

fn push_unique_prefetch_index(indices: &mut Vec<usize>, index: usize, page_count: usize) {
    if index < page_count && !indices.contains(&index) {
        indices.push(index);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeBackend {
    ImageCrate,
    JpegScaled,
    BmpSampled,
    GifSampled,
    PngSampled,
    PngExactRows,
    ZuneJpeg,
    PngCrate,
    ZunePng,
    ImageWebp,
    LibWebp,
    LibWebpScaled,
    GifCrate,
    BmpFastPath,
    IcoFastPath,
    LibAvifDav1d,
    ZunePsd,
    PdfiumAi,
}

impl DecodeBackend {
    pub fn is_sampled_or_scaled_prepare(self) -> bool {
        matches!(
            self,
            Self::JpegScaled
                | Self::BmpSampled
                | Self::GifSampled
                | Self::PngSampled
                | Self::LibWebpScaled
        )
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::ImageCrate => "image crate",
            Self::JpegScaled => "JPEG scaled",
            Self::BmpSampled => "BMP sampled",
            Self::GifSampled => "GIF sampled",
            Self::PngSampled => "PNG sampled",
            Self::PngExactRows => "PNG exact rows",
            Self::ZuneJpeg => "zune JPEG",
            Self::PngCrate => "png crate",
            Self::ZunePng => "zune PNG",
            Self::ImageWebp => "image-webp",
            Self::LibWebp => "libwebp",
            Self::LibWebpScaled => "libwebp scaled",
            Self::GifCrate => "gif crate",
            Self::BmpFastPath => "BMP fast path",
            Self::IcoFastPath => "ICO fast path",
            Self::LibAvifDav1d => "libavif dav1d",
            Self::ZunePsd => "zune PSD",
            Self::PdfiumAi => "pdfium AI",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::ImageCrate => "image-crate",
            Self::JpegScaled => "jpeg-scaled",
            Self::BmpSampled => "bmp-sampled",
            Self::GifSampled => "gif-sampled",
            Self::PngSampled => "png-sampled",
            Self::PngExactRows => "png-exact-rows",
            Self::ZuneJpeg => "zune-jpeg",
            Self::PngCrate => "png-crate",
            Self::ZunePng => "zune-png",
            Self::ImageWebp => "image-webp",
            Self::LibWebp => "libwebp",
            Self::LibWebpScaled => "libwebp-scaled",
            Self::GifCrate => "gif-crate",
            Self::BmpFastPath => "bmp-fast",
            Self::IcoFastPath => "ico-fast",
            Self::LibAvifDav1d => "libavif-dav1d",
            Self::ZunePsd => "zune-psd",
            Self::PdfiumAi => "pdfium-ai",
        }
    }
}

/// Retained pixel storage for a prepared page. Grayscale content reported as such by the decoder
/// is kept as 1 byte/px (`Luma`) to quarter its RAM footprint; everything else stays RGBA. VRAM
/// always uses RGBA, so consumers expand to RGBA only transiently at the point of use — the cache
/// never stores the expansion. The [`PreparedPage::rgba`] field was replaced with this enum on
/// purpose so the compiler flags every consumer (a missed one would silently upload a
/// wrong-length buffer and paint a blank page rather than crash).
#[derive(Clone)]
pub enum PagePixels {
    Rgba(Arc<[u8]>),
    Luma(Arc<[u8]>),
}

impl PagePixels {
    pub fn is_luma(&self) -> bool {
        matches!(self, PagePixels::Luma(_))
    }

    /// Bytes actually retained in RAM (RGBA: 4/px, Luma: 1/px).
    pub fn byte_len(&self) -> usize {
        match self {
            PagePixels::Rgba(bytes) | PagePixels::Luma(bytes) => bytes.len(),
        }
    }

    pub fn bytes_per_pixel(&self) -> usize {
        match self {
            PagePixels::Rgba(_) => 4,
            PagePixels::Luma(_) => 1,
        }
    }

    pub fn as_slice(&self) -> &[u8] {
        match self {
            PagePixels::Rgba(bytes) | PagePixels::Luma(bytes) => bytes,
        }
    }

    /// Transiently expand to an RGBA byte buffer sized `width*height*4`. For `Luma`, each byte
    /// becomes an opaque gray triplet. The caller owns the result; nothing is cached.
    pub fn to_rgba_vec(&self, width: usize, height: usize) -> Vec<u8> {
        match self {
            PagePixels::Rgba(bytes) => bytes.to_vec(),
            PagePixels::Luma(bytes) => {
                let mut rgba = Vec::with_capacity(width.saturating_mul(height).saturating_mul(4));
                for &gray in bytes.iter() {
                    rgba.extend_from_slice(&[gray, gray, gray, 255]);
                }
                rgba
            }
        }
    }

    /// Build an egui `ColorImage` of the given `size`. Bit-identical to expanding to RGBA and
    /// calling `ColorImage::from_rgba_unmultiplied`, but the luma branch skips the RGBA Vec.
    pub fn to_color_image(&self, size: [usize; 2]) -> ColorImage {
        match self {
            PagePixels::Rgba(bytes) => ColorImage::from_rgba_unmultiplied(size, bytes),
            PagePixels::Luma(bytes) => {
                let pixels = bytes.iter().map(|&gray| Color32::from_gray(gray)).collect();
                ColorImage::new(size, pixels)
            }
        }
    }
}

#[derive(Clone)]
pub struct PreparedPage {
    pub pixels: PagePixels,
    pub original_width: usize,
    pub original_height: usize,
    pub display_width: usize,
    pub display_height: usize,
    pub byte_size: usize,
    pub target_long_edge: u32,
    pub decode_backend: DecodeBackend,
    pub notice: Option<String>,
}

impl PreparedPage {
    pub fn image_size(&self) -> [usize; 2] {
        [self.display_width, self.display_height]
    }

    pub fn color_image(&self) -> ColorImage {
        // Delegates to `PagePixels::to_color_image`; the luma branch builds gray Color32s directly
        // (no intermediate RGBA Vec) and is bit-identical to `from_rgba_unmultiplied` on an
        // expanded [g,g,g,255] buffer.
        self.pixels.to_color_image(self.image_size())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DecodeStrategy {
    Auto,
    ImageCrate,
}

impl DecodeStrategy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::ImageCrate => "image-crate",
        }
    }

    pub fn parse_cli(value: &str) -> Option<Self> {
        match value {
            "auto" => Some(Self::Auto),
            "image" | "image-crate" | "baseline" => Some(Self::ImageCrate),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DecodeOptions {
    pub strategy: DecodeStrategy,
    pub decoder_preferences: DecoderPreferences,
    pub fast_sampled_scaled_decode: bool,
    pub cpu_upscale_filter: CpuScaleFilter,
    pub cpu_downscale_filter: CpuScaleFilter,
    pub allow_display_upscale: bool,
    pub apply_exif_orientation: bool,
    pub apply_embedded_icc: bool,
}

impl Default for DecodeOptions {
    fn default() -> Self {
        Self {
            strategy: DecodeStrategy::Auto,
            decoder_preferences: DecoderPreferences::default(),
            fast_sampled_scaled_decode: true,
            cpu_upscale_filter: CpuScaleFilter::CatmullRom,
            cpu_downscale_filter: CpuScaleFilter::Hamming,
            allow_display_upscale: false,
            apply_exif_orientation: false,
            apply_embedded_icc: false,
        }
    }
}

impl DecodeOptions {
    pub fn cache_token(self) -> String {
        format!(
            "{}-{}-fastprep-{}-down-{}-{}{}{}",
            self.strategy.as_str(),
            self.decoder_preferences.cache_token(),
            if self.fast_sampled_scaled_decode {
                "on"
            } else {
                "off"
            },
            self.cpu_downscale_filter.token(),
            if self.allow_display_upscale {
                self.cpu_upscale_filter.token()
            } else {
                "no-upscale"
            },
            if self.apply_exif_orientation {
                "-exif"
            } else {
                ""
            },
            if self.apply_embedded_icc { "-icc" } else { "" }
        )
    }

    pub fn scale_filter_for(
        self,
        source_width: u32,
        source_height: u32,
        target_width: u32,
        target_height: u32,
    ) -> CpuScaleFilter {
        if target_width > source_width || target_height > source_height {
            self.cpu_upscale_filter
        } else {
            self.cpu_downscale_filter
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CachedPageKey {
    pub page_id: PageId,
    pub target_long_edge: u32,
    pub decode: DecodeOptions,
}

impl CachedPageKey {
    pub fn new(page_id: PageId, target_long_edge: u32, decode: DecodeOptions) -> Self {
        Self {
            page_id,
            target_long_edge: clamp_target_long_edge(target_long_edge),
            decode,
        }
    }

    pub(in crate::core::worker) fn covers(
        self,
        page_id: PageId,
        target_long_edge: u32,
        decode: DecodeOptions,
    ) -> bool {
        let requested_target = clamp_target_long_edge(target_long_edge);
        if requested_target <= MAX_TARGET_LONG_EDGE && self.target_long_edge > MAX_TARGET_LONG_EDGE
        {
            return false;
        }
        self.page_id == page_id
            && self.decode == decode
            && self.target_long_edge >= requested_target
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerOptions {
    pub decode: DecodeOptions,
    pub target_intent: PreparedTargetIntent,
    pub prefetch_enabled: bool,
    pub progressive_preview_enabled: bool,
    pub cache_bytes: usize,
    pub app_cached_pages: Vec<CachedPageKey>,
}

impl Default for WorkerOptions {
    fn default() -> Self {
        Self {
            decode: DecodeOptions::default(),
            target_intent: PreparedTargetIntent::NormalNavigation,
            prefetch_enabled: true,
            progressive_preview_enabled: true,
            cache_bytes: WORKER_CACHE_BYTES,
            app_cached_pages: Vec::new(),
        }
    }
}

impl WorkerOptions {
    pub(in crate::core::worker) fn normalized(self) -> Self {
        Self {
            cache_bytes: self.cache_bytes.max(8 * 1024 * 1024),
            ..self
        }
    }

    pub(in crate::core::worker) fn app_cache_covers(
        &self,
        page_id: PageId,
        target_long_edge: u32,
    ) -> bool {
        self.app_cached_pages
            .iter()
            .any(|cached| cached.covers(page_id, target_long_edge, self.decode))
    }
}

pub enum WorkerEvent {
    PageReady {
        book_id: String,
        source_instance_id: u64,
        page_id: PageId,
        decode: DecodeOptions,
        page: Arc<PreparedPage>,
    },
    PageFailed {
        book_id: String,
        source_instance_id: u64,
        page_id: PageId,
        target_long_edge: u32,
        decode: DecodeOptions,
        message: String,
    },
}

pub(in crate::core::worker) enum WorkerCommand {
    LoadBook {
        source: SharedSource,
        center: usize,
        direction: NavigationDirection,
        target_long_edge: u32,
        visible_pages: usize,
        options: WorkerOptions,
    },
    SetPage {
        center: usize,
        direction: NavigationDirection,
        target_long_edge: u32,
        visible_pages: usize,
        options: WorkerOptions,
    },
    ClearBook {
        ack: Sender<()>,
    },
    Shutdown,
}
