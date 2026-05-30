use super::{
    clamp_target_long_edge, decoded_byte_size, display_dimensions_with_upscale,
    prepared_page_from_rgba, reject_oversized_dimensions, DecodeBackend, DecodeOptions,
    PreparedPage, MAX_TARGET_LONG_EDGE,
};
use libwebp_sys::{
    VP8StatusCode, WebPBitstreamFeatures, WebPDecode, WebPDecoderConfig, WebPGetFeatures,
    WebPRGBABuffer, WEBP_CSP_MODE,
};

pub(super) fn prepare_image_with_scaled_libwebp(
    bytes: &[u8],
    target_long_edge: u32,
    options: DecodeOptions,
) -> Result<Option<PreparedPage>, String> {
    if options.resize_filter != crate::core::state::ResizeFilter::Bicubic {
        return Ok(None);
    }

    let features = read_features(bytes)?;
    if features.has_animation != 0 {
        return Ok(None);
    }

    let original_width = features.width as u32;
    let original_height = features.height as u32;
    reject_oversized_dimensions(original_width, original_height)?;

    let target_long_edge = clamp_target_long_edge(target_long_edge);
    if target_long_edge > MAX_TARGET_LONG_EDGE {
        return Ok(None);
    }
    let (display_width, display_height) = display_dimensions_with_upscale(
        original_width,
        original_height,
        target_long_edge,
        options.allow_display_upscale,
    )?;
    if display_width == original_width && display_height == original_height {
        return Ok(None);
    }

    let raw = decode_scaled_rgba(bytes, display_width, display_height)?;
    prepared_page_from_rgba(
        raw,
        original_width,
        original_height,
        display_width,
        display_height,
        target_long_edge,
        DecodeBackend::LibWebpScaled,
    )
    .map(Some)
}

fn read_features(bytes: &[u8]) -> Result<WebPBitstreamFeatures, String> {
    // SAFETY: libwebp's feature struct is a plain C data record, and the
    // upstream wrapper crate initializes it the same way before WebPGetFeatures.
    let mut features = unsafe { std::mem::zeroed::<WebPBitstreamFeatures>() };
    // SAFETY: bytes.as_ptr() is valid for bytes.len() bytes, and libwebp writes
    // only to the initialized feature struct passed by mutable pointer.
    let status = unsafe { WebPGetFeatures(bytes.as_ptr(), bytes.len(), &mut features) };
    if status == VP8StatusCode::VP8_STATUS_OK {
        Ok(features)
    } else {
        Err(format!("libwebp failed to read WebP features: {status:?}"))
    }
}

fn decode_scaled_rgba(bytes: &[u8], width: u32, height: u32) -> Result<Vec<u8>, String> {
    let output_size = decoded_byte_size(width, height)?;
    let mut rgba = vec![0u8; output_size];
    let mut config = WebPDecoderConfig::new()
        .map_err(|_| "libwebp failed to initialize decoder config".to_owned())?;

    config.output.colorspace = WEBP_CSP_MODE::MODE_RGBA;
    config.output.is_external_memory = 1;
    config.output.u.RGBA = WebPRGBABuffer {
        rgba: rgba.as_mut_ptr(),
        stride: i32::try_from(width.saturating_mul(4))
            .map_err(|_| "WebP output stride exceeds platform limits".to_owned())?,
        size: output_size,
    };
    config.options.use_scaling = 1;
    config.options.scaled_width =
        i32::try_from(width).map_err(|_| "WebP scaled width exceeds platform limits".to_owned())?;
    config.options.scaled_height = i32::try_from(height)
        .map_err(|_| "WebP scaled height exceeds platform limits".to_owned())?;

    // SAFETY: config points libwebp at the owned RGBA buffer above, whose size
    // and stride match the requested scaled output dimensions.
    let status = unsafe { WebPDecode(bytes.as_ptr(), bytes.len(), &mut config) };
    if status == VP8StatusCode::VP8_STATUS_OK {
        Ok(rgba)
    } else {
        Err(format!("libwebp scaled decode failed: {status:?}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::state::{DecoderPreference, DecoderPreferences};
    use crate::core::worker::{prepare_image_with_options, DecodeOptions};
    use image::{DynamicImage, ImageFormat, Rgb, RgbImage};
    use std::io::Cursor;

    #[test]
    fn scaled_libwebp_prepares_display_sized_page() {
        let bytes = encoded_webp(2304, 1536);
        let page = prepare_image_with_options(
            &bytes,
            1024,
            DecodeOptions {
                decoder_preferences: DecoderPreferences {
                    webp: DecoderPreference::LibWebp,
                    ..DecoderPreferences::default()
                },
                ..DecodeOptions::default()
            },
        )
        .unwrap();

        assert_eq!(page.decode_backend, DecodeBackend::LibWebpScaled);
        assert_eq!(page.original_width, 2304);
        assert_eq!(page.original_height, 1536);
        assert_eq!(page.display_width, 1024);
        assert_eq!(page.display_height, 683);
    }

    #[test]
    fn scaled_libwebp_defers_non_bicubic_filter_to_full_path() {
        let bytes = encoded_webp(2304, 1536);
        let page = prepare_image_with_options(
            &bytes,
            1024,
            DecodeOptions {
                decoder_preferences: DecoderPreferences {
                    webp: DecoderPreference::LibWebp,
                    ..DecoderPreferences::default()
                },
                resize_filter: crate::core::state::ResizeFilter::Lanczos3,
                ..DecodeOptions::default()
            },
        )
        .unwrap();

        assert_eq!(page.decode_backend, DecodeBackend::LibWebp);
        assert_eq!(page.display_width, 1024);
        assert_eq!(page.display_height, 683);
    }

    fn encoded_webp(width: u32, height: u32) -> Vec<u8> {
        let image = RgbImage::from_fn(width, height, |x, y| {
            Rgb([
                ((x * 3 + y) % 255) as u8,
                ((x + y * 5) % 255) as u8,
                ((x * 7 + y * 11) % 255) as u8,
            ])
        });
        let mut bytes = Vec::new();
        DynamicImage::ImageRgb8(image)
            .write_to(&mut Cursor::new(&mut bytes), ImageFormat::WebP)
            .unwrap();
        bytes
    }
}
