use super::{
    bmp, clamp_target_long_edge, display_dimensions_with_upscale, gif, jpeg, png,
    prepare_image_with_image_crate, prepared_page_from_rgba, reject_oversized_original,
    resize_rgba, DecodeBackend, DecodeOptions, PreparedPage,
};
use crate::core::decoder_backend::{self, DecodedRgba, DecoderFormat};
use crate::core::state::DecoderPreference;
use image::RgbaImage;
#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
use std::time::Instant;

pub(super) fn prepare_image_with_selected_decoder(
    bytes: &[u8],
    target_long_edge: u32,
    options: DecodeOptions,
) -> Result<PreparedPage, String> {
    let Some(format) = decoder_backend::detect_format(bytes) else {
        return prepare_image_with_image_crate(bytes, target_long_edge, options);
    };

    match format {
        DecoderFormat::Jpeg => prepare_jpeg_with_preference(bytes, target_long_edge, options),
        DecoderFormat::Png => prepare_png_with_preference(bytes, target_long_edge, options),
        DecoderFormat::Webp => prepare_webp_with_preference(bytes, target_long_edge, options),
        DecoderFormat::Gif => prepare_gif_with_preference(bytes, target_long_edge, options),
        DecoderFormat::Bmp => prepare_bmp_with_preference(bytes, target_long_edge, options),
        DecoderFormat::Ico => prepare_ico_with_preference(bytes, target_long_edge, options),
        DecoderFormat::Avif => prepare_avif_with_preference(bytes, target_long_edge, options),
        DecoderFormat::Svg => prepare_image_with_image_crate(bytes, target_long_edge, options),
        DecoderFormat::Psd => prepare_psd_with_preference(bytes, target_long_edge, options),
        DecoderFormat::AiPdf => prepare_ai_with_preference(bytes, target_long_edge, options),
    }
}

fn prepare_jpeg_with_preference(
    bytes: &[u8],
    target_long_edge: u32,
    options: DecodeOptions,
) -> Result<PreparedPage, String> {
    match options.decoder_preferences.jpeg {
        DecoderPreference::Default => {
            if let Ok(Some(page)) =
                jpeg::prepare_image_with_scaled_jpeg(bytes, target_long_edge, options)
            {
                return Ok(page);
            }
            prepare_direct_or_image_fallback(
                bytes,
                target_long_edge,
                options,
                DecodeBackend::ZuneJpeg,
                decoder_backend::decode_zune_jpeg,
            )
        }
        DecoderPreference::ImageCrate => {
            prepare_image_with_image_crate(bytes, target_long_edge, options)
        }
        DecoderPreference::ZuneJpeg => prepare_direct_or_image_fallback(
            bytes,
            target_long_edge,
            options,
            DecodeBackend::ZuneJpeg,
            decoder_backend::decode_zune_jpeg,
        ),
        _ => prepare_image_with_image_crate(bytes, target_long_edge, options),
    }
}

fn prepare_png_with_preference(
    bytes: &[u8],
    target_long_edge: u32,
    options: DecodeOptions,
) -> Result<PreparedPage, String> {
    match options.decoder_preferences.png {
        DecoderPreference::Default => {
            match png::prepare_image_with_png_rows(bytes, target_long_edge) {
                Ok(png::PngRowResult::ExactOriginal(page) | png::PngRowResult::Sampled(page)) => {
                    return Ok(page);
                }
                Ok(png::PngRowResult::Unsupported) => {}
                Err(png::PngRowError::ExactOriginal(error)) => return Err(error),
                Err(png::PngRowError::FallbackAllowed(_error)) => {}
            }
            prepare_direct_or_image_fallback(
                bytes,
                target_long_edge,
                options,
                DecodeBackend::PngCrate,
                decoder_backend::decode_png_crate,
            )
        }
        DecoderPreference::ImageCrate => {
            prepare_image_with_image_crate(bytes, target_long_edge, options)
        }
        DecoderPreference::PngCrate => {
            match png::prepare_exact_original_with_png_rows(bytes, target_long_edge) {
                Ok(Some(page)) => return Ok(page),
                Ok(None) => {}
                Err(png::PngRowError::ExactOriginal(error)) => return Err(error),
                Err(png::PngRowError::FallbackAllowed(_error)) => {}
            }
            prepare_direct_or_image_fallback(
                bytes,
                target_long_edge,
                options,
                DecodeBackend::PngCrate,
                decoder_backend::decode_png_crate,
            )
        }
        DecoderPreference::ZunePng => prepare_direct_or_image_fallback(
            bytes,
            target_long_edge,
            options,
            DecodeBackend::ZunePng,
            decoder_backend::decode_zune_png,
        ),
        _ => prepare_image_with_image_crate(bytes, target_long_edge, options),
    }
}

