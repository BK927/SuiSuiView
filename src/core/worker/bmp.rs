use super::{
    clamp_target_long_edge, decoded_byte_size, display_dimensions, prepared_page_from_rgba,
    reject_oversized_original, sampled_source_index, DecodeBackend, PreparedPage,
    BMP_SAMPLED_MIN_RATIO,
};

pub(super) fn prepare_image_with_sampled_bmp(
    bytes: &[u8],
    target_long_edge: u32,
) -> Result<Option<PreparedPage>, String> {
    let Some(header) = parse_bmp_header(bytes)? else {
        return Ok(None);
    };
    let target_long_edge = clamp_target_long_edge(target_long_edge);
    reject_oversized_original(header.width, header.height)?;

    let (display_width, display_height) =
        display_dimensions(header.width, header.height, target_long_edge)?;
    if header.width.max(header.height) < target_long_edge * BMP_SAMPLED_MIN_RATIO {
        return Ok(None);
    }

    let raw = sample_bmp_to_rgba(bytes, &header, display_width, display_height)?;
    prepared_page_from_rgba(
        raw,
        header.width,
        header.height,
        display_width,
        display_height,
        target_long_edge,
        DecodeBackend::BmpSampled,
    )
    .map(Some)
}

#[derive(Debug, Clone, Copy)]
struct BmpHeader {
    width: u32,
    height: u32,
    top_down: bool,
    bits_per_pixel: u16,
    pixel_offset: usize,
    row_stride: usize,
}

fn parse_bmp_header(bytes: &[u8]) -> Result<Option<BmpHeader>, String> {
    if bytes.len() < 54 || bytes[0] != b'B' || bytes[1] != b'M' {
        return Ok(None);
    }

    let pixel_offset = le_u32(bytes, 10)? as usize;
    let dib_header_size = le_u32(bytes, 14)?;
    if dib_header_size < 40 {
        return Ok(None);
    }

    let width = le_i32(bytes, 18)?;
    let raw_height = le_i32(bytes, 22)?;
    let planes = le_u16(bytes, 26)?;
    let bits_per_pixel = le_u16(bytes, 28)?;
    let compression = le_u32(bytes, 30)?;
    if width <= 0
        || raw_height == 0
        || raw_height == i32::MIN
        || planes != 1
        || !matches!(bits_per_pixel, 24 | 32)
        || compression != 0
    {
        return Ok(None);
    }

    let width = width as u32;
    let height = raw_height.unsigned_abs();
    let bytes_per_pixel = u32::from(bits_per_pixel / 8);
    let row_stride = bmp_row_stride(width, bits_per_pixel)?;
    let pixel_bytes = row_stride
        .checked_mul(height as usize)
        .ok_or_else(|| "BMP pixel data size overflows memory limits".to_owned())?;
    let required = pixel_offset
        .checked_add(pixel_bytes)
        .ok_or_else(|| "BMP pixel data offset overflows memory limits".to_owned())?;
    if bytes_per_pixel == 0 || required > bytes.len() {
        return Ok(None);
    }

    Ok(Some(BmpHeader {
        width,
        height,
        top_down: raw_height < 0,
        bits_per_pixel,
        pixel_offset,
        row_stride,
    }))
}

fn bmp_row_stride(width: u32, bits_per_pixel: u16) -> Result<usize, String> {
    let bits_per_row = u64::from(width)
        .checked_mul(u64::from(bits_per_pixel))
        .ok_or_else(|| "BMP row stride overflows memory limits".to_owned())?;
    let stride = bits_per_row.div_ceil(32) * 4;
    usize::try_from(stride).map_err(|_| "BMP row stride exceeds platform limits".to_owned())
}

fn sample_bmp_to_rgba(
    bytes: &[u8],
    header: &BmpHeader,
    display_width: u32,
    display_height: u32,
) -> Result<Vec<u8>, String> {
    let byte_size = decoded_byte_size(display_width, display_height)?;
    let mut raw = vec![0u8; byte_size];
    let bytes_per_pixel = usize::from(header.bits_per_pixel / 8);
    let source_width = header.width as usize;
    let source_height = header.height as usize;
    let display_width = display_width as usize;
    let display_height = display_height as usize;

    for out_y in 0..display_height {
        let source_y = sampled_source_index(out_y, display_height, source_height);
        let row_y = if header.top_down {
            source_y
        } else {
            source_height - 1 - source_y
        };
        let row_start = header
            .pixel_offset
            .checked_add(row_y.saturating_mul(header.row_stride))
            .ok_or_else(|| "BMP row offset overflows memory limits".to_owned())?;

        for out_x in 0..display_width {
            let source_x = sampled_source_index(out_x, display_width, source_width);
            let source_offset = row_start
                .checked_add(source_x.saturating_mul(bytes_per_pixel))
                .ok_or_else(|| "BMP pixel offset overflows memory limits".to_owned())?;
            if source_offset + bytes_per_pixel > bytes.len() {
                return Err("BMP pixel data ended unexpectedly".to_owned());
            }

            let target_offset = (out_y * display_width + out_x) * 4;
            raw[target_offset] = bytes[source_offset + 2];
            raw[target_offset + 1] = bytes[source_offset + 1];
            raw[target_offset + 2] = bytes[source_offset];
            raw[target_offset + 3] = 255;
        }
    }

    Ok(raw)
}

fn le_u16(bytes: &[u8], offset: usize) -> Result<u16, String> {
    let value = bytes
        .get(offset..offset + 2)
        .ok_or_else(|| "Unexpected end of image header".to_owned())?;
    Ok(u16::from_le_bytes([value[0], value[1]]))
}

fn le_u32(bytes: &[u8], offset: usize) -> Result<u32, String> {
    let value = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| "Unexpected end of image header".to_owned())?;
    Ok(u32::from_le_bytes([value[0], value[1], value[2], value[3]]))
}

fn le_i32(bytes: &[u8], offset: usize) -> Result<i32, String> {
    let value = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| "Unexpected end of image header".to_owned())?;
    Ok(i32::from_le_bytes([value[0], value[1], value[2], value[3]]))
}
