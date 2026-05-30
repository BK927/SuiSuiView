use super::{
    clamp_target_long_edge, decoded_byte_size, display_dimensions, prepared_page_from_rgba,
    reject_oversized_dimensions, reject_oversized_original, sampled_source_index, DecodeBackend,
    PreparedPage, MAX_TARGET_LONG_EDGE, PNG_SAMPLED_MIN_RATIO,
};
use ::png::{
    BitDepth as PngBitDepth, ColorType as PngColorType, Decoder as PngDecoder, Reader as PngReader,
    Transformations as PngTransformations,
};
use std::io::{BufRead, Cursor, Seek};
use std::sync::Arc;

pub(super) fn prepare_image_with_png_rows(
    bytes: &[u8],
    target_long_edge: u32,
) -> Result<PngRowResult, PngRowError> {
    prepare_image_with_png_rows_for_mode(bytes, target_long_edge, PngRowMode::Default)
}

pub(super) fn prepare_exact_original_with_png_rows(
    bytes: &[u8],
    target_long_edge: u32,
) -> Result<Option<PreparedPage>, PngRowError> {
    match prepare_image_with_png_rows_for_mode(bytes, target_long_edge, PngRowMode::ExactOnly)? {
        PngRowResult::ExactOriginal(page) => Ok(Some(page)),
        PngRowResult::Unsupported | PngRowResult::Sampled(_) => Ok(None),
    }
}

#[allow(dead_code)]
pub(super) fn prepare_exact_original_region_with_png_rows(
    bytes: &[u8],
    region: PngRegion,
) -> Result<Option<PngRegionImage>, PngRowError> {
    if !is_png(bytes) {
        return Ok(None);
    }

    let mut decoder = PngDecoder::new(Cursor::new(bytes));
    decoder.set_transformations(PngTransformations::normalize_to_color8());
    let mut reader = decoder
        .read_info()
        .map_err(|error| PngRowError::ExactOriginal(error.to_string()))?;
    let info = reader.info();
    if info.interlaced || info.is_animated() {
        return Ok(None);
    }

    let width = info.width;
    let height = info.height;
    reject_oversized_dimensions(width, height).map_err(PngRowError::ExactOriginal)?;
    validate_png_region(region, width, height).map_err(PngRowError::ExactOriginal)?;
    reject_oversized_original(region.width, region.height).map_err(PngRowError::ExactOriginal)?;

    let (color_type, bit_depth) = reader.output_color_type();
    if bit_depth != PngBitDepth::Eight {
        return Ok(None);
    }
    let channels = png_channel_count(color_type).map_err(PngRowError::ExactOriginal)?;
    let line_size = reader
        .output_line_size(width)
        .ok_or_else(|| "PNG output row size exceeds platform limits".to_owned())
        .map_err(PngRowError::ExactOriginal)?;
    let mut row = vec![0u8; line_size];
    let raw =
        copy_png_region_rows_to_rgba(&mut reader, &mut row, color_type, channels, width, region)
            .map_err(PngRowError::ExactOriginal)?;
    let byte_size = raw.len();

    Ok(Some(PngRegionImage {
        rgba: Arc::<[u8]>::from(raw.into_boxed_slice()),
        original_width: width,
        original_height: height,
        region,
        byte_size,
        decode_backend: DecodeBackend::PngExactRows,
    }))
}