fn prepare_webp_with_preference(
    bytes: &[u8],
    target_long_edge: u32,
    options: DecodeOptions,
) -> Result<PreparedPage, String> {
    match options.decoder_preferences.webp {
        DecoderPreference::Default => {
            if decoder_backend::is_webp_animated(bytes) {
                return prepare_direct_or_image_fallback(
                    bytes,
                    target_long_edge,
                    options,
                    DecodeBackend::ImageWebp,
                    decoder_backend::decode_image_webp,
                );
            }
            prepare_default_webp_still(bytes, target_long_edge, options)
        }
        DecoderPreference::ImageCrate => {
            prepare_image_with_image_crate(bytes, target_long_edge, options)
        }
        DecoderPreference::ImageWebp => prepare_direct_or_image_fallback(
            bytes,
            target_long_edge,
            options,
            DecodeBackend::ImageWebp,
            decoder_backend::decode_image_webp,
        ),
        DecoderPreference::LibWebp => prepare_libwebp_or_fallback(bytes, target_long_edge, options),
        _ => prepare_image_with_image_crate(bytes, target_long_edge, options),
    }
}

fn prepare_gif_with_preference(
    bytes: &[u8],
    target_long_edge: u32,
    options: DecodeOptions,
) -> Result<PreparedPage, String> {
    match options.decoder_preferences.gif {
        DecoderPreference::Default => {
            if let Ok(Some(page)) = gif::prepare_image_with_sampled_gif(bytes, target_long_edge) {
                return Ok(page);
            }
            prepare_direct_or_image_fallback(
                bytes,
                target_long_edge,
                options,
                DecodeBackend::GifCrate,
                decoder_backend::decode_gif_first_frame,
            )
        }
        DecoderPreference::ImageCrate => {
            prepare_image_with_image_crate(bytes, target_long_edge, options)
        }
        DecoderPreference::GifCrate => prepare_direct_or_image_fallback(
            bytes,
            target_long_edge,
            options,
            DecodeBackend::GifCrate,
            decoder_backend::decode_gif_first_frame,
        ),
        _ => prepare_image_with_image_crate(bytes, target_long_edge, options),
    }
}

fn prepare_bmp_with_preference(
    bytes: &[u8],
    target_long_edge: u32,
    options: DecodeOptions,
) -> Result<PreparedPage, String> {
    match options.decoder_preferences.bmp {
        DecoderPreference::Default => {
            if let Ok(Some(page)) = bmp::prepare_image_with_sampled_bmp(bytes, target_long_edge) {
                return Ok(page);
            }
            prepare_direct_or_image_fallback(
                bytes,
                target_long_edge,
                options,
                DecodeBackend::BmpFastPath,
                decoder_backend::decode_bmp,
            )
        }
        DecoderPreference::ImageCrate => {
            prepare_image_with_image_crate(bytes, target_long_edge, options)
        }
        DecoderPreference::BmpFastPath => prepare_direct_or_image_fallback(
            bytes,
            target_long_edge,
            options,
            DecodeBackend::BmpFastPath,
            decoder_backend::decode_bmp,
        ),
        _ => prepare_image_with_image_crate(bytes, target_long_edge, options),
    }
}

fn prepare_ico_with_preference(
    bytes: &[u8],
    target_long_edge: u32,
    options: DecodeOptions,
) -> Result<PreparedPage, String> {
    match options.decoder_preferences.ico {
        DecoderPreference::Default | DecoderPreference::ImageCrate => {
            prepare_image_with_image_crate(bytes, target_long_edge, options)
        }
        DecoderPreference::IcoFastPath => prepare_direct_or_image_fallback(
            bytes,
            target_long_edge,
            options,
            DecodeBackend::IcoFastPath,
            decoder_backend::decode_ico,
        ),
        _ => prepare_image_with_image_crate(bytes, target_long_edge, options),
    }
}

