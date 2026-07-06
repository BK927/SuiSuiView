use super::image_crate::{prepare_image_with_image_crate, prepare_image_with_image_crate_and_icc};
use super::metadata::{apply_exif_orientation_to_page, read_image_metadata, ImageMetadata};
use super::scheduler::PageJob;
use super::selection;
use super::{
    DecodeBackend, DecodeOptions, DecodeStrategy, PagePixels, PreparedPage, MAX_DECODED_PAGE_BYTES,
    MAX_IMAGE_DIMENSION, MAX_ORIGINAL_TARGET_LONG_EDGE, MAX_TARGET_LONG_EDGE, MIN_TARGET_LONG_EDGE,
};
use crate::core::decoder_backend::{self, DecoderFormat};
use crate::core::formats::unsupported_message_for_bytes;
#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
use crate::core::{perf_trace, perf_trace::PerfField};
use image::{ImageReader, Limits};
use std::io::Cursor;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
pub(in crate::core::worker) fn record_prepare_stage(
    stage: &'static str,
    backend: DecodeBackend,
    duration: Duration,
    target_long_edge: u32,
    source_bytes: usize,
    original_width: u32,
    original_height: u32,
    display_width: u32,
    display_height: u32,
) {
    perf_trace::record_duration_if_at_least(
        "page_prepare_stage",
        duration,
        Duration::from_millis(1),
        &[
            PerfField::Str("stage", stage),
            PerfField::Str("backend", backend.as_str()),
            PerfField::U32("target_long_edge", target_long_edge),
            PerfField::Usize("source_bytes", source_bytes),
            PerfField::U32("original_width", original_width),
            PerfField::U32("original_height", original_height),
            PerfField::U32("display_width", display_width),
            PerfField::U32("display_height", display_height),
        ],
    );
}

pub(in crate::core::worker) struct PreparedPageWithTiming {
    pub page: PreparedPage,
    pub prepare_duration: Option<Duration>,
}

