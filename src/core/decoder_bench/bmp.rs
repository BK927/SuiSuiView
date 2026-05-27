use super::candidates::{checked_pixel_count, expect_len, DecodedImage};

pub fn decode_bmp(bytes: &[u8]) -> Result<DecodedImage, String> {
    let header = parse_bmp_file_header(bytes)?;
    decode_bmp_pixels(bytes, &header)
}

pub(super) fn decode_dib(
    bytes: &[u8],
    expected_width: u32,
    expected_height: u32,
) -> Result<DecodedImage, String> {
    let header = parse_dib_header(bytes, 0, 0, Some(expected_width), Some(expected_height))?;
    decode_bmp_pixels(bytes, &header)
}

#[derive(Debug, Clone, Copy)]
struct BmpHeader {
    width: u32,
    height: u32,
    top_down: bool,
    bits_per_pixel: u16,
    pixel_offset: usize,
    row_stride: usize,
    mask_offset: Option<usize>,
    mask_stride: usize,
}

fn parse_bmp_file_header(bytes: &[u8]) -> Result<BmpHeader, String> {
    if bytes.len() < 54 || bytes[0] != b'B' || bytes[1] != b'M' {
        return Err("not a BMP file".to_owned());
    }

    let pixel_offset = le_u32(bytes, 10)? as usize;
    parse_dib_header(bytes, 14, pixel_offset, None, None)
}

fn parse_dib_header(
    bytes: &[u8],
    dib_offset: usize,
    pixel_offset: usize,
    expected_width: Option<u32>,
    expected_height: Option<u32>,
) -> Result<BmpHeader, String> {
    if bytes.len() < dib_offset.saturating_add(40) {
        return Err("DIB header is too short".to_owned());
    }

    let dib_header_size = le_u32(bytes, dib_offset)? as usize;
    if dib_header_size < 40 || dib_offset.saturating_add(dib_header_size) > bytes.len() {
        return Err("unsupported DIB header size".to_owned());
    }

    let width = le_i32(bytes, dib_offset + 4)?;
    let raw_height = le_i32(bytes, dib_offset + 8)?;
    let planes = le_u16(bytes, dib_offset + 12)?;
    let bits_per_pixel = le_u16(bytes, dib_offset + 14)?;
    let compression = le_u32(bytes, dib_offset + 16)?;
    if width <= 0
        || raw_height == 0
        || raw_height == i32::MIN
        || planes != 1
        || !matches!(bits_per_pixel, 24 | 32)
        || compression != 0
    {
        return Err("unsupported BMP variant".to_owned());
    }

    let width = width as u32;
    let mut height = raw_height.unsigned_abs();
    if let Some(expected_height) = expected_height {
        if height == expected_height.saturating_mul(2) {
            height = expected_height;
        } else if height != expected_height {
            return Err("ICO DIB height did not match directory entry".to_owned());
        }
    }
    if let Some(expected_width) = expected_width {
        if width != expected_width {
            return Err("ICO DIB width did not match directory entry".to_owned());
        }
    }

    let row_stride = bmp_row_stride(width, bits_per_pixel)?;
    let pixel_offset = if pixel_offset == 0 {
        dib_offset + dib_header_size
    } else {
        pixel_offset
    };
    let pixel_bytes = row_stride
        .checked_mul(height as usize)
        .ok_or_else(|| "BMP pixel data size overflows memory limits".to_owned())?;
    let pixel_end = pixel_offset
        .checked_add(pixel_bytes)
        .ok_or_else(|| "BMP pixel data offset overflows memory limits".to_owned())?;
    if pixel_end > bytes.len() {
        return Err("BMP pixel data ended unexpectedly".to_owned());
    }

    let mask_stride = bmp_mask_stride(width)?;
    let mask_offset = if expected_height.is_some() {
        let mask_bytes = mask_stride
            .checked_mul(height as usize)
            .ok_or_else(|| "BMP mask size overflows memory limits".to_owned())?;
        let mask_end = pixel_end
            .checked_add(mask_bytes)
            .ok_or_else(|| "BMP mask offset overflows memory limits".to_owned())?;
        (mask_end <= bytes.len()).then_some(pixel_end)
    } else {
        None
    };

    Ok(BmpHeader {
        width,
        height,
        top_down: raw_height < 0,
        bits_per_pixel,
        pixel_offset,
        row_stride,
        mask_offset,
        mask_stride,
    })
}