fn prepare_image_with_png_rows_for_mode(
    bytes: &[u8],
    target_long_edge: u32,
    mode: PngRowMode,
) -> Result<PngRowResult, PngRowError> {
    if !is_png(bytes) {
        return Ok(PngRowResult::Unsupported);
    }

    let target_long_edge = clamp_target_long_edge(target_long_edge);
    let mut decoder = PngDecoder::new(Cursor::new(bytes));
    decoder.set_transformations(PngTransformations::normalize_to_color8());
    let mut reader = decoder
        .read_info()
        .map_err(|error| PngRowError::FallbackAllowed(error.to_string()))?;
    let info = reader.info();
    if info.interlaced || info.is_animated() {
        return Ok(PngRowResult::Unsupported);
    }

    let width = info.width;
    let height = info.height;
    reject_oversized_dimensions(width, height).map_err(PngRowError::FallbackAllowed)?;

    let (display_width, display_height) = display_dimensions(width, height, target_long_edge)
        .map_err(PngRowError::FallbackAllowed)?;
    let exact_original_target = target_long_edge > MAX_TARGET_LONG_EDGE
        && display_width == width
        && display_height == height;
    let sampled_display_target = matches!(mode, PngRowMode::Default)
        && width.max(height) >= target_long_edge.saturating_mul(PNG_SAMPLED_MIN_RATIO);
    if !exact_original_target && !sampled_display_target {
        return Ok(PngRowResult::Unsupported);
    }
    if exact_original_target {
        reject_oversized_original(width, height).map_err(PngRowError::ExactOriginal)?;
    }

    let (color_type, bit_depth) = reader.output_color_type();
    if bit_depth != PngBitDepth::Eight {
        return Ok(PngRowResult::Unsupported);
    }
    let channels = png_channel_count(color_type).map_err(PngRowError::FallbackAllowed)?;
    let line_size = reader
        .output_line_size(width)
        .ok_or_else(|| "PNG output row size exceeds platform limits".to_owned())
        .map_err(|error| png_row_error(exact_original_target, error))?;
    let mut row = vec![0u8; line_size];
    let plan = PngSamplePlan {
        color_type,
        channels,
        width,
        height,
        display_width,
        display_height,
    };
    let (raw, backend) = if exact_original_target {
        (
            copy_png_rows_to_rgba(&mut reader, &mut row, plan)
                .map_err(PngRowError::ExactOriginal)?,
            DecodeBackend::PngExactRows,
        )
    } else {
        (
            sample_png_rows_to_rgba(&mut reader, &mut row, plan)
                .map_err(PngRowError::FallbackAllowed)?,
            DecodeBackend::PngSampled,
        )
    };

    prepared_page_from_rgba(
        raw,
        width,
        height,
        display_width,
        display_height,
        target_long_edge,
        backend,
    )
    .map_err(|error| png_row_error(exact_original_target, error))
    .map(|page| {
        if exact_original_target {
            PngRowResult::ExactOriginal(page)
        } else {
            PngRowResult::Sampled(page)
        }
    })
}

pub(super) enum PngRowResult {
    Unsupported,
    Sampled(PreparedPage),
    ExactOriginal(PreparedPage),
}