fn prepare_avif_with_preference(
    bytes: &[u8],
    target_long_edge: u32,
    options: DecodeOptions,
) -> Result<PreparedPage, String> {
    match options.decoder_preferences.avif {
        DecoderPreference::Default | DecoderPreference::LibAvifDav1d => {
            prepare_libavif_or_fallback(bytes, target_long_edge, options)
        }
        DecoderPreference::ImageCrate => {
            prepare_image_with_image_crate(bytes, target_long_edge, options)
        }
        _ => prepare_image_with_image_crate(bytes, target_long_edge, options),
    }
}

fn prepare_psd_with_preference(
    bytes: &[u8],
    target_long_edge: u32,
    options: DecodeOptions,
) -> Result<PreparedPage, String> {
    prepare_required_direct(
        bytes,
        target_long_edge,
        options,
        DecodeBackend::ZunePsd,
        decoder_backend::decode_zune_psd,
    )
}

fn prepare_ai_with_preference(
    bytes: &[u8],
    target_long_edge: u32,
    options: DecodeOptions,
) -> Result<PreparedPage, String> {
    prepare_pdfium_ai_or_error(bytes, target_long_edge, options)
}

#[cfg(feature = "native-webp")]
fn prepare_default_webp_still(
    bytes: &[u8],
    target_long_edge: u32,
    options: DecodeOptions,
) -> Result<PreparedPage, String> {
    if let Ok(Some(page)) =
        super::webp::prepare_image_with_scaled_libwebp(bytes, target_long_edge, options)
    {
        return Ok(page);
    }
    prepare_direct_or_image_fallback(
        bytes,
        target_long_edge,
        options,
        DecodeBackend::LibWebp,
        decoder_backend::decode_libwebp,
    )
}

#[cfg(not(feature = "native-webp"))]
fn prepare_default_webp_still(
    bytes: &[u8],
    target_long_edge: u32,
    options: DecodeOptions,
) -> Result<PreparedPage, String> {
    prepare_image_with_image_crate(bytes, target_long_edge, options)
}

#[cfg(feature = "native-webp")]
fn prepare_libwebp_or_fallback(
    bytes: &[u8],
    target_long_edge: u32,
    options: DecodeOptions,
) -> Result<PreparedPage, String> {
    if let Ok(Some(page)) =
        super::webp::prepare_image_with_scaled_libwebp(bytes, target_long_edge, options)
    {
        return Ok(page);
    }
    prepare_direct_or_image_fallback(
        bytes,
        target_long_edge,
        options,
        DecodeBackend::LibWebp,
        decoder_backend::decode_libwebp,
    )
}

#[cfg(not(feature = "native-webp"))]
fn prepare_libwebp_or_fallback(
    bytes: &[u8],
    target_long_edge: u32,
    options: DecodeOptions,
) -> Result<PreparedPage, String> {
    prepare_unavailable_or_image_fallback(
        bytes,
        target_long_edge,
        options,
        DecodeBackend::LibWebp,
        "libwebp backend is not enabled in this build",
    )
}

#[cfg(feature = "native-avif")]
fn prepare_libavif_or_fallback(
    bytes: &[u8],
    target_long_edge: u32,
    options: DecodeOptions,
) -> Result<PreparedPage, String> {
    prepare_direct_or_image_fallback(
        bytes,
        target_long_edge,
        options,
        DecodeBackend::LibAvifDav1d,
        decoder_backend::decode_libavif,
    )
}

#[cfg(not(feature = "native-avif"))]
fn prepare_libavif_or_fallback(
    bytes: &[u8],
    target_long_edge: u32,
    options: DecodeOptions,
) -> Result<PreparedPage, String> {
    prepare_unavailable_or_image_fallback(
        bytes,
        target_long_edge,
        options,
        DecodeBackend::LibAvifDav1d,
        "libavif + dav1d backend is not enabled in this build",
    )
}

