use super::{
    checked_pixel_count, dimensions_to_u32, ensure_backend_rgba_budget, expect_len,
    rgba_from_colorspace, DecodedRgba, MAX_BACKEND_DECODED_BYTES, MAX_BACKEND_DIMENSION,
};
use zune_core::{
    bytestream::ZCursor, colorspace::ColorSpace, options::DecoderOptions, result::DecodingResult,
};
use zune_psd::PSDDecoder as ZunePsdDecoder;

pub fn decode_zune_psd(bytes: &[u8]) -> Result<DecodedRgba, String> {
    validate_psd_decode_budget(parse_psd_header(bytes)?)?;
    let options = DecoderOptions::new_fast()
        .set_max_width(MAX_BACKEND_DIMENSION)
        .set_max_height(MAX_BACKEND_DIMENSION);
    let mut decoder = ZunePsdDecoder::new_with_options(ZCursor::new(bytes), options);
    let decoded = decoder.decode().map_err(|error| format!("{error:?}"))?;
    let (width, height) = dimensions_to_u32(decoder.dimensions())?;
    let colorspace = decoder
        .colorspace()
        .ok_or_else(|| "zune-psd did not report output colorspace".to_owned())?;
    match decoded {
        DecodingResult::U8(pixels) => rgba_from_colorspace(pixels, colorspace, width, height),
        DecodingResult::U16(pixels) => {
            rgba_from_u16_preview_colorspace(pixels, colorspace, width, height)
        }
        _ => Err("zune-psd returned an unsupported pixel depth".to_owned()),
    }
}

fn rgba_from_u16_preview_colorspace(
    pixels: Vec<u16>,
    colorspace: ColorSpace,
    width: u32,
    height: u32,
) -> Result<DecodedRgba, String> {
    let pixel_count = checked_pixel_count(width, height)?;
    match colorspace {
        ColorSpace::RGB => {
            expect_len(pixels.len(), pixel_count * 3, "RGB16")?;
            let mut rgba = Vec::with_capacity(pixel_count * 4);
            for rgb in pixels.chunks_exact(3) {
                rgba.extend_from_slice(&[
                    u16_preview_channel(rgb[0]),
                    u16_preview_channel(rgb[1]),
                    u16_preview_channel(rgb[2]),
                    255,
                ]);
            }
            DecodedRgba::new(width, height, rgba)
        }
        ColorSpace::RGBA => {
            expect_len(pixels.len(), pixel_count * 4, "RGBA16")?;
            let mut rgba = Vec::with_capacity(pixel_count * 4);
            for pixel in pixels.chunks_exact(4) {
                rgba.extend_from_slice(&[
                    u16_preview_channel(pixel[0]),
                    u16_preview_channel(pixel[1]),
                    u16_preview_channel(pixel[2]),
                    u16_preview_channel(pixel[3]),
                ]);
            }
            DecodedRgba::new(width, height, rgba)
        }
        ColorSpace::Luma => {
            expect_len(pixels.len(), pixel_count, "luma16")?;
            let mut rgba = Vec::with_capacity(pixel_count * 4);
            for gray in pixels {
                let gray = u16_preview_channel(gray);
                rgba.extend_from_slice(&[gray, gray, gray, 255]);
            }
            DecodedRgba::new(width, height, rgba)
        }
        ColorSpace::LumaA => {
            expect_len(pixels.len(), pixel_count * 2, "luma-alpha16")?;
            let mut rgba = Vec::with_capacity(pixel_count * 4);
            for pair in pixels.chunks_exact(2) {
                let gray = u16_preview_channel(pair[0]);
                rgba.extend_from_slice(&[gray, gray, gray, u16_preview_channel(pair[1])]);
            }
            DecodedRgba::new(width, height, rgba)
        }
        other => Err(format!(
            "unsupported zune 16-bit output colorspace: {other:?}"
        )),
    }
}

fn u16_preview_channel(value: u16) -> u8 {
    (value >> 8) as u8
}

#[derive(Debug, Clone, Copy)]
struct PsdHeader {
    width: u32,
    height: u32,
    channels: u16,
    depth: u16,
    color_mode: u16,
}

fn parse_psd_header(bytes: &[u8]) -> Result<PsdHeader, String> {
    if bytes.len() < 26 || !bytes.starts_with(b"8BPS") {
        return Err("not a PSD file".to_owned());
    }
    let version = be_u16(bytes, 4)?;
    if version != 1 {
        return Err("unsupported PSD version".to_owned());
    }
    Ok(PsdHeader {
        channels: be_u16(bytes, 12)?,
        height: be_u32(bytes, 14)?,
        width: be_u32(bytes, 18)?,
        depth: be_u16(bytes, 22)?,
        color_mode: be_u16(bytes, 24)?,
    })
}

fn validate_psd_decode_budget(header: PsdHeader) -> Result<(), String> {
    match header.color_mode {
        1 if (1..=2).contains(&header.channels) => {}
        3 if (3..=4).contains(&header.channels) => {}
        1 | 3 => {
            return Err(format!(
                "unsupported PSD channel count for color mode {}: {}",
                header.color_mode, header.channels
            ));
        }
        other => return Err(format!("unsupported PSD color mode: {other}")),
    }
    let bytes_per_sample = match header.depth {
        8 => 1usize,
        16 => 2,
        other => return Err(format!("unsupported PSD bit depth: {other}")),
    };
    let pixel_count = checked_pixel_count(header.width, header.height)?;
    let source_bytes = pixel_count
        .checked_mul(usize::from(header.channels))
        .and_then(|samples| samples.checked_mul(bytes_per_sample))
        .ok_or_else(|| "PSD preview size overflows memory limits".to_owned())?;
    if source_bytes > MAX_BACKEND_DECODED_BYTES {
        return Err(format!(
            "PSD preview is too large: {:.1} MB",
            source_bytes as f32 / (1024.0 * 1024.0)
        ));
    }
    ensure_backend_rgba_budget(header.width, header.height, "PSD preview")
}

fn be_u16(bytes: &[u8], offset: usize) -> Result<u16, String> {
    let value = bytes
        .get(offset..offset + 2)
        .ok_or_else(|| "Unexpected end of image header".to_owned())?;
    Ok(u16::from_be_bytes([value[0], value[1]]))
}

fn be_u32(bytes: &[u8], offset: usize) -> Result<u32, String> {
    let value = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| "Unexpected end of image header".to_owned())?;
    Ok(u32::from_be_bytes([value[0], value[1], value[2], value[3]]))
}
