use gif::{ColorOutput as GifColorOutput, DecodeOptions as GifDecodeOptions};
use image_webp::WebPDecoder;
use png::{
    BitDepth as PngBitDepth, ColorType as PngColorType, Decoder as PngDecoder,
    Transformations as PngTransformations,
};
use std::io::Cursor;
use zune_core::bytestream::ZCursor;
use zune_core::colorspace::ColorSpace;
use zune_core::options::DecoderOptions;
use zune_core::result::DecodingResult;
use zune_jpeg::JpegDecoder as ZuneJpegDecoder;
use zune_png::PngDecoder as ZunePngDecoder;

const MAX_BACKEND_DIMENSION: usize = 20_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecoderFormat {
    Jpeg,
    Png,
    Webp,
    Gif,
    Bmp,
    Ico,
    Avif,
    Svg,
}

pub struct DecodedRgba {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl DecodedRgba {
    fn new(width: u32, height: u32, pixels: Vec<u8>) -> Result<Self, String> {
        checked_rgba_len(width, height)
            .and_then(|expected| expect_len(pixels.len(), expected, "RGBA"))?;
        Ok(Self {
            width,
            height,
            pixels,
        })
    }
}

pub fn detect_format(bytes: &[u8]) -> Option<DecoderFormat> {
    if bytes.len() >= 3 && bytes[0] == 0xff && bytes[1] == 0xd8 && bytes[2] == 0xff {
        return Some(DecoderFormat::Jpeg);
    }
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some(DecoderFormat::Png);
    }
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return Some(DecoderFormat::Webp);
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some(DecoderFormat::Gif);
    }
    if bytes.len() >= 2 && &bytes[..2] == b"BM" {
        return Some(DecoderFormat::Bmp);
    }
    if bytes.len() >= 4 && &bytes[..4] == b"\0\0\x01\0" {
        return Some(DecoderFormat::Ico);
    }
    if is_avif_signature(bytes) {
        return Some(DecoderFormat::Avif);
    }
    if is_svg_signature(bytes) {
        return Some(DecoderFormat::Svg);
    }
    None
}

pub fn decode_zune_jpeg(bytes: &[u8]) -> Result<DecodedRgba, String> {
    let options = DecoderOptions::new_fast()
        .set_max_width(MAX_BACKEND_DIMENSION)
        .set_max_height(MAX_BACKEND_DIMENSION)
        .jpeg_set_out_colorspace(ColorSpace::RGBA);
    let mut decoder = ZuneJpegDecoder::new_with_options(ZCursor::new(bytes), options);
    let pixels = decoder.decode().map_err(|error| error.to_string())?;
    let (width, height) = dimensions_to_u32(decoder.dimensions())?;
    let colorspace = decoder
        .output_colorspace()
        .ok_or_else(|| "zune-jpeg did not report output colorspace".to_owned())?;
    rgba_from_colorspace(pixels, colorspace, width, height)
}