fn prepare_direct_or_image_fallback(
    bytes: &[u8],
    target_long_edge: u32,
    options: DecodeOptions,
    backend: DecodeBackend,
    decode: fn(&[u8]) -> Result<DecodedRgba, String>,
) -> Result<PreparedPage, String> {
    #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
    let decode_started = Instant::now();
    match decode(bytes) {
        Ok(decoded) => {
            #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
            super::record_prepare_stage(
                "direct_decode",
                backend,
                decode_started.elapsed(),
                target_long_edge,
                bytes.len(),
                decoded.width,
                decoded.height,
                decoded.width,
                decoded.height,
            );
            prepare_decoded_rgba(decoded, target_long_edge, options, backend, bytes.len())
        }
        Err(error) => {
            prepare_unavailable_or_image_fallback(bytes, target_long_edge, options, backend, &error)
        }
    }
}

fn prepare_required_direct(
    bytes: &[u8],
    target_long_edge: u32,
    options: DecodeOptions,
    backend: DecodeBackend,
    decode: fn(&[u8]) -> Result<DecodedRgba, String>,
) -> Result<PreparedPage, String> {
    let decoded = decode(bytes)?;
    prepare_decoded_rgba(decoded, target_long_edge, options, backend, bytes.len())
}

#[cfg(feature = "native-ai")]
fn prepare_pdfium_ai_or_error(
    bytes: &[u8],
    target_long_edge: u32,
    options: DecodeOptions,
) -> Result<PreparedPage, String> {
    let decoded = decoder_backend::decode_pdfium_ai(bytes, target_long_edge)?;
    prepare_decoded_rgba(
        decoded,
        target_long_edge,
        options,
        DecodeBackend::PdfiumAi,
        bytes.len(),
    )
}

#[cfg(not(feature = "native-ai"))]
fn prepare_pdfium_ai_or_error(
    _bytes: &[u8],
    _target_long_edge: u32,
    _options: DecodeOptions,
) -> Result<PreparedPage, String> {
    Err("PDF-compatible AI preview requires a native-ai build with bundled PDFium".to_owned())
}

pub(super) fn prepare_unavailable_or_image_fallback(
    bytes: &[u8],
    target_long_edge: u32,
    options: DecodeOptions,
    backend: DecodeBackend,
    reason: &str,
) -> Result<PreparedPage, String> {
    match prepare_image_with_image_crate(bytes, target_long_edge, options) {
        Ok(mut page) => {
            page.notice = Some(format!(
                "{} failed; used image fallback: {reason}",
                backend.as_str()
            ));
            Ok(page)
        }
        Err(fallback_error) => Err(format!(
            "{} failed: {reason}; image fallback failed: {fallback_error}",
            backend.as_str()
        )),
    }
}

fn prepare_decoded_rgba(
    decoded: DecodedRgba,
    target_long_edge: u32,
    options: DecodeOptions,
    decode_backend: DecodeBackend,
    _source_bytes: usize,
) -> Result<PreparedPage, String> {
    reject_oversized_original(decoded.width, decoded.height)?;
    let rgba = RgbaImage::from_raw(decoded.width, decoded.height, decoded.pixels)
        .ok_or_else(|| "Decoded RGBA buffer did not match dimensions".to_owned())?;
    let target_long_edge = clamp_target_long_edge(target_long_edge);
    let (display_width, display_height) = display_dimensions_with_upscale(
        decoded.width,
        decoded.height,
        target_long_edge,
        options.allow_display_upscale,
    )?;
    let display = if display_width == decoded.width && display_height == decoded.height {
        rgba
    } else {
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        let resize_started = Instant::now();
        let resize_filter =
            options.scale_filter_for(decoded.width, decoded.height, display_width, display_height);
        let display = resize_rgba(&rgba, display_width, display_height, resize_filter);
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        super::record_prepare_stage(
            "direct_resize",
            decode_backend,
            resize_started.elapsed(),
            target_long_edge,
            _source_bytes,
            decoded.width,
            decoded.height,
            display_width,
            display_height,
        );
        display
    };
    prepared_page_from_rgba(
        display.into_raw(),
        decoded.width,
        decoded.height,
        display_width,
        display_height,
        target_long_edge,
        decode_backend,
    )
}
