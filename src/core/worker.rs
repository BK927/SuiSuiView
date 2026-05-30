use crate::core::decoder_backend::{self, DecoderFormat};
use crate::core::formats::unsupported_message_for_bytes;
use crate::core::perf_trace::{self, PerfField};
use crate::core::source::SharedSource;
use crate::core::state::{DecoderPreferences, ResizeFilter};
use crossbeam_channel::{bounded, unbounded, Receiver, Sender};
use eframe::egui::{ColorImage, Context};
use image::{imageops::FilterType, ImageReader, Limits, RgbaImage};
use lru::LruCache;
use std::io::Cursor;
use std::num::NonZeroUsize;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

mod bmp;
#[cfg(test)]
mod decoder_tests;
mod gif;
mod image_crate;
mod jpeg;
mod metadata;
mod png;
mod scheduler;
mod selection;

use image_crate::{prepare_image_with_image_crate, prepare_image_with_image_crate_and_icc};
use metadata::{apply_exif_orientation_to_page, read_image_metadata, ImageMetadata};
use scheduler::{prioritized_jobs, should_skip_ai_preview_or_prefetch};
#[cfg(test)]
use selection::prepare_unavailable_or_image_fallback;

const WORKER_CACHE_BYTES: usize = 48 * 1024 * 1024;
const WORKER_CACHE_ENTRY_LIMIT: usize = 12;
const MAX_IMAGE_DIMENSION: u32 = 20_000;
const MAX_DECODED_PAGE_BYTES: usize = 256 * 1024 * 1024;
const JPEG_SCALED_MIN_RATIO: u32 = 2;
const BMP_SAMPLED_MIN_RATIO: u32 = 2;
const GIF_SAMPLED_MIN_RATIO: u32 = 2;
const PNG_SAMPLED_MIN_RATIO: u32 = 2;
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

#[derive(Clone)]
pub struct PreparedPage {
    pub rgba: Arc<[u8]>,
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
        ColorImage::from_rgba_unmultiplied(self.image_size(), &self.rgba)
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
    pub resize_filter: ResizeFilter,
    pub allow_display_upscale: bool,
    pub apply_exif_orientation: bool,
    pub apply_embedded_icc: bool,
}

impl Default for DecodeOptions {
    fn default() -> Self {
        Self {
            strategy: DecodeStrategy::Auto,
            decoder_preferences: DecoderPreferences::default(),
            resize_filter: ResizeFilter::Bicubic,
            allow_display_upscale: false,
            apply_exif_orientation: false,
            apply_embedded_icc: false,
        }
    }
}