pub fn decode_png_crate(bytes: &[u8]) -> Result<DecodedRgba, String> {
    let mut decoder = PngDecoder::new(Cursor::new(bytes));
    decoder.set_transformations(PngTransformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(|error| error.to_string())?;
    let output_size = reader
        .output_buffer_size()
        .ok_or_else(|| "PNG output size exceeds platform limits".to_owned())?;
    let mut pixels = vec![0u8; output_size];
    let info = reader
        .next_frame(&mut pixels)
        .map_err(|error| error.to_string())?;
    pixels.truncate(info.buffer_size());
    rgba_from_png_pixels(
        pixels,
        info.color_type,
        info.bit_depth,
        info.width,
        info.height,
    )
}

pub fn decode_zune_png(bytes: &[u8]) -> Result<DecodedRgba, String> {
    let options = DecoderOptions::new_fast()
        .set_max_width(MAX_BACKEND_DIMENSION)
        .set_max_height(MAX_BACKEND_DIMENSION)
        .png_set_strip_to_8bit(true)
        .png_set_add_alpha_channel(true);
    let mut decoder = ZunePngDecoder::new_with_options(ZCursor::new(bytes), options);
    let decoded = decoder.decode().map_err(|error| error.to_string())?;
    let DecodingResult::U8(pixels) = decoded else {
        return Err("zune-png returned a non-u8 decode result".to_owned());
    };
    let (width, height) = dimensions_to_u32(decoder.dimensions())?;
    let colorspace = decoder
        .colorspace()
        .ok_or_else(|| "zune-png did not report output colorspace".to_owned())?;
    rgba_from_colorspace(pixels, colorspace, width, height)
}

pub fn decode_image_webp(bytes: &[u8]) -> Result<DecodedRgba, String> {
    let mut decoder = WebPDecoder::new(Cursor::new(bytes)).map_err(|error| error.to_string())?;
    let (width, height) = decoder.dimensions();
    let has_alpha = decoder.has_alpha();
    let mut pixels = vec![
        0u8;
        decoder
            .output_buffer_size()
            .ok_or_else(|| "WebP output size exceeds platform limits".to_owned())?
    ];
    if decoder.is_animated() {
        decoder
            .read_frame(&mut pixels)
            .map_err(|error| error.to_string())?;
    } else {
        decoder
            .read_image(&mut pixels)
            .map_err(|error| error.to_string())?;
    }
    if has_alpha {
        DecodedRgba::new(width, height, pixels)
    } else {
        rgba_from_rgb(pixels, width, height)
    }
}

#[cfg(feature = "native-webp")]
pub fn decode_libwebp(bytes: &[u8]) -> Result<DecodedRgba, String> {
    let features = webp::BitstreamFeatures::new(bytes)
        .ok_or_else(|| "libwebp failed to read WebP bitstream features".to_owned())?;
    if features.has_animation() {
        return Err("libwebp still-image path rejected animated WebP".to_owned());
    }
    let decoder = webp::Decoder::new(bytes);
    let image = decoder
        .decode()
        .ok_or_else(|| "libwebp failed to decode image".to_owned())?;
    let width = image.width();
    let height = image.height();
    let pixels = image.to_vec();
    if image.is_alpha() {
        DecodedRgba::new(width, height, pixels)
    } else {
        rgba_from_rgb(pixels, width, height)
    }
}

#[cfg(feature = "native-webp")]
pub fn is_webp_animated(bytes: &[u8]) -> bool {
    webp::BitstreamFeatures::new(bytes).is_some_and(|features| features.has_animation())
}

#[cfg(not(feature = "native-webp"))]
pub fn is_webp_animated(bytes: &[u8]) -> bool {
    WebPDecoder::new(Cursor::new(bytes)).is_ok_and(|decoder| decoder.is_animated())
}

pub fn decode_gif_first_frame(bytes: &[u8]) -> Result<DecodedRgba, String> {
    let mut options = GifDecodeOptions::new();
    options.set_color_output(GifColorOutput::RGBA);
    let mut reader = options
        .read_info(Cursor::new(bytes))
        .map_err(|error| error.to_string())?;
    let width = u32::from(reader.width());
    let height = u32::from(reader.height());
    let frame = reader
        .read_next_frame()
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "GIF did not contain a frame".to_owned())?;
    let frame_width = u32::from(frame.width);
    let frame_height = u32::from(frame.height);
    expect_len(
        frame.buffer.len(),
        checked_pixel_count(frame_width, frame_height)? * 4,
        "GIF RGBA frame rectangle",
    )?;
    if frame.left == 0
        && frame.top == 0
        && frame_width == width
        && frame_height == height
        && frame.buffer.chunks_exact(4).all(|pixel| pixel[3] == 255)
    {
        return DecodedRgba::new(width, height, frame.buffer.to_vec());
    }

    let pixel_count = checked_pixel_count(width, height)?;
    let mut rgba = vec![0u8; pixel_count * 4];
    overlay_gif_frame(&mut rgba, width, height, frame)?;
    DecodedRgba::new(width, height, rgba)
}

pub fn decode_bmp(bytes: &[u8]) -> Result<DecodedRgba, String> {
    let header = parse_bmp_file_header(bytes)?;
    decode_bmp_pixels(bytes, &header)
}

pub fn decode_ico(bytes: &[u8]) -> Result<DecodedRgba, String> {
    let entries = parse_ico_directory(bytes)?;
    let entry = entries
        .into_iter()
        .max_by_key(|entry| {
            (
                u64::from(entry.width) * u64::from(entry.height),
                entry.bit_count,
            )
        })
        .ok_or_else(|| "ICO did not contain an image entry".to_owned())?;
    let payload_end = entry
        .offset
        .checked_add(entry.size)
        .ok_or_else(|| "ICO payload offset overflows memory limits".to_owned())?;
    let payload = bytes
        .get(entry.offset..payload_end)
        .ok_or_else(|| "ICO payload points outside the file".to_owned())?;

    if payload.starts_with(b"\x89PNG\r\n\x1a\n") {
        decode_png_crate(payload)
    } else {
        let header = parse_dib_header(payload, 0, 0, Some(entry.width), Some(entry.height))?;
        decode_bmp_pixels(payload, &header)
    }
}

#[cfg(feature = "native-avif")]
pub fn decode_libavif(bytes: &[u8]) -> Result<DecodedRgba, String> {
    let pixels = libavif::decode_rgb(bytes).map_err(|error| error.to_string())?;
    let width = pixels.width();
    let height = pixels.height();
    DecodedRgba::new(width, height, pixels.to_vec())
}

