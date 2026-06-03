use super::{
    clamp_target_long_edge, display_dimensions_with_upscale, prepared_page_from_rgba,
    reject_oversized_dimensions, resize_rgba, DecodeBackend, DecodeOptions, PreparedPage,
    JPEG_SCALED_MIN_RATIO,
};
use image::RgbaImage;
use jpeg_decoder::{Decoder as JpegDecoder, PixelFormat};
use std::io::Cursor;
#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
use std::time::Instant;

pub(super) fn prepare_image_with_scaled_jpeg(
    bytes: &[u8],
    target_long_edge: u32,
    options: DecodeOptions,
) -> Result<Option<PreparedPage>, String> {
    if !is_jpeg(bytes) {
        return Ok(None);
    }

    let target_long_edge = clamp_target_long_edge(target_long_edge);
    #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
    let header_started = Instant::now();
    let mut decoder = JpegDecoder::new(Cursor::new(bytes));
    decoder.read_info().map_err(|error| error.to_string())?;
    let info = decoder
        .info()
        .ok_or_else(|| "JPEG header did not include dimensions".to_owned())?;
    let original_width = u32::from(info.width);
    let original_height = u32::from(info.height);
    #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
    super::record_prepare_stage(
        "jpeg_scaled_header",
        DecodeBackend::JpegScaled,
        header_started.elapsed(),
        target_long_edge,
        bytes.len(),
        original_width,
        original_height,
        original_width,
        original_height,
    );
    reject_oversized_dimensions(original_width, original_height)?;

    let (display_width, display_height) = display_dimensions_with_upscale(
        original_width,
        original_height,
        target_long_edge,
        options.allow_display_upscale,
    )?;
    if original_width.max(original_height) < target_long_edge * JPEG_SCALED_MIN_RATIO {
        return Ok(None);
    }

    match info.pixel_format {
        PixelFormat::L8 | PixelFormat::RGB24 => {}
        PixelFormat::CMYK32 | PixelFormat::L16 => return Ok(None),
    }

    #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
    let scale_started = Instant::now();
    decoder
        .scale(display_width as u16, display_height as u16)
        .map_err(|error| error.to_string())?;
    #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
    super::record_prepare_stage(
        "jpeg_scaled_choose_scale",
        DecodeBackend::JpegScaled,
        scale_started.elapsed(),
        target_long_edge,
        bytes.len(),
        original_width,
        original_height,
        display_width,
        display_height,
    );

    #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
    let decode_started = Instant::now();
    let pixels = decoder.decode().map_err(|error| error.to_string())?;
    let scaled_info = decoder
        .info()
        .ok_or_else(|| "JPEG decoder did not report scaled dimensions".to_owned())?;
    let scaled_width = u32::from(scaled_info.width);
    let scaled_height = u32::from(scaled_info.height);
    #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
    super::record_prepare_stage(
        "jpeg_scaled_decode",
        DecodeBackend::JpegScaled,
        decode_started.elapsed(),
        target_long_edge,
        bytes.len(),
        original_width,
        original_height,
        scaled_width,
        scaled_height,
    );

    #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
    let rgba_started = Instant::now();
    let rgba = jpeg_pixels_to_rgba(
        &pixels,
        scaled_info.pixel_format,
        scaled_width,
        scaled_height,
    )?;
    #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
    super::record_prepare_stage(
        "jpeg_scaled_rgba_expand",
        DecodeBackend::JpegScaled,
        rgba_started.elapsed(),
        target_long_edge,
        bytes.len(),
        scaled_width,
        scaled_height,
        scaled_width,
        scaled_height,
    );
    let display = if scaled_width == display_width && scaled_height == display_height {
        rgba
    } else {
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        let resize_started = Instant::now();
        let resize_filter =
            options.scale_filter_for(scaled_width, scaled_height, display_width, display_height);
        let display = resize_rgba(&rgba, display_width, display_height, resize_filter);
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        super::record_prepare_stage(
            "jpeg_scaled_resize",
            DecodeBackend::JpegScaled,
            resize_started.elapsed(),
            target_long_edge,
            bytes.len(),
            scaled_width,
            scaled_height,
            display_width,
            display_height,
        );
        display
    };

    prepared_page_from_rgba(
        display.into_raw(),
        original_width,
        original_height,
        display_width,
        display_height,
        target_long_edge,
        DecodeBackend::JpegScaled,
    )
    .map(Some)
}

pub(super) fn is_jpeg(bytes: &[u8]) -> bool {
    bytes.len() >= 3 && bytes[0] == 0xff && bytes[1] == 0xd8 && bytes[2] == 0xff
}

fn jpeg_pixels_to_rgba(
    pixels: &[u8],
    format: PixelFormat,
    width: u32,
    height: u32,
) -> Result<RgbaImage, String> {
    let pixel_count = (width as usize)
        .checked_mul(height as usize)
        .ok_or_else(|| "JPEG dimensions overflow memory limits".to_owned())?;
    let expected = match format {
        PixelFormat::L8 => pixel_count,
        PixelFormat::RGB24 => pixel_count
            .checked_mul(3)
            .ok_or_else(|| "JPEG RGB buffer size overflows memory limits".to_owned())?,
        PixelFormat::CMYK32 => pixel_count
            .checked_mul(4)
            .ok_or_else(|| "JPEG CMYK buffer size overflows memory limits".to_owned())?,
        PixelFormat::L16 => pixel_count
            .checked_mul(2)
            .ok_or_else(|| "JPEG 16-bit buffer size overflows memory limits".to_owned())?,
    };
    if pixels.len() != expected {
        return Err("JPEG decoder returned an unexpected buffer size".to_owned());
    }

    let mut rgba = Vec::with_capacity(
        pixel_count
            .checked_mul(4)
            .ok_or_else(|| "JPEG RGBA buffer size overflows memory limits".to_owned())?,
    );
    match format {
        PixelFormat::L8 => {
            for &value in pixels {
                rgba.extend_from_slice(&[value, value, value, 255]);
            }
        }
        PixelFormat::RGB24 => {
            for rgb in pixels.chunks_exact(3) {
                rgba.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
            }
        }
        PixelFormat::CMYK32 | PixelFormat::L16 => {
            return Err("Unsupported JPEG pixel format for scaled decode".to_owned());
        }
    }

    RgbaImage::from_raw(width, height, rgba)
        .ok_or_else(|| "JPEG RGBA buffer did not match dimensions".to_owned())
}
