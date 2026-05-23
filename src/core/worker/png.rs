use super::{
    clamp_target_long_edge, decoded_byte_size, display_dimensions, prepared_page_from_rgba,
    reject_oversized_original, sampled_source_index, DecodeBackend, PreparedPage,
    PNG_SAMPLED_MIN_RATIO,
};
use ::png::{
    BitDepth as PngBitDepth, ColorType as PngColorType, Decoder as PngDecoder, Reader as PngReader,
    Transformations as PngTransformations,
};
use std::io::{BufRead, Cursor, Seek};

pub(super) fn prepare_image_with_sampled_png(
    bytes: &[u8],
    target_long_edge: u32,
) -> Result<Option<PreparedPage>, String> {
    if !is_png(bytes) {
        return Ok(None);
    }

    let target_long_edge = clamp_target_long_edge(target_long_edge);
    let mut decoder = PngDecoder::new(Cursor::new(bytes));
    decoder.set_transformations(PngTransformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(|error| error.to_string())?;
    let info = reader.info();
    if info.interlaced || info.is_animated() {
        return Ok(None);
    }

    let width = info.width;
    let height = info.height;
    reject_oversized_original(width, height)?;

    let (display_width, display_height) = display_dimensions(width, height, target_long_edge)?;
    if width.max(height) < target_long_edge * PNG_SAMPLED_MIN_RATIO {
        return Ok(None);
    }

    let (color_type, bit_depth) = reader.output_color_type();
    if bit_depth != PngBitDepth::Eight {
        return Ok(None);
    }
    let channels = png_channel_count(color_type)?;
    let line_size = reader
        .output_line_size(width)
        .ok_or_else(|| "PNG output row size exceeds platform limits".to_owned())?;
    let mut row = vec![0u8; line_size];
    let raw = sample_png_rows_to_rgba(
        &mut reader,
        &mut row,
        PngSamplePlan {
            color_type,
            channels,
            width,
            height,
            display_width,
            display_height,
        },
    )?;

    prepared_page_from_rgba(
        raw,
        width,
        height,
        display_width,
        display_height,
        target_long_edge,
        DecodeBackend::PngSampled,
    )
    .map(Some)
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