fn decode_bmp_pixels(bytes: &[u8], header: &BmpHeader) -> Result<DecodedImage, String> {
    let pixel_count = checked_pixel_count(header.width, header.height)?;
    let bytes_per_pixel = usize::from(header.bits_per_pixel / 8);
    let mut rgba = vec![0u8; pixel_count * 4];
    let mut saw_source_alpha = false;
    let source_width = header.width as usize;
    let source_height = header.height as usize;

    for out_y in 0..source_height {
        let source_y = if header.top_down {
            out_y
        } else {
            source_height - 1 - out_y
        };
        let row_start = header
            .pixel_offset
            .checked_add(source_y.saturating_mul(header.row_stride))
            .ok_or_else(|| "BMP row offset overflows memory limits".to_owned())?;

        for x in 0..source_width {
            let source_offset = row_start
                .checked_add(x.saturating_mul(bytes_per_pixel))
                .ok_or_else(|| "BMP pixel offset overflows memory limits".to_owned())?;
            if source_offset + bytes_per_pixel > bytes.len() {
                return Err("BMP pixel data ended unexpectedly".to_owned());
            }

            let target_offset = (out_y * source_width + x) * 4;
            raw_to_rgba_pixel(
                bytes,
                source_offset,
                bytes_per_pixel,
                &mut rgba,
                target_offset,
            );
            if header.bits_per_pixel == 32 && bytes[source_offset + 3] != 0 {
                saw_source_alpha = true;
            }
        }
    }

    if header.bits_per_pixel == 32 && !saw_source_alpha && header.mask_offset.is_none() {
        set_opaque_alpha(&mut rgba);
    }
    if header.mask_offset.is_some() && !(header.bits_per_pixel == 32 && saw_source_alpha) {
        apply_alpha_mask(bytes, header, &mut rgba)?;
    }

    expect_len(rgba.len(), pixel_count * 4, "BMP RGBA")?;
    Ok(DecodedImage::still(header.width, header.height, rgba))
}

fn raw_to_rgba_pixel(
    bytes: &[u8],
    source_offset: usize,
    bytes_per_pixel: usize,
    rgba: &mut [u8],
    target_offset: usize,
) {
    rgba[target_offset] = bytes[source_offset + 2];
    rgba[target_offset + 1] = bytes[source_offset + 1];
    rgba[target_offset + 2] = bytes[source_offset];
    rgba[target_offset + 3] = if bytes_per_pixel == 4 {
        bytes[source_offset + 3]
    } else {
        255
    };
}

fn set_opaque_alpha(rgba: &mut [u8]) {
    for pixel in rgba.chunks_exact_mut(4) {
        pixel[3] = 255;
    }
}

fn apply_alpha_mask(bytes: &[u8], header: &BmpHeader, rgba: &mut [u8]) -> Result<(), String> {
    let Some(mask_offset) = header.mask_offset else {
        return Ok(());
    };
    let source_width = header.width as usize;
    let source_height = header.height as usize;

    for out_y in 0..source_height {
        let source_y = if header.top_down {
            out_y
        } else {
            source_height - 1 - out_y
        };
        let mask_row = mask_offset
            .checked_add(source_y.saturating_mul(header.mask_stride))
            .ok_or_else(|| "BMP mask row offset overflows memory limits".to_owned())?;
        for x in 0..source_width {
            let mask_byte = mask_row + x / 8;
            if mask_byte >= bytes.len() {
                return Err("BMP mask ended unexpectedly".to_owned());
            }
            let target_offset = (out_y * source_width + x) * 4 + 3;
            let transparent = bytes[mask_byte] & (0x80 >> (x % 8)) != 0;
            rgba[target_offset] = if transparent { 0 } else { 255 };
        }
    }

    Ok(())
}

fn bmp_row_stride(width: u32, bits_per_pixel: u16) -> Result<usize, String> {
    let bits_per_row = u64::from(width)
        .checked_mul(u64::from(bits_per_pixel))
        .ok_or_else(|| "BMP row stride overflows memory limits".to_owned())?;
    let stride = bits_per_row.div_ceil(32) * 4;
    usize::try_from(stride).map_err(|_| "BMP row stride exceeds platform limits".to_owned())
}

fn bmp_mask_stride(width: u32) -> Result<usize, String> {
    let stride = u64::from(width).div_ceil(32) * 4;
    usize::try_from(stride).map_err(|_| "BMP mask stride exceeds platform limits".to_owned())
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

#[cfg(test)]
mod tests {
    use super::decode_bmp;

    #[test]
    fn decodes_bottom_up_24_bit_bmp() {
        let decoded = decode_bmp(&two_by_two_bmp()).unwrap();

        assert_eq!(decoded.width, 2);
        assert_eq!(decoded.height, 2);
        assert_eq!(
            decoded.pixels,
            vec![0, 0, 255, 255, 255, 255, 255, 255, 255, 0, 0, 255, 0, 255, 0, 255,]
        );
    }

    pub(super) fn two_by_two_bmp() -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"BM");
        bytes.extend_from_slice(&70u32.to_le_bytes());
        bytes.extend_from_slice(&[0; 4]);
        bytes.extend_from_slice(&54u32.to_le_bytes());
        bytes.extend_from_slice(&40u32.to_le_bytes());
        bytes.extend_from_slice(&2i32.to_le_bytes());
        bytes.extend_from_slice(&2i32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&24u16.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&[0; 16]);
        bytes.extend_from_slice(&[0, 0, 255, 0, 255, 0, 0, 0]);
        bytes.extend_from_slice(&[255, 0, 0, 255, 255, 255, 0, 0]);
        bytes
    }
}