#[derive(Debug)]
pub(super) enum PngRowError {
    ExactOriginal(String),
    FallbackAllowed(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub(super) struct PngRegion {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone)]
#[allow(dead_code)]
pub(super) struct PngRegionImage {
    pub rgba: Arc<[u8]>,
    pub original_width: u32,
    pub original_height: u32,
    pub region: PngRegion,
    pub byte_size: usize,
    pub decode_backend: DecodeBackend,
}

#[derive(Clone, Copy)]
enum PngRowMode {
    Default,
    ExactOnly,
}

fn png_row_error(exact_original_target: bool, error: String) -> PngRowError {
    if exact_original_target {
        PngRowError::ExactOriginal(error)
    } else {
        PngRowError::FallbackAllowed(error)
    }
}

fn is_png(bytes: &[u8]) -> bool {
    bytes.starts_with(b"\x89PNG\r\n\x1a\n")
}

fn png_channel_count(color_type: PngColorType) -> Result<usize, String> {
    match color_type {
        PngColorType::Grayscale => Ok(1),
        PngColorType::GrayscaleAlpha => Ok(2),
        PngColorType::Rgb => Ok(3),
        PngColorType::Rgba => Ok(4),
        PngColorType::Indexed => Err("Indexed PNG was not expanded to RGB".to_owned()),
    }
}

#[allow(dead_code)]
fn validate_png_region(
    region: PngRegion,
    source_width: u32,
    source_height: u32,
) -> Result<(), String> {
    if region.width == 0 || region.height == 0 {
        return Err("PNG original region must be non-empty".to_owned());
    }

    let right = region
        .x
        .checked_add(region.width)
        .ok_or_else(|| "PNG original region width overflows image bounds".to_owned())?;
    let bottom = region
        .y
        .checked_add(region.height)
        .ok_or_else(|| "PNG original region height overflows image bounds".to_owned())?;
    if right > source_width || bottom > source_height {
        return Err(format!(
            "PNG original region {region:?} is outside image bounds {source_width}x{source_height}"
        ));
    }

    Ok(())
}

struct PngSamplePlan {
    color_type: PngColorType,
    channels: usize,
    width: u32,
    height: u32,
    display_width: u32,
    display_height: u32,
}

fn sample_png_rows_to_rgba<R: BufRead + Seek>(
    reader: &mut PngReader<R>,
    row: &mut [u8],
    plan: PngSamplePlan,
) -> Result<Vec<u8>, String> {
    let byte_size = decoded_byte_size(plan.display_width, plan.display_height)?;
    let mut raw = vec![0u8; byte_size];
    let source_width = plan.width as usize;
    let source_height = plan.height as usize;
    let display_width = plan.display_width as usize;
    let display_height = plan.display_height as usize;
    let mut next_output_y = 0usize;

    for source_y in 0..source_height {
        if reader
            .read_row(row)
            .map_err(|error| error.to_string())?
            .is_none()
        {
            return Err("PNG ended before all rows were decoded".to_owned());
        }

        while next_output_y < display_height
            && sampled_source_index(next_output_y, display_height, source_height) == source_y
        {
            sample_png_row_to_rgba(
                row,
                &mut raw,
                plan.color_type,
                plan.channels,
                source_width,
                display_width,
                next_output_y,
            )?;
            next_output_y += 1;
        }
    }

    if next_output_y != display_height {
        return Err("PNG sampling did not produce every output row".to_owned());
    }

    Ok(raw)
}

#[allow(dead_code)]
fn copy_png_region_rows_to_rgba<R: BufRead + Seek>(
    reader: &mut PngReader<R>,
    row: &mut [u8],
    color_type: PngColorType,
    channels: usize,
    source_width: u32,
    region: PngRegion,
) -> Result<Vec<u8>, String> {
    let byte_size = decoded_byte_size(region.width, region.height)?;
    let mut raw = vec![0u8; byte_size];
    let source_width = source_width as usize;
    let region_x = region.x as usize;
    let region_y = region.y as usize;
    let region_width = region.width as usize;
    let region_height = region.height as usize;
    let output_stride = region_width
        .checked_mul(4)
        .ok_or_else(|| "PNG region output row size overflows memory limits".to_owned())?;
    let rows_to_read = region_y
        .checked_add(region_height)
        .ok_or_else(|| "PNG region row range overflows memory limits".to_owned())?;

    for source_y in 0..rows_to_read {
        if reader
            .read_row(row)
            .map_err(|error| error.to_string())?
            .is_none()
        {
            return Err("PNG ended before the requested region was decoded".to_owned());
        }

        if source_y < region_y {
            continue;
        }

        let output_y = source_y - region_y;
        let output_start = output_y
            .checked_mul(output_stride)
            .ok_or_else(|| "PNG region output offset overflows memory limits".to_owned())?;
        let output_end = output_start + output_stride;
        copy_png_row_region_to_rgba(
            row,
            &mut raw[output_start..output_end],
            color_type,
            channels,
            source_width,
            region_x,
            region_width,
        )?;
    }

    Ok(raw)
}

fn copy_png_rows_to_rgba<R: BufRead + Seek>(
    reader: &mut PngReader<R>,
    row: &mut [u8],
    plan: PngSamplePlan,
) -> Result<Vec<u8>, String> {
    let byte_size = decoded_byte_size(plan.width, plan.height)?;
    let mut raw = vec![0u8; byte_size];
    let width = plan.width as usize;
    let height = plan.height as usize;
    let output_stride = width
        .checked_mul(4)
        .ok_or_else(|| "PNG output row size overflows memory limits".to_owned())?;

    for source_y in 0..height {
        if reader
            .read_row(row)
            .map_err(|error| error.to_string())?
            .is_none()
        {
            return Err("PNG ended before all rows were decoded".to_owned());
        }

        let output_start = source_y
            .checked_mul(output_stride)
            .ok_or_else(|| "PNG output offset overflows memory limits".to_owned())?;
        let output_end = output_start + output_stride;
        copy_png_row_to_rgba(
            row,
            &mut raw[output_start..output_end],
            plan.color_type,
            plan.channels,
            width,
        )?;
    }

    Ok(raw)
}

#[allow(dead_code)]
fn copy_png_row_region_to_rgba(
    row: &[u8],
    output: &mut [u8],
    color_type: PngColorType,
    channels: usize,
    source_width: usize,
    x: usize,
    width: usize,
) -> Result<(), String> {
    let expected_input = source_width
        .checked_mul(channels)
        .ok_or_else(|| "PNG source row size overflows memory limits".to_owned())?;
    let start = x
        .checked_mul(channels)
        .ok_or_else(|| "PNG region row offset overflows memory limits".to_owned())?;
    let byte_width = width
        .checked_mul(channels)
        .ok_or_else(|| "PNG region row size overflows memory limits".to_owned())?;
    let end = start
        .checked_add(byte_width)
        .ok_or_else(|| "PNG region row end overflows memory limits".to_owned())?;
    if row.len() < expected_input || end > row.len() {
        return Err("PNG row ended before the requested region".to_owned());
    }

    copy_png_row_to_rgba(&row[start..end], output, color_type, channels, width)
}

fn copy_png_row_to_rgba(
    row: &[u8],
    output: &mut [u8],
    color_type: PngColorType,
    channels: usize,
    width: usize,
) -> Result<(), String> {
    let expected_input = width
        .checked_mul(channels)
        .ok_or_else(|| "PNG row size overflows memory limits".to_owned())?;
    let expected_output = width
        .checked_mul(4)
        .ok_or_else(|| "PNG output row size overflows memory limits".to_owned())?;
    if row.len() < expected_input || output.len() < expected_output {
        return Err("PNG row ended unexpectedly".to_owned());
    }

    match color_type {
        PngColorType::Grayscale => {
            for (gray, rgba) in row[..width].iter().zip(output.chunks_exact_mut(4)) {
                rgba.copy_from_slice(&[*gray, *gray, *gray, 255]);
            }
        }
        PngColorType::GrayscaleAlpha => {
            for (pair, rgba) in row[..expected_input]
                .chunks_exact(2)
                .zip(output.chunks_exact_mut(4))
            {
                rgba.copy_from_slice(&[pair[0], pair[0], pair[0], pair[1]]);
            }
        }
        PngColorType::Rgb => {
            for (rgb, rgba) in row[..expected_input]
                .chunks_exact(3)
                .zip(output.chunks_exact_mut(4))
            {
                rgba.copy_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
            }
        }
        PngColorType::Rgba => {
            output[..expected_input].copy_from_slice(&row[..expected_input]);
        }
        PngColorType::Indexed => return Err("Indexed PNG row was not expanded".to_owned()),
    }

    Ok(())
}

fn sample_png_row_to_rgba(
    row: &[u8],
    raw: &mut [u8],
    color_type: PngColorType,
    channels: usize,
    source_width: usize,
    display_width: usize,
    out_y: usize,
) -> Result<(), String> {
    for out_x in 0..display_width {
        let source_x = sampled_source_index(out_x, display_width, source_width);
        let source_offset = source_x
            .checked_mul(channels)
            .ok_or_else(|| "PNG row offset overflows memory limits".to_owned())?;
        if source_offset + channels > row.len() {
            return Err("PNG row ended unexpectedly".to_owned());
        }

        let target_offset = (out_y * display_width + out_x) * 4;
        match color_type {
            PngColorType::Grayscale => {
                let gray = row[source_offset];
                raw[target_offset] = gray;
                raw[target_offset + 1] = gray;
                raw[target_offset + 2] = gray;
                raw[target_offset + 3] = 255;
            }
            PngColorType::GrayscaleAlpha => {
                let gray = row[source_offset];
                raw[target_offset] = gray;
                raw[target_offset + 1] = gray;
                raw[target_offset + 2] = gray;
                raw[target_offset + 3] = row[source_offset + 1];
            }
            PngColorType::Rgb => {
                raw[target_offset] = row[source_offset];
                raw[target_offset + 1] = row[source_offset + 1];
                raw[target_offset + 2] = row[source_offset + 2];
                raw[target_offset + 3] = 255;
            }
            PngColorType::Rgba => {
                raw[target_offset] = row[source_offset];
                raw[target_offset + 1] = row[source_offset + 1];
                raw[target_offset + 2] = row[source_offset + 2];
                raw[target_offset + 3] = row[source_offset + 3];
            }
            PngColorType::Indexed => return Err("Indexed PNG row was not expanded".to_owned()),
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::core::worker::{
        prepare_image_with_strategy, DecodeBackend, DecodeStrategy, MAX_TARGET_LONG_EDGE,
    };
    use image::{ImageBuffer, ImageFormat, Rgba};
    use std::io::Cursor;

    #[test]
    fn auto_strategy_uses_exact_row_png_for_original_inspection_target() {
        let target = MAX_TARGET_LONG_EDGE + 1;
        let bytes = encoded_png(target, 2);
        let page = prepare_image_with_strategy(&bytes, target, DecodeStrategy::Auto).unwrap();

        assert_eq!(page.decode_backend, DecodeBackend::PngExactRows);
        assert_eq!(page.original_width, target as usize);
        assert_eq!(page.original_height, 2);
        assert_eq!(page.display_width, target as usize);
        assert_eq!(page.display_height, 2);
        assert_eq!(page.rgba.len(), target as usize * 2 * 4);
        assert_eq!(
            &page.rgba[..12],
            &[0, 0, 0, 255, 1, 1, 1, 255, 2, 2, 2, 255]
        );
    }

    #[test]
    fn oversized_original_png_rows_do_not_fallback_to_full_decode() {
        let bytes = png_header_only(8192, 8193);
        let error = match prepare_image_with_strategy(&bytes, 8193, DecodeStrategy::Auto) {
            Ok(_) => panic!("oversized original PNG unexpectedly decoded"),
            Err(error) => error,
        };

        assert!(error.contains("Decoded page is too large"), "{error}");
        assert!(!error.contains("png-crate failed"), "{error}");
    }

    #[test]
    fn sampled_png_row_error_can_fallback_to_full_decoder() {
        let bytes = png_header_only(10_000, 10_000);
        let error = match prepare_image_with_strategy(&bytes, 5000, DecodeStrategy::Auto) {
            Ok(_) => panic!("malformed sampled PNG unexpectedly decoded"),
            Err(error) => error,
        };

        assert!(error.contains("png-crate failed"), "{error}");
    }

    #[test]
    fn exact_original_row_helper_skips_sampled_targets() {
        let target = MAX_TARGET_LONG_EDGE;
        let bytes = encoded_png(target * 2, 1);

        assert!(matches!(
            super::prepare_exact_original_with_png_rows(&bytes, target),
            Ok(None)
        ));
        assert!(matches!(
            super::prepare_image_with_png_rows(&bytes, target),
            Ok(super::PngRowResult::Sampled(_))
        ));
    }

    #[test]
    fn exact_original_region_png_rows_decode_only_requested_pixels() {
        let bytes = encoded_xy_png(6, 4);
        let region = super::PngRegion {
            x: 2,
            y: 1,
            width: 3,
            height: 2,
        };
        let image = super::prepare_exact_original_region_with_png_rows(&bytes, region)
            .unwrap()
            .expect("PNG region image");

        assert_eq!(image.original_width, 6);
        assert_eq!(image.original_height, 4);
        assert_eq!(image.region, region);
        assert_eq!(image.byte_size, 3 * 2 * 4);
        assert_eq!(image.decode_backend, DecodeBackend::PngExactRows);

        let mut expected = Vec::new();
        for y in 1..3 {
            for x in 2..5 {
                expected.extend_from_slice(&xy_rgba(x, y));
            }
        }
        assert_eq!(&*image.rgba, expected.as_slice());
    }

    #[test]
    fn exact_original_region_png_rows_rejects_out_of_bounds_region() {
        let bytes = encoded_xy_png(6, 4);
        let region = super::PngRegion {
            x: 4,
            y: 2,
            width: 3,
            height: 2,
        };
        let error = match super::prepare_exact_original_region_with_png_rows(&bytes, region) {
            Err(super::PngRowError::ExactOriginal(error)) => error,
            _ => panic!("out-of-bounds PNG region unexpectedly decoded"),
        };

        assert!(error.contains("outside image bounds"), "{error}");
    }

    #[test]
    fn exact_original_region_png_rows_rejects_oversized_region_before_decode() {
        let bytes = png_header_only(10_000, 10_000);
        let region = super::PngRegion {
            x: 0,
            y: 0,
            width: 10_000,
            height: 10_000,
        };
        let error = match super::prepare_exact_original_region_with_png_rows(&bytes, region) {
            Err(super::PngRowError::ExactOriginal(error)) => error,
            _ => panic!("oversized PNG region unexpectedly decoded"),
        };

        assert!(error.contains("Decoded page is too large"), "{error}");
    }

    #[test]
    fn exact_original_region_png_rows_skips_non_png() {
        assert!(matches!(
            super::prepare_exact_original_region_with_png_rows(
                b"not a png",
                super::PngRegion {
                    x: 0,
                    y: 0,
                    width: 1,
                    height: 1,
                },
            ),
            Ok(None)
        ));
    }

    fn encoded_png(width: u32, height: u32) -> Vec<u8> {
        let image = ImageBuffer::from_fn(width, height, |x, _y| {
            let value = (x % 251) as u8;
            Rgba([value, value, value, 255])
        });
        let mut cursor = Cursor::new(Vec::new());
        image
            .write_to(&mut cursor, ImageFormat::Png)
            .expect("encode PNG fixture");
        cursor.into_inner()
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

    fn png_header_only(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut ihdr = Vec::with_capacity(13);
        ihdr.extend_from_slice(&width.to_be_bytes());
        ihdr.extend_from_slice(&height.to_be_bytes());
        ihdr.extend_from_slice(&[8, 0, 0, 0, 0]);
        push_png_chunk(&mut bytes, *b"IHDR", &ihdr);
        push_png_chunk(&mut bytes, *b"IDAT", &[]);
        push_png_chunk(&mut bytes, *b"IEND", &[]);
        bytes
    }

    fn push_png_chunk(bytes: &mut Vec<u8>, name: [u8; 4], data: &[u8]) {
        bytes.extend_from_slice(&(data.len() as u32).to_be_bytes());
        bytes.extend_from_slice(&name);
        bytes.extend_from_slice(data);

        let mut crc_input = Vec::with_capacity(name.len() + data.len());
        crc_input.extend_from_slice(&name);
        crc_input.extend_from_slice(data);
        bytes.extend_from_slice(&crc32(&crc_input).to_be_bytes());
    }

    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = 0xffff_ffffu32;
        for byte in bytes {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                let mask = 0u32.wrapping_sub(crc & 1);
                crc = (crc >> 1) ^ (0xedb8_8320 & mask);
            }
        }
        !crc
    }
}