impl DecodeOptions {
    pub fn cache_token(self) -> String {
        format!(
            "{}-{}-{}-{}{}{}",
            self.strategy.as_str(),
            self.decoder_preferences.cache_token(),
            self.resize_filter.token(),
            if self.allow_display_upscale {
                "upscale"
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CachedPageKey {
    pub index: usize,
    pub target_long_edge: u32,
    pub decode: DecodeOptions,
}

impl CachedPageKey {
    pub fn new(index: usize, target_long_edge: u32, decode: DecodeOptions) -> Self {
        Self {
            index,
            target_long_edge: clamp_target_long_edge(target_long_edge),
            decode,
        }
    }

    fn covers(self, index: usize, target_long_edge: u32, decode: DecodeOptions) -> bool {
        let requested_target = clamp_target_long_edge(target_long_edge);
        if requested_target <= MAX_TARGET_LONG_EDGE && self.target_long_edge > MAX_TARGET_LONG_EDGE
        {
            return false;
        }
        self.index == index && self.decode == decode && self.target_long_edge >= requested_target
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerOptions {
    pub decode: DecodeOptions,
    pub prefetch_enabled: bool,
    pub progressive_preview_enabled: bool,
    pub cache_bytes: usize,
    pub app_cached_pages: Vec<CachedPageKey>,
}

impl Default for WorkerOptions {
    fn default() -> Self {
        Self {
            decode: DecodeOptions::default(),
            prefetch_enabled: true,
            progressive_preview_enabled: true,
            cache_bytes: WORKER_CACHE_BYTES,
            app_cached_pages: Vec::new(),
        }
    }
}

impl WorkerOptions {
    fn normalized(self) -> Self {
        Self {
            cache_bytes: self.cache_bytes.max(8 * 1024 * 1024),
            ..self
        }
    }

    fn app_cache_covers(&self, index: usize, target_long_edge: u32) -> bool {
        self.app_cached_pages
            .iter()
            .any(|cached| cached.covers(index, target_long_edge, self.decode))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeBackend {
    ImageCrate,
    JpegScaled,
    BmpSampled,
    GifSampled,
    PngSampled,
    ZuneJpeg,
    PngCrate,
    ZunePng,
    ImageWebp,
    LibWebp,
    GifCrate,
    BmpFastPath,
    IcoFastPath,
    LibAvifDav1d,
    ZunePsd,
    PdfiumAi,
}

impl DecodeBackend {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ImageCrate => "image-crate",
            Self::JpegScaled => "jpeg-scaled",
            Self::BmpSampled => "bmp-sampled",
            Self::GifSampled => "gif-sampled",
            Self::PngSampled => "png-sampled",
            Self::ZuneJpeg => "zune-jpeg",
            Self::PngCrate => "png-crate",
            Self::ZunePng => "zune-png",
            Self::ImageWebp => "image-webp",
            Self::LibWebp => "libwebp",
            Self::GifCrate => "gif-crate",
            Self::BmpFastPath => "bmp-fast",
            Self::IcoFastPath => "ico-fast",
            Self::LibAvifDav1d => "libavif-dav1d",
            Self::ZunePsd => "zune-psd",
            Self::PdfiumAi => "pdfium-ai",
        }
    }
}

pub enum WorkerEvent {
    PageReady {
        book_id: String,
        index: usize,
        decode: DecodeOptions,
        page: Arc<PreparedPage>,
    },
    PageFailed {
        book_id: String,
        index: usize,
        target_long_edge: u32,
        decode: DecodeOptions,
        message: String,
    },
}

enum WorkerCommand {
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

pub struct PageWorker {
    command_tx: Sender<WorkerCommand>,
    event_rx: Receiver<WorkerEvent>,
    shutdown_requested: Arc<AtomicBool>,
    stopped_rx: Receiver<()>,
    join: Option<JoinHandle<()>>,
}

impl PageWorker {
    pub fn new(ctx: Context) -> Self {
        let (command_tx, command_rx) = unbounded();
        let (event_tx, event_rx) = unbounded();
        let (stopped_tx, stopped_rx) = bounded(1);
        let shutdown_requested = Arc::new(AtomicBool::new(false));
        let worker_shutdown_requested = shutdown_requested.clone();
        let join = thread::Builder::new()
            .name("suisuiview-page-worker".to_owned())
            .spawn(move || {
                run_worker(command_rx, event_tx, ctx, worker_shutdown_requested);
                let _ = stopped_tx.send(());
            })
            .expect("page worker thread should start");

        Self {
            command_tx,
            event_rx,
            shutdown_requested,
            stopped_rx,
            join: Some(join),
        }
    }

    pub fn load_book(
        &self,
        source: SharedSource,
        center: usize,
        direction: NavigationDirection,
        target_long_edge: u32,
        visible_pages: usize,
        options: WorkerOptions,
    ) {
        let _ = self.command_tx.send(WorkerCommand::LoadBook {
            source,
            center,
            direction,
            target_long_edge: clamp_target_long_edge(target_long_edge),
            visible_pages: visible_pages.max(1),
            options: options.normalized(),
        });
    }

    pub fn set_page(
        &self,
        center: usize,
        direction: NavigationDirection,
        target_long_edge: u32,
        visible_pages: usize,
        options: WorkerOptions,
    ) {
        let _ = self.command_tx.send(WorkerCommand::SetPage {
            center,
            direction,
            target_long_edge: clamp_target_long_edge(target_long_edge),
            visible_pages: visible_pages.max(1),
            options: options.normalized(),
        });
    }

    pub fn try_recv(&self) -> Option<WorkerEvent> {
        self.event_rx.try_recv().ok()
    }

    pub fn clear_book_blocking(&self) -> bool {
        let (ack, done) = bounded(1);
        if self
            .command_tx
            .send(WorkerCommand::ClearBook { ack })
            .is_err()
        {
            return false;
        }
        done.recv_timeout(Duration::from_millis(1500)).is_ok()
    }

    pub fn request_shutdown(&mut self) -> bool {
        if self.shutdown_requested.swap(true, Ordering::AcqRel) {
            return self.join.is_none();
        }
        let started = Instant::now();
        let sent = self.command_tx.send(WorkerCommand::Shutdown).is_ok();
        let had_thread = self.join.take().is_some();
        let stopped = self
            .stopped_rx
            .recv_timeout(Duration::from_millis(30))
            .is_ok();
        perf_trace::record_duration(
            "shutdown_request",
            started.elapsed(),
            &[
                PerfField::Str("component", "page_worker"),
                PerfField::Bool("command_sent", sent),
                PerfField::Bool("thread_detached", had_thread && !stopped),
                PerfField::Bool("thread_stopped", stopped),
            ],
        );
        stopped
    }
}

impl Drop for PageWorker {
    fn drop(&mut self) {
        self.request_shutdown();
    }
}

#[cfg(test)]
pub fn prepare_image(bytes: &[u8], target_long_edge: u32) -> Result<PreparedPage, String> {
    prepare_image_with_strategy(bytes, target_long_edge, DecodeStrategy::Auto)
}

pub fn prepare_image_with_strategy(
    bytes: &[u8],
    target_long_edge: u32,
    strategy: DecodeStrategy,
) -> Result<PreparedPage, String> {
    prepare_image_with_options(
        bytes,
        target_long_edge,
        DecodeOptions {
            strategy,
            ..DecodeOptions::default()
        },
    )
}

pub fn prepare_image_with_options(
    bytes: &[u8],
    target_long_edge: u32,
    options: DecodeOptions,
) -> Result<PreparedPage, String> {
    if let Some(message) = unsupported_message_for_bytes(bytes) {
        return Err(message.to_owned());
    }

    let detected_format = decoder_backend::detect_format(bytes);
    let metadata = if skips_image_metadata_probe(detected_format) {
        ImageMetadata::default()
    } else {
        read_image_metadata(
            bytes,
            options.apply_embedded_icc,
            options.apply_exif_orientation,
        )
    };
    let icc_profile = metadata.icc_profile.as_ref().ok().cloned().flatten();

    let mut page = if options.apply_embedded_icc && icc_profile.is_some() {
        prepare_image_with_image_crate_and_icc(
            bytes,
            target_long_edge,
            options.resize_filter,
            options.allow_display_upscale,
            icc_profile.as_deref(),
        )?
    } else {
        prepare_image_without_metadata(bytes, target_long_edge, options, detected_format)?
    };

    if let Err(error) = metadata.icc_profile {
        page.notice = Some(format!(
            "ICC profile could not be read; assuming sRGB: {error}"
        ));
    }

    if let Some(orientation) = metadata.orientation {
        page = apply_exif_orientation_to_page(page, orientation);
    }

    Ok(page)
}

fn prepare_image_without_metadata(
    bytes: &[u8],
    target_long_edge: u32,
    options: DecodeOptions,
    detected_format: Option<DecoderFormat>,
) -> Result<PreparedPage, String> {
    match options.strategy {
        DecodeStrategy::Auto => {
            selection::prepare_image_with_selected_decoder(bytes, target_long_edge, options)
        }
        DecodeStrategy::ImageCrate if requires_specialized_decoder(detected_format) => {
            selection::prepare_image_with_selected_decoder(bytes, target_long_edge, options)
        }
        DecodeStrategy::ImageCrate => {
            prepare_image_with_image_crate(bytes, target_long_edge, options)
        }
    }
}

fn requires_specialized_decoder(format: Option<DecoderFormat>) -> bool {
    matches!(format, Some(DecoderFormat::Psd | DecoderFormat::AiPdf))
}

fn skips_image_metadata_probe(format: Option<DecoderFormat>) -> bool {
    matches!(format, Some(DecoderFormat::Psd | DecoderFormat::AiPdf))
}

fn reject_oversized_original(width: u32, height: u32) -> Result<(), String> {
    if width > MAX_IMAGE_DIMENSION || height > MAX_IMAGE_DIMENSION {
        return Err(format!(
            "Image dimensions exceed limit: {}x{}",
            width, height
        ));
    }

    let decoded_bytes = decoded_byte_size(width, height)?;
    if decoded_bytes > MAX_DECODED_PAGE_BYTES {
        return Err(format!(
            "Decoded page is too large: {:.1} MB",
            decoded_bytes as f32 / (1024.0 * 1024.0)
        ));
    }

    Ok(())
}

fn sampled_source_index(out_index: usize, out_len: usize, source_len: usize) -> usize {
    (((out_index * 2 + 1) * source_len) / (out_len * 2)).min(source_len.saturating_sub(1))
}

fn prepared_page_from_rgba(
    raw: Vec<u8>,
    original_width: u32,
    original_height: u32,
    display_width: u32,
    display_height: u32,
    target_long_edge: u32,
    decode_backend: DecodeBackend,
) -> Result<PreparedPage, String> {
    let rgba = Arc::<[u8]>::from(raw.into_boxed_slice());
    let byte_size = prepared_page_byte_size(rgba.len())?;

    Ok(PreparedPage {
        rgba,
        original_width: original_width as usize,
        original_height: original_height as usize,
        display_width: display_width as usize,
        display_height: display_height as usize,
        byte_size,
        target_long_edge,
        decode_backend,
        notice: None,
    })
}

fn image_reader(bytes: &[u8]) -> Result<ImageReader<Cursor<&[u8]>>, String> {
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|error| error.to_string())?;
    reader.limits(decode_limits());
    Ok(reader)
}

fn resize_rgba(
    image: &RgbaImage,
    width: u32,
    height: u32,
    resize_filter: ResizeFilter,
) -> RgbaImage {
    image::imageops::resize(image, width, height, image_filter_type(resize_filter))
}

fn image_filter_type(resize_filter: ResizeFilter) -> FilterType {
    match resize_filter {
        ResizeFilter::Bicubic => FilterType::CatmullRom,
        ResizeFilter::Lanczos3 => FilterType::Lanczos3,
        ResizeFilter::FastTriangle => FilterType::Triangle,
        ResizeFilter::Nearest => FilterType::Nearest,
    }
}

fn decode_limits() -> Limits {
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_DIMENSION);
    limits.max_image_height = Some(MAX_IMAGE_DIMENSION);
    limits.max_alloc = Some(MAX_DECODED_PAGE_BYTES as u64);
    limits
}

pub fn clamp_target_long_edge(target_long_edge: u32) -> u32 {
    target_long_edge.clamp(MIN_TARGET_LONG_EDGE, MAX_ORIGINAL_TARGET_LONG_EDGE)
}

pub fn clamp_navigation_target_long_edge(target_long_edge: u32) -> u32 {
    target_long_edge.clamp(MIN_TARGET_LONG_EDGE, MAX_TARGET_LONG_EDGE)
}

pub fn is_original_inspection_target(target_long_edge: u32) -> bool {
    clamp_target_long_edge(target_long_edge) > MAX_TARGET_LONG_EDGE
}

pub fn display_dimensions(
    width: u32,
    height: u32,
    target_long_edge: u32,
) -> Result<(u32, u32), String> {
    display_dimensions_with_upscale(width, height, target_long_edge, false)
}

pub fn display_dimensions_with_upscale(
    width: u32,
    height: u32,
    target_long_edge: u32,
    allow_upscale: bool,
) -> Result<(u32, u32), String> {
    if width == 0 || height == 0 {
        return Err("Image has zero-sized dimensions".to_owned());
    }

    let target_long_edge = clamp_target_long_edge(target_long_edge);
    let longest = width.max(height);
    if longest <= target_long_edge && !allow_upscale {
        return Ok((width, height));
    }

    let scale = target_long_edge as f64 / longest as f64;
    let display_width = ((width as f64 * scale).round() as u32).max(1);
    let display_height = ((height as f64 * scale).round() as u32).max(1);
    Ok((display_width, display_height))
}

fn run_worker(
    command_rx: Receiver<WorkerCommand>,
    event_tx: Sender<WorkerEvent>,
    ctx: Context,
    shutdown_requested: Arc<AtomicBool>,
) {
    let mut source: Option<SharedSource> = None;
    let mut center = 0usize;
    let mut direction = NavigationDirection::Forward;
    let mut target_long_edge = DEFAULT_TARGET_LONG_EDGE;
    let mut visible_pages = 1usize;
    let mut options = WorkerOptions::default();
    let mut cache: LruCache<String, Arc<PreparedPage>> =
        LruCache::new(NonZeroUsize::new(WORKER_CACHE_ENTRY_LIMIT).unwrap());
    let mut cache_bytes = 0usize;
    let mut book_epoch = 0usize;

    while !shutdown_requested.load(Ordering::Acquire) {
        let Ok(command) = command_rx.recv() else {
            break;
        };
        let previous_book_id = source.as_ref().map(|source| source.book_id().to_owned());
        let previous_decode = options.decode;
        if !apply_command(
            command,
            &mut source,
            &mut center,
            &mut direction,
            &mut target_long_edge,
            &mut visible_pages,
            &mut options,
        ) {
            break;
        }
        update_book_epoch(&mut book_epoch, &source, previous_book_id.as_deref());
        clear_cache_on_book_or_decode_change(
            &source,
            previous_book_id.as_deref(),
            previous_decode,
            options.decode,
            &mut cache,
            &mut cache_bytes,
        );
        prune_worker_cache(&mut cache, &mut cache_bytes, options.cache_bytes);

        'work: loop {
            if shutdown_requested.load(Ordering::Acquire) {
                break;
            }
            let Some(active_source) = source.as_ref().cloned() else {
                break;
            };
            let book_id = active_source.book_id().to_owned();
            let jobs = prioritized_jobs(
                center,
                active_source.page_count(),
                direction,
                target_long_edge,
                visible_pages,
                options.prefetch_enabled,
                options.progressive_preview_enabled,
            );

            for job in jobs {
                if shutdown_requested.load(Ordering::Acquire) {
                    break 'work;
                }
                if should_skip_ai_preview_or_prefetch(
                    active_source.page_name(job.index),
                    center,
                    visible_pages,
                    job.index,
                    job.target_long_edge,
                ) {
                    continue;
                }
                if let Some(command) = drain_latest_command(&command_rx) {
                    let previous_book_id =
                        source.as_ref().map(|source| source.book_id().to_owned());
                    let previous_decode = options.decode;
                    if !apply_command(
                        command,
                        &mut source,
                        &mut center,
                        &mut direction,
                        &mut target_long_edge,
                        &mut visible_pages,
                        &mut options,
                    ) {
                        return;
                    }
                    update_book_epoch(&mut book_epoch, &source, previous_book_id.as_deref());
                    clear_cache_on_book_or_decode_change(
                        &source,
                        previous_book_id.as_deref(),
                        previous_decode,
                        options.decode,
                        &mut cache,
                        &mut cache_bytes,
                    );
                    prune_worker_cache(&mut cache, &mut cache_bytes, options.cache_bytes);
                    continue 'work;
                }

                let key = page_cache_key(&book_id, job.index, job.target_long_edge, options.decode);
                #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
                perf_trace::record_duration(
                    "page_worker_job_start",
                    Duration::ZERO,
                    &[
                        PerfField::Usize("page", job.index),
                        PerfField::Usize("book_epoch", book_epoch),
                        PerfField::U32("target_long_edge", job.target_long_edge),
                    ],
                );
                if let Some(page) = cache.get(&key).cloned() {
                    let _ = event_tx.send(WorkerEvent::PageReady {
                        book_id: book_id.clone(),
                        index: job.index,
                        decode: options.decode,
                        page,
                    });
                    #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
                    perf_trace::record_duration(
                        "page_worker_publish",
                        Duration::ZERO,
                        &[
                            PerfField::Usize("page", job.index),
                            PerfField::Usize("book_epoch", book_epoch),
                            PerfField::U32("target_long_edge", job.target_long_edge),
                            PerfField::Bool("cache_hit", true),
                        ],
                    );
                    #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
                    record_worker_cache_snapshot(
                        "publish_hit",
                        job.index,
                        job.target_long_edge,
                        cache.len(),
                        cache_bytes,
                        options.cache_bytes,
                        true,
                    );
                    ctx.request_repaint();
                    continue;
                }
                if options.app_cache_covers(job.index, job.target_long_edge) {
                    continue;
                }

                #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
                let read_started = Instant::now();
                let read_result = active_source
                    .read_page(job.index)
                    .map_err(|error| error.to_string());
                #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
                perf_trace::record_duration_if_at_least(
                    "page_read",
                    read_started.elapsed(),
                    Duration::from_millis(25),
                    &[
                        PerfField::Usize("page", job.index),
                        PerfField::Usize("book_epoch", book_epoch),
                        PerfField::Bool("success", read_result.is_ok()),
                    ],
                );
                if shutdown_requested.load(Ordering::Acquire) {
                    break 'work;
                }

                let result = read_result.and_then(|bytes| {
                    #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
                    let prepare_started = Instant::now();
                    let prepared =
                        prepare_image_with_options(&bytes, job.target_long_edge, options.decode);
                    #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
                    perf_trace::record_duration_if_at_least(
                        "page_prepare",
                        prepare_started.elapsed(),
                        Duration::from_millis(40),
                        &[
                            PerfField::Usize("page", job.index),
                            PerfField::Usize("book_epoch", book_epoch),
                            PerfField::U32("target_long_edge", job.target_long_edge),
                            PerfField::Str("decode_strategy", options.decode.strategy.as_str()),
                            PerfField::Str("resize_filter", options.decode.resize_filter.token()),
                            PerfField::Bool(
                                "allow_display_upscale",
                                options.decode.allow_display_upscale,
                            ),
                            PerfField::Bool(
                                "apply_exif_orientation",
                                options.decode.apply_exif_orientation,
                            ),
                            PerfField::Bool(
                                "apply_embedded_icc",
                                options.decode.apply_embedded_icc,
                            ),
                            PerfField::Bool("success", prepared.is_ok()),
                        ],
                    );
                    prepared
                });
                if shutdown_requested.load(Ordering::Acquire) {
                    break 'work;
                }

                match result {
                    Ok(page) => {
                        let page = Arc::new(page);

                        insert_worker_cache(&mut cache, &mut cache_bytes, key, page.clone());
                        prune_worker_cache(&mut cache, &mut cache_bytes, options.cache_bytes);
                        let _ = event_tx.send(WorkerEvent::PageReady {
                            book_id: book_id.clone(),
                            index: job.index,
                            decode: options.decode,
                            page,
                        });
                        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
                        perf_trace::record_duration(
                            "page_worker_publish",
                            Duration::ZERO,
                            &[
                                PerfField::Usize("page", job.index),
                                PerfField::Usize("book_epoch", book_epoch),
                                PerfField::U32("target_long_edge", job.target_long_edge),
                                PerfField::Bool("cache_hit", false),
                            ],
                        );
                        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
                        record_worker_cache_snapshot(
                            "publish_miss",
                            job.index,
                            job.target_long_edge,
                            cache.len(),
                            cache_bytes,
                            options.cache_bytes,
                            false,
                        );
                        ctx.request_repaint();

                        if let Some(command) = drain_latest_command(&command_rx) {
                            let previous_book_id =
                                source.as_ref().map(|source| source.book_id().to_owned());
                            let previous_decode = options.decode;
                            if !apply_command(
                                command,
                                &mut source,
                                &mut center,
                                &mut direction,
                                &mut target_long_edge,
                                &mut visible_pages,
                                &mut options,
                            ) {
                                return;
                            }
                            update_book_epoch(
                                &mut book_epoch,
                                &source,
                                previous_book_id.as_deref(),
                            );
                            clear_cache_on_book_or_decode_change(
                                &source,
                                previous_book_id.as_deref(),
                                previous_decode,
                                options.decode,
                                &mut cache,
                                &mut cache_bytes,
                            );
                            prune_worker_cache(&mut cache, &mut cache_bytes, options.cache_bytes);
                            continue 'work;
                        }
                    }
                    Err(message) => {
                        let _ = event_tx.send(WorkerEvent::PageFailed {
                            book_id: book_id.clone(),
                            index: job.index,
                            target_long_edge: job.target_long_edge,
                            decode: options.decode,
                            message,
                        });
                        ctx.request_repaint();
                    }
                }
            }

            break;
        }
    }
}

fn insert_worker_cache(
    cache: &mut LruCache<String, Arc<PreparedPage>>,
    cache_bytes: &mut usize,
    key: String,
    page: Arc<PreparedPage>,
) {
    if let Some((_evicted_key, evicted_page)) = cache.push(key, page.clone()) {
        *cache_bytes = (*cache_bytes).saturating_sub(evicted_page.byte_size);
    }
    *cache_bytes = (*cache_bytes).saturating_add(page.byte_size);
}

fn decoded_byte_size(width: u32, height: u32) -> Result<usize, String> {
    (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| "Decoded image dimensions overflow memory limits".to_owned())
}

fn prepared_page_byte_size(upload_bytes: usize) -> Result<usize, String> {
    let byte_size = upload_bytes;
    if byte_size > MAX_DECODED_PAGE_BYTES {
        return Err(format!(
            "Prepared page is too large: {:.1} MB",
            byte_size as f32 / (1024.0 * 1024.0)
        ));
    }
    Ok(byte_size)
}

fn retained_page_byte_size(upload_bytes: usize) -> usize {
    upload_bytes
}

fn clear_cache_on_book_or_decode_change(
    source: &Option<SharedSource>,
    previous_book_id: Option<&str>,
    previous_decode: DecodeOptions,
    current_decode: DecodeOptions,
    cache: &mut LruCache<String, Arc<PreparedPage>>,
    cache_bytes: &mut usize,
) {
    let current_book_id = source.as_ref().map(|source| source.book_id());
    if previous_book_id != current_book_id || previous_decode != current_decode {
        cache.clear();
        *cache_bytes = 0;
    }
}

fn update_book_epoch(
    book_epoch: &mut usize,
    source: &Option<SharedSource>,
    previous_book_id: Option<&str>,
) {
    let current_book_id = source.as_ref().map(|source| source.book_id());
    if current_book_id.is_some() && previous_book_id != current_book_id {
        *book_epoch = book_epoch.saturating_add(1);
    }
}

fn prune_worker_cache(
    cache: &mut LruCache<String, Arc<PreparedPage>>,
    cache_bytes: &mut usize,
    budget_bytes: usize,
) {
    while *cache_bytes > budget_bytes {
        let Some((_key, page)) = cache.pop_lru() else {
            break;
        };
        *cache_bytes = (*cache_bytes).saturating_sub(page.byte_size);
    }
}

#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
fn record_worker_cache_snapshot(
    reason: &'static str,
    page: usize,
    target_long_edge: u32,
    cache_pages: usize,
    cache_bytes: usize,
    cache_budget_bytes: usize,
    cache_hit: bool,
) {
    perf_trace::record_duration(
        "page_worker_cache_snapshot",
        Duration::ZERO,
        &[
            PerfField::Str("reason", reason),
            PerfField::Usize("page", page),
            PerfField::U32("target_long_edge", target_long_edge),
            PerfField::Usize("cache_pages", cache_pages),
            PerfField::Usize("cache_bytes", cache_bytes),
            PerfField::Usize("cache_budget_bytes", cache_budget_bytes),
            PerfField::Bool("cache_hit", cache_hit),
        ],
    );
}

fn apply_command(
    command: WorkerCommand,
    source: &mut Option<SharedSource>,
    center: &mut usize,
    direction: &mut NavigationDirection,
    target_long_edge: &mut u32,
    visible_pages: &mut usize,
    options: &mut WorkerOptions,
) -> bool {
    match command {
        WorkerCommand::LoadBook {
            source: new_source,
            center: new_center,
            direction: new_direction,
            target_long_edge: new_target_long_edge,
            visible_pages: new_visible_pages,
            options: new_options,
        } => {
            *source = Some(new_source);
            *center = new_center;
            *direction = new_direction;
            *target_long_edge = new_target_long_edge;
            *visible_pages = new_visible_pages.max(1);
            *options = new_options.normalized();
            true
        }
        WorkerCommand::SetPage {
            center: new_center,
            direction: new_direction,
            target_long_edge: new_target_long_edge,
            visible_pages: new_visible_pages,
            options: new_options,
        } => {
            *center = new_center;
            *direction = new_direction;
            *target_long_edge = new_target_long_edge;
            *visible_pages = new_visible_pages.max(1);
            *options = new_options.normalized();
            true
        }
        WorkerCommand::ClearBook { ack } => {
            *source = None;
            *center = 0;
            *direction = NavigationDirection::Forward;
            *target_long_edge = DEFAULT_TARGET_LONG_EDGE;
            *visible_pages = 1;
            *options = WorkerOptions::default();
            let _ = ack.send(());
            true
        }
        WorkerCommand::Shutdown => false,
    }
}

fn drain_latest_command(command_rx: &Receiver<WorkerCommand>) -> Option<WorkerCommand> {
    let mut latest = None;
    while let Ok(command) = command_rx.try_recv() {
        latest = Some(command);
    }
    latest
}

fn page_cache_key(
    book_id: &str,
    index: usize,
    target_long_edge: u32,
    decode: DecodeOptions,
) -> String {
    format!(
        "{book_id}:{index}:{}:{}",
        clamp_target_long_edge(target_long_edge),
        decode.cache_token()
    )
}

#[cfg(test)]
mod tests {
    use super::{
        clamp_navigation_target_long_edge, clamp_target_long_edge, display_dimensions,
        display_dimensions_with_upscale, image_filter_type, page_cache_key, prepare_image,
        prepare_image_with_options, prepare_image_with_strategy,
        prepare_unavailable_or_image_fallback, run_worker, CachedPageKey, DecodeBackend,
        DecodeOptions, DecodeStrategy, NavigationDirection, WorkerCommand, WorkerEvent,
        WorkerOptions, MAX_ORIGINAL_TARGET_LONG_EDGE, MAX_TARGET_LONG_EDGE,
    };
    use crate::core::source::{BookSource, SharedSource, SourceError};
    use crate::core::state::{DecoderPreference, DecoderPreferences, ResizeFilter};
    use crossbeam_channel::unbounded;
    use image::{DynamicImage, ImageFormat, Rgb, RgbImage};
    use std::io::Cursor;
    use std::path::{Path, PathBuf};
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    use std::thread;
    use std::time::Duration;

    #[test]
    fn damaged_image_returns_error_instead_of_panicking() {
        assert!(prepare_image(b"not-an-image", 2048).is_err());
    }

    #[test]
    fn supported_formats_decode_to_prepared_pages() {
        for format in [
            ImageFormat::Jpeg,
            ImageFormat::Png,
            ImageFormat::WebP,
            ImageFormat::Bmp,
            ImageFormat::Gif,
        ] {
            let bytes = encoded_test_image(format);
            let page = prepare_image(&bytes, 1024).unwrap();
            assert_eq!(page.original_width, 48);
            assert_eq!(page.original_height, 32);
            assert_eq!(page.display_width, 48);
            assert_eq!(page.display_height, 32);
        }
    }

    #[test]
    fn prepared_page_retains_single_rgba_buffer_budget() {
        let bytes = encoded_test_image(ImageFormat::Png);
        let page = prepare_image(&bytes, 1024).unwrap();

        assert_eq!(page.image_size(), [48, 32]);
        assert_eq!(page.rgba.len(), 48 * 32 * 4);
        assert_eq!(page.byte_size, page.rgba.len());
        assert_eq!(page.color_image().size, [48, 32]);
    }

    #[test]
    fn auto_strategy_uses_scaled_decode_for_large_jpegs() {
        let bytes = encoded_sized_test_image(ImageFormat::Jpeg, 2304, 1536);
        let page = prepare_image_with_strategy(&bytes, 1024, DecodeStrategy::Auto).unwrap();

        assert_eq!(page.decode_backend, DecodeBackend::JpegScaled);
        assert_eq!(page.original_width, 2304);
        assert_eq!(page.original_height, 1536);
        assert_eq!(page.display_width, 1024);
        assert_eq!(page.display_height, 683);
    }

    #[test]
    fn image_crate_strategy_keeps_baseline_decode_for_large_jpegs() {
        let bytes = encoded_sized_test_image(ImageFormat::Jpeg, 2304, 1536);
        let page = prepare_image_with_strategy(&bytes, 1024, DecodeStrategy::ImageCrate).unwrap();

        assert_eq!(page.decode_backend, DecodeBackend::ImageCrate);
        assert_eq!(page.display_width, 1024);
        assert_eq!(page.display_height, 683);
    }

    #[test]
    fn auto_strategy_default_preferences_resolve_to_benchmark_winners() {
        let jpeg = encoded_test_image(ImageFormat::Jpeg);
        let jpeg_page = prepare_image_with_options(&jpeg, 1024, DecodeOptions::default()).unwrap();
        assert_eq!(jpeg_page.decode_backend, DecodeBackend::ZuneJpeg);

        let png = encoded_test_image(ImageFormat::Png);
        let png_page = prepare_image_with_options(&png, 1024, DecodeOptions::default()).unwrap();
        assert_eq!(png_page.decode_backend, DecodeBackend::PngCrate);

        let gif = encoded_test_image(ImageFormat::Gif);
        let gif_page = prepare_image_with_options(&gif, 1024, DecodeOptions::default()).unwrap();
        assert_eq!(gif_page.decode_backend, DecodeBackend::GifCrate);

        let bmp = encoded_test_image(ImageFormat::Bmp);
        let bmp_page = prepare_image_with_options(&bmp, 1024, DecodeOptions::default()).unwrap();
        assert_eq!(bmp_page.decode_backend, DecodeBackend::BmpFastPath);
    }

    #[test]
    fn image_crate_strategy_ignores_format_preferences() {
        let bytes = encoded_test_image(ImageFormat::Jpeg);
        let page = prepare_image_with_options(
            &bytes,
            1024,
            DecodeOptions {
                strategy: DecodeStrategy::ImageCrate,
                decoder_preferences: DecoderPreferences {
                    jpeg: DecoderPreference::ZuneJpeg,
                    ..DecoderPreferences::default()
                },
                ..DecodeOptions::default()
            },
        )
        .unwrap();

        assert_eq!(page.decode_backend, DecodeBackend::ImageCrate);
    }

    #[test]
    fn unavailable_selected_backend_falls_back_with_notice() {
        let bytes = encoded_test_image(ImageFormat::Png);
        let page = prepare_unavailable_or_image_fallback(
            &bytes,
            1024,
            DecodeOptions::default(),
            DecodeBackend::LibWebp,
            "backend not enabled",
        )
        .unwrap();

        assert_eq!(page.decode_backend, DecodeBackend::ImageCrate);
        let notice = page.notice.as_deref().unwrap_or_default();
        assert!(notice.contains("libwebp"));
        assert!(notice.contains("used image fallback"));
    }

    #[test]
    fn auto_strategy_samples_large_uncompressed_bmps() {
        let bytes = encoded_sized_test_image(ImageFormat::Bmp, 2048, 16);
        let page = prepare_image_with_strategy(&bytes, 1024, DecodeStrategy::Auto).unwrap();

        assert_eq!(page.decode_backend, DecodeBackend::BmpSampled);
        assert_eq!(page.original_width, 2048);
        assert_eq!(page.original_height, 16);
        assert_eq!(page.display_width, 1024);
        assert_eq!(page.display_height, 8);
    }

    #[test]
    fn image_crate_strategy_keeps_baseline_decode_for_large_bmps() {
        let bytes = encoded_sized_test_image(ImageFormat::Bmp, 2048, 16);
        let page = prepare_image_with_strategy(&bytes, 1024, DecodeStrategy::ImageCrate).unwrap();

        assert_eq!(page.decode_backend, DecodeBackend::ImageCrate);
        assert_eq!(page.display_width, 1024);
        assert_eq!(page.display_height, 8);
    }

    #[test]
    fn auto_strategy_samples_large_static_gifs() {
        let bytes = encoded_sized_test_image(ImageFormat::Gif, 2048, 16);
        let page = prepare_image_with_strategy(&bytes, 1024, DecodeStrategy::Auto).unwrap();

        assert_eq!(page.decode_backend, DecodeBackend::GifSampled);
        assert_eq!(page.original_width, 2048);
        assert_eq!(page.original_height, 16);
        assert_eq!(page.display_width, 1024);
        assert_eq!(page.display_height, 8);
    }

    #[test]
    fn image_crate_strategy_keeps_baseline_decode_for_large_gifs() {
        let bytes = encoded_sized_test_image(ImageFormat::Gif, 2048, 16);
        let page = prepare_image_with_strategy(&bytes, 1024, DecodeStrategy::ImageCrate).unwrap();

        assert_eq!(page.decode_backend, DecodeBackend::ImageCrate);
        assert_eq!(page.display_width, 1024);
        assert_eq!(page.display_height, 8);
    }

    #[test]
    fn auto_strategy_samples_large_pngs() {
        let bytes = encoded_sized_test_image(ImageFormat::Png, 2048, 16);
        let page = prepare_image_with_strategy(&bytes, 1024, DecodeStrategy::Auto).unwrap();

        assert_eq!(page.decode_backend, DecodeBackend::PngSampled);
        assert_eq!(page.original_width, 2048);
        assert_eq!(page.original_height, 16);
        assert_eq!(page.display_width, 1024);
        assert_eq!(page.display_height, 8);
    }

    #[test]
    fn image_crate_strategy_keeps_baseline_decode_for_large_pngs() {
        let bytes = encoded_sized_test_image(ImageFormat::Png, 2048, 16);
        let page = prepare_image_with_strategy(&bytes, 1024, DecodeStrategy::ImageCrate).unwrap();

        assert_eq!(page.decode_backend, DecodeBackend::ImageCrate);
        assert_eq!(page.display_width, 1024);
        assert_eq!(page.display_height, 8);
    }

    #[test]
    fn display_dimensions_preserve_ratio_and_do_not_upscale() {
        assert_eq!(display_dimensions(800, 600, 2048).unwrap(), (800, 600));
        assert_eq!(display_dimensions(8000, 4000, 2000).unwrap(), (2000, 1000));
        assert_eq!(
            display_dimensions(3000, 9000, MAX_TARGET_LONG_EDGE + 500).unwrap(),
            (1532, 4596)
        );
        assert_eq!(
            clamp_target_long_edge(MAX_ORIGINAL_TARGET_LONG_EDGE + 500),
            MAX_ORIGINAL_TARGET_LONG_EDGE
        );
    }

    #[test]
    fn navigation_target_clamp_keeps_display_path_capped() {
        assert_eq!(
            clamp_navigation_target_long_edge(MAX_TARGET_LONG_EDGE + 500),
            MAX_TARGET_LONG_EDGE
        );
    }

    #[test]
    fn display_dimensions_can_upscale_for_fit_modes() {
        assert_eq!(
            display_dimensions_with_upscale(640, 320, 2048, true).unwrap(),
            (2048, 1024)
        );
        assert_eq!(
            display_dimensions_with_upscale(640, 320, 2048, false).unwrap(),
            (640, 320)
        );
    }

    #[test]
    fn resize_filters_map_to_image_filters() {
        assert_eq!(
            image_filter_type(ResizeFilter::Bicubic),
            image::imageops::FilterType::CatmullRom
        );
        assert_eq!(
            image_filter_type(ResizeFilter::Lanczos3),
            image::imageops::FilterType::Lanczos3
        );
        assert_eq!(
            image_filter_type(ResizeFilter::FastTriangle),
            image::imageops::FilterType::Triangle
        );
        assert_eq!(
            image_filter_type(ResizeFilter::Nearest),
            image::imageops::FilterType::Nearest
        );
    }

    #[test]
    fn worker_publishes_completed_page_before_handling_queued_command() {
        let (command_tx, command_rx) = unbounded();
        let (event_tx, event_rx) = unbounded();
        let shutdown_requested = Arc::new(AtomicBool::new(false));
        let worker_shutdown = shutdown_requested.clone();
        let source: SharedSource = Arc::new(CommandingSource {
            command_tx: command_tx.clone(),
            sent_command: AtomicBool::new(false),
            page_bytes: encoded_test_image(ImageFormat::Png),
            path: PathBuf::from("commanding-source"),
        });
        let handle = thread::spawn(move || {
            run_worker(
                command_rx,
                event_tx,
                eframe::egui::Context::default(),
                worker_shutdown,
            );
        });
        command_tx
            .send(WorkerCommand::LoadBook {
                source,
                center: 0,
                direction: NavigationDirection::Forward,
                target_long_edge: 2048,
                visible_pages: 1,
                options: WorkerOptions::default(),
            })
            .unwrap();
        let first_event = event_rx.recv_timeout(Duration::from_secs(2)).unwrap();

        match first_event {
            WorkerEvent::PageReady { index, .. } => assert_eq!(index, 0),
            WorkerEvent::PageFailed { message, .. } => panic!("page failed: {message}"),
        }

        shutdown_requested.store(true, Ordering::Release);
        let _ = command_tx.send(WorkerCommand::Shutdown);
        handle.join().unwrap();
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        assert!(crate::core::perf_trace::flush_timeout(Duration::from_secs(
            1
        )));
    }

    #[test]
    fn cached_page_key_covers_same_page_decode_and_sufficient_size() {
        let decode = DecodeOptions::default();
        let key = CachedPageKey::new(3, 2048, decode);

        assert!(key.covers(3, 1024, decode));
        assert!(key.covers(3, 2048, decode));
        assert!(!key.covers(3, 4096, decode));
        assert!(!key.covers(4, 1024, decode));
        assert!(!key.covers(
            3,
            1024,
            DecodeOptions {
                apply_embedded_icc: true,
                ..decode
            }
        ));

        let original_key = CachedPageKey::new(3, MAX_TARGET_LONG_EDGE + 1, decode);
        assert!(original_key.covers(3, MAX_TARGET_LONG_EDGE + 1, decode));
        assert!(!original_key.covers(3, MAX_TARGET_LONG_EDGE, decode));
    }

    #[test]
    fn worker_cache_key_tracks_decode_options() {
        let normal = page_cache_key("book", 1, 2048, DecodeOptions::default());
        let exif = page_cache_key(
            "book",
            1,
            2048,
            DecodeOptions {
                apply_exif_orientation: true,
                ..DecodeOptions::default()
            },
        );
        let icc = page_cache_key(
            "book",
            1,
            2048,
            DecodeOptions {
                apply_embedded_icc: true,
                ..DecodeOptions::default()
            },
        );
        let lanczos = page_cache_key(
            "book",
            1,
            2048,
            DecodeOptions {
                resize_filter: ResizeFilter::Lanczos3,
                ..DecodeOptions::default()
            },
        );
        let upscaled = page_cache_key(
            "book",
            1,
            2048,
            DecodeOptions {
                allow_display_upscale: true,
                ..DecodeOptions::default()
            },
        );

        assert_ne!(normal, exif);
        assert_ne!(normal, icc);
        assert_ne!(normal, lanczos);
        assert_ne!(normal, upscaled);

        let zune_jpeg = page_cache_key(
            "book",
            1,
            2048,
            DecodeOptions {
                decoder_preferences: DecoderPreferences {
                    jpeg: DecoderPreference::ZuneJpeg,
                    ..DecoderPreferences::default()
                },
                ..DecodeOptions::default()
            },
        );
        assert_ne!(normal, zune_jpeg);
    }

    fn encoded_test_image(format: ImageFormat) -> Vec<u8> {
        encoded_sized_test_image(format, 48, 32)
    }

    fn encoded_sized_test_image(format: ImageFormat, width: u32, height: u32) -> Vec<u8> {
        let image = RgbImage::from_fn(width, height, |x, y| {
            Rgb([
                ((x * 3 + y) % 255) as u8,
                ((x + y * 5) % 255) as u8,
                ((x * 7 + y * 11) % 255) as u8,
            ])
        });
        let mut bytes = Vec::new();
        DynamicImage::ImageRgb8(image)
            .write_to(&mut Cursor::new(&mut bytes), format)
            .unwrap();
        bytes
    }

    struct CommandingSource {
        command_tx: crossbeam_channel::Sender<WorkerCommand>,
        sent_command: AtomicBool,
        page_bytes: Vec<u8>,
        path: PathBuf,
    }

    impl BookSource for CommandingSource {
        fn title(&self) -> &str {
            "commanding"
        }

        fn source_path(&self) -> &Path {
            &self.path
        }

        fn book_id(&self) -> &str {
            "commanding"
        }

        fn page_count(&self) -> usize {
            2
        }

        fn page_name(&self, index: usize) -> Option<&str> {
            match index {
                0 => Some("page-0000.png"),
                1 => Some("page-0001.png"),
                _ => None,
            }
        }

        fn read_page(&self, index: usize) -> Result<Vec<u8>, SourceError> {
            if index == 0 && !self.sent_command.swap(true, Ordering::AcqRel) {
                self.command_tx
                    .send(WorkerCommand::SetPage {
                        center: 1,
                        direction: NavigationDirection::Forward,
                        target_long_edge: 2048,
                        visible_pages: 1,
                        options: WorkerOptions::default(),
                    })
                    .unwrap();
            }
            Ok(self.page_bytes.clone())
        }
    }
}