pub(in crate::core::worker) fn prepare_page_with_perf(
    bytes: &[u8],
    job: PageJob,
    book_epoch: usize,
    decode: DecodeOptions,
    decode_ahead: bool,
    measure_prepare_timing: bool,
) -> Result<PreparedPageWithTiming, String> {
    #[cfg(not(any(feature = "perf-dev", feature = "perf-diagnostics")))]
    let _ = (book_epoch, decode_ahead);
    #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
    let _ = measure_prepare_timing;
    #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
    let prepare_started = Instant::now();
    #[cfg(not(any(feature = "perf-dev", feature = "perf-diagnostics")))]
    let prepare_started = measure_prepare_timing.then(Instant::now);
    let prepared = prepare_image_with_options(bytes, job.target_long_edge, decode);
    #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
    let prepare_duration = Some(prepare_started.elapsed());
    #[cfg(not(any(feature = "perf-dev", feature = "perf-diagnostics")))]
    let prepare_duration = prepare_started.map(|started| started.elapsed());
    #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
    perf_trace::record_duration_if_at_least(
        "page_prepare",
        prepare_duration.expect("diagnostic prepare timing should be recorded"),
        Duration::from_millis(40),
        &[
            PerfField::Usize("page", job.index),
            PerfField::Usize("book_epoch", book_epoch),
            PerfField::U32("target_long_edge", job.target_long_edge),
            PerfField::Str("decode_strategy", decode.strategy.as_str()),
            PerfField::Bool(
                "fast_sampled_scaled_decode",
                decode.fast_sampled_scaled_decode,
            ),
            PerfField::Str("cpu_upscale_filter", decode.cpu_upscale_filter.token()),
            PerfField::Str("cpu_downscale_filter", decode.cpu_downscale_filter.token()),
            PerfField::Bool("allow_display_upscale", decode.allow_display_upscale),
            PerfField::Bool("apply_exif_orientation", decode.apply_exif_orientation),
            PerfField::Bool("apply_embedded_icc", decode.apply_embedded_icc),
            PerfField::Bool("decode_ahead", decode_ahead),
            PerfField::Bool("success", prepared.is_ok()),
        ],
    );
    prepared.map(|page| PreparedPageWithTiming {
        page,
        prepare_duration,
    })
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
            options,
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

pub(in crate::core::worker) fn reject_oversized_original(
    width: u32,
    height: u32,
) -> Result<(), String> {
    reject_oversized_dimensions(width, height)?;

    let decoded_bytes = decoded_byte_size(width, height)?;
    if decoded_bytes > MAX_DECODED_PAGE_BYTES {
        return Err(format!(
            "Decoded page is too large: {:.1} MB",
            decoded_bytes as f32 / (1024.0 * 1024.0)
        ));
    }

    Ok(())
}

pub(in crate::core::worker) fn reject_oversized_dimensions(
    width: u32,
    height: u32,
) -> Result<(), String> {
    if width <= MAX_IMAGE_DIMENSION && height <= MAX_IMAGE_DIMENSION {
        return Ok(());
    }

    Err(format!("Image dimensions exceed limit: {width}x{height}"))
}

fn sampled_source_index(out_index: usize, out_len: usize, source_len: usize) -> usize {
    (((out_index * 2 + 1) * source_len) / (out_len * 2)).min(source_len.saturating_sub(1))
}

pub(in crate::core::worker) fn sampled_index_map(out_len: usize, source_len: usize) -> Vec<usize> {
    (0..out_len)
        .map(|out_index| sampled_source_index(out_index, out_len, source_len))
        .collect()
}

pub(in crate::core::worker) fn prepared_page_from_rgba(
    raw: Vec<u8>,
    original_width: u32,
    original_height: u32,
    display_width: u32,
    display_height: u32,
    target_long_edge: u32,
    decode_backend: DecodeBackend,
) -> Result<PreparedPage, String> {
    let rgba = Arc::<[u8]>::from(raw.into_boxed_slice());
    prepared_page_from_pixels(
        PagePixels::Rgba(rgba),
        original_width,
        original_height,
        display_width,
        display_height,
        target_long_edge,
        decode_backend,
    )
}

/// Build a `PreparedPage` from single-channel gray bytes retained as `PagePixels::Luma` (1 byte/px).
/// Callers must only reach this for content the decoder reported as grayscale with no separate
/// alpha and no color-management transform in play; VRAM still uploads RGBA via a transient expand.
pub(in crate::core::worker) fn prepared_page_from_luma(
    raw: Vec<u8>,
    original_width: u32,
    original_height: u32,
    display_width: u32,
    display_height: u32,
    target_long_edge: u32,
    decode_backend: DecodeBackend,
) -> Result<PreparedPage, String> {
    let luma = Arc::<[u8]>::from(raw.into_boxed_slice());
    prepared_page_from_pixels(
        PagePixels::Luma(luma),
        original_width,
        original_height,
        display_width,
        display_height,
        target_long_edge,
        decode_backend,
    )
}

fn prepared_page_from_pixels(
    pixels: PagePixels,
    original_width: u32,
    original_height: u32,
    display_width: u32,
    display_height: u32,
    target_long_edge: u32,
    decode_backend: DecodeBackend,
) -> Result<PreparedPage, String> {
    // Budget accounting tracks bytes actually retained, so a luma page counts a quarter of an RGBA
    // page of the same dimensions. The oversized-page guard uses the expanded RGBA footprint so the
    // cap still reflects the memory a consumer will transiently allocate on upload.
    let byte_size = pixels.byte_len();
    let expanded_rgba_bytes = display_width
        .checked_mul(display_height)
        .and_then(|pixels| pixels.checked_mul(4))
        .map(|bytes| bytes as usize)
        .unwrap_or(usize::MAX)
        .max(byte_size);
    prepared_page_byte_size(expanded_rgba_bytes)?;

    Ok(PreparedPage {
        pixels,
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

pub(in crate::core::worker) fn image_reader(
    bytes: &[u8],
) -> Result<ImageReader<Cursor<&[u8]>>, String> {
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|error| error.to_string())?;
    reader.limits(decode_limits());
    Ok(reader)
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

pub(in crate::core::worker) fn decoded_byte_size(width: u32, height: u32) -> Result<usize, String> {
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

pub(in crate::core::worker) fn retained_page_byte_size(upload_bytes: usize) -> usize {
    upload_bytes
}