fn rgba_from_png_pixels(
    pixels: Vec<u8>,
    color_type: PngColorType,
    bit_depth: PngBitDepth,
    width: u32,
    height: u32,
) -> Result<DecodedRgba, String> {
    if bit_depth != PngBitDepth::Eight {
        return Err("PNG direct candidate expected normalized 8-bit output".to_owned());
    }
    match color_type {
        PngColorType::Grayscale => rgba_from_luma(pixels, width, height),
        PngColorType::GrayscaleAlpha => rgba_from_luma_alpha(pixels, width, height),
        PngColorType::Rgb => rgba_from_rgb(pixels, width, height),
        PngColorType::Rgba => DecodedRgba::new(width, height, pixels),
        PngColorType::Indexed => Err("Indexed PNG was not expanded to RGB".to_owned()),
    }
}

fn rgba_from_colorspace(
    pixels: Vec<u8>,
    colorspace: ColorSpace,
    width: u32,
    height: u32,
) -> Result<DecodedRgba, String> {
    match colorspace {
        ColorSpace::RGB => rgba_from_rgb(pixels, width, height),
        ColorSpace::RGBA => DecodedRgba::new(width, height, pixels),
        ColorSpace::Luma => rgba_from_luma(pixels, width, height),
        ColorSpace::LumaA => rgba_from_luma_alpha(pixels, width, height),
        ColorSpace::BGR => rgba_from_bgr(pixels, width, height),
        ColorSpace::BGRA => rgba_from_bgra(pixels, width, height),
        other => Err(format!("unsupported zune output colorspace: {other:?}")),
    }
}

fn rgba_from_rgb(pixels: Vec<u8>, width: u32, height: u32) -> Result<DecodedRgba, String> {
    let pixel_count = checked_pixel_count(width, height)?;
    expect_len(pixels.len(), pixel_count * 3, "RGB")?;
    let mut rgba = Vec::with_capacity(pixel_count * 4);
    for rgb in pixels.chunks_exact(3) {
        rgba.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
    }
    DecodedRgba::new(width, height, rgba)
}

fn rgba_from_bgr(pixels: Vec<u8>, width: u32, height: u32) -> Result<DecodedRgba, String> {
    let pixel_count = checked_pixel_count(width, height)?;
    expect_len(pixels.len(), pixel_count * 3, "BGR")?;
    let mut rgba = Vec::with_capacity(pixel_count * 4);
    for bgr in pixels.chunks_exact(3) {
        rgba.extend_from_slice(&[bgr[2], bgr[1], bgr[0], 255]);
    }
    DecodedRgba::new(width, height, rgba)
}

fn rgba_from_bgra(pixels: Vec<u8>, width: u32, height: u32) -> Result<DecodedRgba, String> {
    let pixel_count = checked_pixel_count(width, height)?;
    expect_len(pixels.len(), pixel_count * 4, "BGRA")?;
    let mut rgba = Vec::with_capacity(pixel_count * 4);
    for bgra in pixels.chunks_exact(4) {
        rgba.extend_from_slice(&[bgra[2], bgra[1], bgra[0], bgra[3]]);
    }
    DecodedRgba::new(width, height, rgba)
}

fn rgba_from_luma(pixels: Vec<u8>, width: u32, height: u32) -> Result<DecodedRgba, String> {
    let pixel_count = checked_pixel_count(width, height)?;
    expect_len(pixels.len(), pixel_count, "luma")?;
    let mut rgba = Vec::with_capacity(pixel_count * 4);
    for gray in pixels {
        rgba.extend_from_slice(&[gray, gray, gray, 255]);
    }
    DecodedRgba::new(width, height, rgba)
}

