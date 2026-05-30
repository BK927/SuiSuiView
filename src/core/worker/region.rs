use super::{png, DecodeBackend, DecodeOptions, DecodeStrategy};
use crate::core::decoder_backend::{self, DecoderFormat};
use crate::core::state::DecoderPreference;
use serde::Serialize;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct OriginalRegion {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone)]
pub struct PreparedRegion {
    pub rgba: Arc<[u8]>,
    pub original_width: u32,
    pub original_height: u32,
    pub region: OriginalRegion,
    pub byte_size: usize,
    pub decode_backend: DecodeBackend,
}

impl PreparedRegion {
    pub fn image_size(&self) -> [usize; 2] {
        [self.region.width as usize, self.region.height as usize]
    }
}

pub fn prepare_original_region_with_options(
    bytes: &[u8],
    region: OriginalRegion,
    options: DecodeOptions,
) -> Result<Option<PreparedRegion>, String> {
    if !can_use_region_decoder(options) {
        return Ok(None);
    }

    match decoder_backend::detect_format(bytes) {
        Some(DecoderFormat::Png) => prepare_png_region(bytes, region),
        _ => Ok(None),
    }
}

fn can_use_region_decoder(options: DecodeOptions) -> bool {
    options.strategy == DecodeStrategy::Auto
        && !options.apply_embedded_icc
        && !options.apply_exif_orientation
        && matches!(
            options.decoder_preferences.png,
            DecoderPreference::Default | DecoderPreference::PngCrate
        )
}

fn prepare_png_region(
    bytes: &[u8],
    region: OriginalRegion,
) -> Result<Option<PreparedRegion>, String> {
    let png_region = png::PngRegion {
        x: region.x,
        y: region.y,
        width: region.width,
        height: region.height,
    };
    let Some(image) =
        png::prepare_exact_original_region_with_png_rows(bytes, png_region).map_err(|error| {
            match error {
                png::PngRowError::ExactOriginal(error)
                | png::PngRowError::FallbackAllowed(error) => error,
            }
        })?
    else {
        return Ok(None);
    };

    Ok(Some(PreparedRegion {
        rgba: image.rgba,
        original_width: image.original_width,
        original_height: image.original_height,
        region,
        byte_size: image.byte_size,
        decode_backend: image.decode_backend,
    }))
}

#[cfg(test)]
mod tests {
    use super::{prepare_original_region_with_options, OriginalRegion};
    use crate::core::worker::{DecodeBackend, DecodeOptions, DecodeStrategy};
    use image::{ImageBuffer, ImageFormat, Rgba};
    use std::io::Cursor;

    #[test]
    fn original_region_decodes_png_source_pixels() {
        let bytes = encoded_xy_png(6, 4);
        let region = OriginalRegion {
            x: 1,
            y: 1,
            width: 3,
            height: 2,
        };

        let prepared =
            prepare_original_region_with_options(&bytes, region, DecodeOptions::default())
                .unwrap()
                .expect("prepared region");

        assert_eq!(prepared.original_width, 6);
        assert_eq!(prepared.original_height, 4);
        assert_eq!(prepared.region, region);
        assert_eq!(prepared.image_size(), [3, 2]);
        assert_eq!(prepared.byte_size, 3 * 2 * 4);
        assert_eq!(prepared.decode_backend, DecodeBackend::PngExactRows);

        let mut expected = Vec::new();
        for y in 1..3 {
            for x in 1..4 {
                expected.extend_from_slice(&xy_rgba(x, y));
            }
        }
        assert_eq!(&*prepared.rgba, expected.as_slice());
    }

    #[test]
    fn original_region_skips_when_options_require_full_image_policy() {
        let bytes = encoded_xy_png(6, 4);
        let region = OriginalRegion {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
        };
        let options = DecodeOptions {
            strategy: DecodeStrategy::ImageCrate,
            ..DecodeOptions::default()
        };

        assert!(matches!(
            prepare_original_region_with_options(&bytes, region, options),
            Ok(None)
        ));
    }

    #[test]
    fn original_region_skips_non_png() {
        let region = OriginalRegion {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
        };

        assert!(matches!(
            prepare_original_region_with_options(b"not-an-image", region, DecodeOptions::default()),
            Ok(None)
        ));
    }

    fn encoded_xy_png(width: u32, height: u32) -> Vec<u8> {
        let image = ImageBuffer::from_fn(width, height, |x, y| Rgba(xy_rgba(x, y)));
        let mut cursor = Cursor::new(Vec::new());
        image
            .write_to(&mut cursor, ImageFormat::Png)
            .expect("encode XY PNG fixture");
        cursor.into_inner()
    }

    fn xy_rgba(x: u32, y: u32) -> [u8; 4] {
        [(x * 17) as u8, (y * 31) as u8, ((x + y) * 13) as u8, 255]
    }
}