fn rgba_from_luma_alpha(pixels: Vec<u8>, width: u32, height: u32) -> Result<DecodedRgba, String> {
    let pixel_count = checked_pixel_count(width, height)?;
    expect_len(pixels.len(), pixel_count * 2, "luma-alpha")?;
    let mut rgba = Vec::with_capacity(pixel_count * 4);
    for pair in pixels.chunks_exact(2) {
        rgba.extend_from_slice(&[pair[0], pair[0], pair[0], pair[1]]);
    }
    DecodedRgba::new(width, height, rgba)
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

fn decode_bmp_pixels(bytes: &[u8], header: &BmpHeader) -> Result<DecodedRgba, String> {
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
            rgba[target_offset] = bytes[source_offset + 2];
            rgba[target_offset + 1] = bytes[source_offset + 1];
            rgba[target_offset + 2] = bytes[source_offset];
            rgba[target_offset + 3] = if bytes_per_pixel == 4 {
                bytes[source_offset + 3]
            } else {
                255
            };
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

    DecodedRgba::new(header.width, header.height, rgba)
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

fn overlay_gif_frame(
    canvas: &mut [u8],
    canvas_width: u32,
    canvas_height: u32,
    frame: &gif::Frame<'_>,
) -> Result<(), String> {
    let frame_width = u32::from(frame.width);
    let frame_height = u32::from(frame.height);
    let left = u32::from(frame.left);
    let top = u32::from(frame.top);
    if left.saturating_add(frame_width) > canvas_width
        || top.saturating_add(frame_height) > canvas_height
    {
        return Err("GIF frame rectangle extends outside logical canvas".to_owned());
    }
    expect_len(
        frame.buffer.len(),
        checked_pixel_count(frame_width, frame_height)? * 4,
        "GIF RGBA frame rectangle",
    )?;

    let canvas_width = canvas_width as usize;
    let frame_width = frame_width as usize;
    for y in 0..frame_height as usize {
        for x in 0..frame_width {
            let source = (y * frame_width + x) * 4;
            if frame.buffer[source + 3] == 0 {
                continue;
            }
            let target = ((top as usize + y) * canvas_width + left as usize + x) * 4;
            canvas[target..target + 4].copy_from_slice(&frame.buffer[source..source + 4]);
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy)]
struct IcoEntry {
    width: u32,
    height: u32,
    bit_count: u16,
    size: usize,
    offset: usize,
}

fn parse_ico_directory(bytes: &[u8]) -> Result<Vec<IcoEntry>, String> {
    if bytes.len() < 6 || &bytes[0..4] != b"\0\0\x01\0" {
        return Err("not an ICO file".to_owned());
    }

    let count = usize::from(le_u16(bytes, 4)?);
    let table_bytes = count
        .checked_mul(16)
        .and_then(|value| value.checked_add(6))
        .ok_or_else(|| "ICO directory size overflows memory limits".to_owned())?;
    if count == 0 || table_bytes > bytes.len() {
        return Err("ICO directory is truncated".to_owned());
    }

    let mut entries = Vec::with_capacity(count);
    for index in 0..count {
        let offset = 6 + index * 16;
        let width = ico_dimension(bytes[offset]);
        let height = ico_dimension(bytes[offset + 1]);
        let bit_count = le_u16(bytes, offset + 6)?;
        let size = le_u32(bytes, offset + 8)? as usize;
        let image_offset = le_u32(bytes, offset + 12)? as usize;
        if width == 0 || height == 0 || size == 0 {
            continue;
        }
        entries.push(IcoEntry {
            width,
            height,
            bit_count,
            size,
            offset: image_offset,
        });
    }

    Ok(entries)
}

fn ico_dimension(value: u8) -> u32 {
    if value == 0 {
        256
    } else {
        u32::from(value)
    }
}

fn dimensions_to_u32(dimensions: Option<(usize, usize)>) -> Result<(u32, u32), String> {
    let (width, height) =
        dimensions.ok_or_else(|| "decoder did not report dimensions".to_owned())?;
    let width = u32::try_from(width).map_err(|_| "width exceeds u32".to_owned())?;
    let height = u32::try_from(height).map_err(|_| "height exceeds u32".to_owned())?;
    Ok((width, height))
}

fn checked_pixel_count(width: u32, height: u32) -> Result<usize, String> {
    let width = usize::try_from(width).map_err(|_| "width exceeds platform limits".to_owned())?;
    let height =
        usize::try_from(height).map_err(|_| "height exceeds platform limits".to_owned())?;
    if width > MAX_BACKEND_DIMENSION || height > MAX_BACKEND_DIMENSION {
        return Err(format!(
            "image dimensions exceed backend limit: {width}x{height}"
        ));
    }
    width
        .checked_mul(height)
        .ok_or_else(|| "image dimensions overflow memory limits".to_owned())
}

fn checked_rgba_len(width: u32, height: u32) -> Result<usize, String> {
    checked_pixel_count(width, height)?
        .checked_mul(4)
        .ok_or_else(|| "RGBA buffer length overflows memory limits".to_owned())
}

fn expect_len(actual: usize, expected: usize, label: &str) -> Result<(), String> {
    if actual == expected {
        Ok(())
    } else {
        Err(format!(
            "{label} buffer length mismatch: expected {expected}, got {actual}"
        ))
    }
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

fn is_avif_signature(bytes: &[u8]) -> bool {
    if bytes.len() < 12 || &bytes[4..8] != b"ftyp" {
        return false;
    }
    let brands = &bytes[8..bytes.len().min(32)];
    brands
        .chunks(4)
        .any(|brand| matches!(brand, b"avif" | b"avis"))
}

fn is_svg_signature(bytes: &[u8]) -> bool {
    let probe = &bytes[..bytes.len().min(512)];
    let Ok(text) = std::str::from_utf8(probe) else {
        return false;
    };
    let text = text.trim_start_matches('\u{feff}').trim_start();
    text.starts_with("<svg") || text.contains("<svg")
}
