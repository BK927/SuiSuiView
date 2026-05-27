use super::candidates::{
    checked_animation_rgba_len, checked_pixel_count, checked_rgba_len, expect_len,
    reserve_animation_frame, DecodedImage,
};
use gif::{
    ColorOutput as GifColorOutput, DecodeOptions as GifDecodeOptions,
    DisposalMethod as GifDisposalMethod,
};
use image::codecs::gif::GifDecoder as ImageGifDecoder;
use image::AnimationDecoder;
use image_webp::WebPDecoder;
use std::io::Cursor;

pub fn decode_image_webp_all_frames(bytes: &[u8]) -> Result<DecodedImage, String> {
    let mut decoder = WebPDecoder::new(Cursor::new(bytes)).map_err(|error| error.to_string())?;
    let (width, height) = decoder.dimensions();
    let has_alpha = decoder.has_alpha();
    checked_rgba_len(width, height)?;
    let buffer_size = decoder
        .output_buffer_size()
        .ok_or_else(|| "WebP output size exceeds platform limits".to_owned())?;
    let mut frame = vec![0u8; buffer_size];
    let mut total_duration_ms = 0u64;

    if decoder.is_animated() {
        let frame_count = decoder.num_frames();
        let mut frames =
            Vec::with_capacity(checked_animation_rgba_len(width, height, frame_count)?);
        for _ in 0..frame_count {
            let duration = decoder
                .read_frame(&mut frame)
                .map_err(|error| error.to_string())?;
            total_duration_ms += u64::from(duration);
            append_frame_rgba(&mut frames, &frame, width, height, has_alpha)?;
        }
        DecodedImage::animation(width, height, frame_count, total_duration_ms, frames)
    } else {
        decoder
            .read_image(&mut frame)
            .map_err(|error| error.to_string())?;
        rgba_or_rgb_frame(frame, width, height, has_alpha)
    }
}

#[cfg(feature = "bench-native-webp")]
pub fn decode_libwebp_all_frames(bytes: &[u8]) -> Result<DecodedImage, String> {
    let features = webp::BitstreamFeatures::new(bytes)
        .ok_or_else(|| "libwebp failed to read WebP bitstream features".to_owned())?;
    if !features.has_animation() {
        return decode_libwebp_still(bytes);
    }

    let animation = webp::AnimDecoder::new(bytes)
        .decode()
        .map_err(|error| format!("libwebp animation decode failed: {error}"))?;
    let mut frames = Vec::new();
    let mut frame_count = 0u32;
    let mut last_timestamp_ms = 0u64;
    let mut canvas_width = None;
    let mut canvas_height = None;

    for frame in &animation {
        let width = frame.width();
        let height = frame.height();
        canvas_width.get_or_insert(width);
        canvas_height.get_or_insert(height);
        if canvas_width != Some(width) || canvas_height != Some(height) {
            return Err("libwebp animation yielded varying canvas dimensions".to_owned());
        }
        last_timestamp_ms = last_timestamp_ms.max(frame.get_time_ms().max(0) as u64);
        append_libwebp_frame_rgba(&mut frames, &frame, width, height)?;
        frame_count += 1;
    }

    let width = canvas_width.ok_or_else(|| "libwebp animation produced no frames".to_owned())?;
    let height = canvas_height.ok_or_else(|| "libwebp animation produced no frames".to_owned())?;
    DecodedImage::animation(width, height, frame_count, last_timestamp_ms, frames)
}

#[cfg(feature = "bench-native-webp")]
fn decode_libwebp_still(bytes: &[u8]) -> Result<DecodedImage, String> {
    let decoder = webp::Decoder::new(bytes);
    let image = decoder
        .decode()
        .ok_or_else(|| "libwebp failed to decode image".to_owned())?;
    let width = image.width();
    let height = image.height();
    let pixels = image.to_vec();
    rgba_or_rgb_frame(pixels, width, height, image.is_alpha())
}

pub fn decode_image_gif_animation(bytes: &[u8]) -> Result<DecodedImage, String> {
    let decoder = ImageGifDecoder::new(Cursor::new(bytes)).map_err(|error| error.to_string())?;
    let mut width = None;
    let mut height = None;
    let mut frames = Vec::new();
    let mut frame_count = 0u32;
    let mut total_duration_ms = 0u64;

    for frame in decoder.into_frames() {
        let frame = frame.map_err(|error| error.to_string())?;
        let (frame_width, frame_height) = frame.buffer().dimensions();
        width.get_or_insert(frame_width);
        height.get_or_insert(frame_height);
        if width != Some(frame_width) || height != Some(frame_height) {
            return Err("GIF animation yielded varying canvas dimensions".to_owned());
        }
        let (delay_num, delay_den) = frame.delay().numer_denom_ms();
        total_duration_ms += rounded_ratio_ms(delay_num, delay_den);
        reserve_animation_frame(&mut frames, frame_width, frame_height)?;
        frames.extend_from_slice(frame.buffer().as_raw());
        frame_count += 1;
    }

    let width = width.ok_or_else(|| "GIF animation contained no frames".to_owned())?;
    let height = height.ok_or_else(|| "GIF animation contained no frames".to_owned())?;
    DecodedImage::animation(width, height, frame_count, total_duration_ms, frames)
}

pub fn decode_gif_animation(bytes: &[u8]) -> Result<DecodedImage, String> {
    let mut options = GifDecodeOptions::new();
    options.set_color_output(GifColorOutput::RGBA);
    let mut reader = options
        .read_info(Cursor::new(bytes))
        .map_err(|error| error.to_string())?;
    let width = u32::from(reader.width());
    let height = u32::from(reader.height());
    let frame_len = checked_rgba_len(width, height)?;
    let mut canvas = vec![0u8; frame_len];
    let mut frames = Vec::new();
    let mut frame_count = 0u32;
    let mut total_duration_ms = 0u64;

    while let Some(frame) = reader
        .read_next_frame()
        .map_err(|error| error.to_string())?
    {
        let restore_previous = if frame.dispose == GifDisposalMethod::Previous {
            Some(copy_gif_rect(&canvas, width, height, frame)?)
        } else {
            None
        };
        overlay_gif_frame(&mut canvas, width, height, frame)?;
        reserve_animation_frame(&mut frames, width, height)?;
        frames.extend_from_slice(&canvas);
        frame_count += 1;
        total_duration_ms += u64::from(frame.delay) * 10;

        match frame.dispose {
            GifDisposalMethod::Any | GifDisposalMethod::Keep => {}
            GifDisposalMethod::Background => clear_gif_rect(&mut canvas, width, height, frame)?,
            GifDisposalMethod::Previous => {
                if let Some(previous) = restore_previous {
                    restore_gif_rect(&mut canvas, width, height, frame, &previous)?;
                }
            }
        }
    }

    DecodedImage::animation(width, height, frame_count, total_duration_ms, frames)
}

fn rgba_or_rgb_frame(
    pixels: Vec<u8>,
    width: u32,
    height: u32,
    has_alpha: bool,
) -> Result<DecodedImage, String> {
    let pixel_count = checked_pixel_count(width, height)?;
    if has_alpha {
        expect_len(pixels.len(), pixel_count * 4, "RGBA")?;
        Ok(DecodedImage::still(width, height, pixels))
    } else {
        expect_len(pixels.len(), pixel_count * 3, "RGB")?;
        let mut rgba = Vec::with_capacity(pixel_count * 4);
        append_rgb_as_rgba(&mut rgba, &pixels);
        Ok(DecodedImage::still(width, height, rgba))
    }
}

fn append_frame_rgba(
    out: &mut Vec<u8>,
    pixels: &[u8],
    width: u32,
    height: u32,
    has_alpha: bool,
) -> Result<(), String> {
    let pixel_count = checked_pixel_count(width, height)?;
    if has_alpha {
        expect_len(pixels.len(), pixel_count * 4, "WebP RGBA frame")?;
        reserve_animation_frame(out, width, height)?;
        out.extend_from_slice(pixels);
    } else {
        expect_len(pixels.len(), pixel_count * 3, "WebP RGB frame")?;
        reserve_animation_frame(out, width, height)?;
        append_rgb_as_rgba(out, pixels);
    }
    Ok(())
}

#[cfg(feature = "bench-native-webp")]
fn append_libwebp_frame_rgba(
    out: &mut Vec<u8>,
    frame: &webp::AnimFrame<'_>,
    width: u32,
    height: u32,
) -> Result<(), String> {
    let pixel_count = checked_pixel_count(width, height)?;
    let pixels = frame.get_image();
    match frame.get_layout() {
        webp::PixelLayout::Rgba => {
            expect_len(
                pixels.len(),
                pixel_count * 4,
                "libwebp RGBA animation frame",
            )?;
            reserve_animation_frame(out, width, height)?;
            out.extend_from_slice(pixels);
        }
        webp::PixelLayout::Rgb => {
            expect_len(pixels.len(), pixel_count * 3, "libwebp RGB animation frame")?;
            reserve_animation_frame(out, width, height)?;
            append_rgb_as_rgba(out, pixels);
        }
    }
    Ok(())
}

fn append_rgb_as_rgba(out: &mut Vec<u8>, rgb: &[u8]) {
    out.reserve(rgb.len() / 3 * 4);
    for pixel in rgb.chunks_exact(3) {
        out.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 255]);
    }
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

fn clear_gif_rect(
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
        return Err("GIF dispose rectangle extends outside logical canvas".to_owned());
    }

    let canvas_width = canvas_width as usize;
    for y in 0..frame_height as usize {
        let start = ((top as usize + y) * canvas_width + left as usize) * 4;
        let end = start + frame_width as usize * 4;
        canvas[start..end].fill(0);
    }
    Ok(())
}

fn copy_gif_rect(
    canvas: &[u8],
    canvas_width: u32,
    canvas_height: u32,
    frame: &gif::Frame<'_>,
) -> Result<Vec<u8>, String> {
    let frame_width = u32::from(frame.width);
    let frame_height = u32::from(frame.height);
    let left = u32::from(frame.left);
    let top = u32::from(frame.top);
    if left.saturating_add(frame_width) > canvas_width
        || top.saturating_add(frame_height) > canvas_height
    {
        return Err("GIF previous rectangle extends outside logical canvas".to_owned());
    }

    let canvas_width = canvas_width as usize;
    let row_len = frame_width as usize * 4;
    let mut rect = Vec::with_capacity(row_len * frame_height as usize);
    for y in 0..frame_height as usize {
        let start = ((top as usize + y) * canvas_width + left as usize) * 4;
        rect.extend_from_slice(&canvas[start..start + row_len]);
    }
    Ok(rect)
}

fn restore_gif_rect(
    canvas: &mut [u8],
    canvas_width: u32,
    canvas_height: u32,
    frame: &gif::Frame<'_>,
    rect: &[u8],
) -> Result<(), String> {
    let frame_width = u32::from(frame.width);
    let frame_height = u32::from(frame.height);
    let left = u32::from(frame.left);
    let top = u32::from(frame.top);
    if left.saturating_add(frame_width) > canvas_width
        || top.saturating_add(frame_height) > canvas_height
    {
        return Err("GIF previous rectangle extends outside logical canvas".to_owned());
    }

    let canvas_width = canvas_width as usize;
    let row_len = frame_width as usize * 4;
    expect_len(
        rect.len(),
        row_len * frame_height as usize,
        "GIF previous rectangle",
    )?;
    for y in 0..frame_height as usize {
        let source = y * row_len;
        let target = ((top as usize + y) * canvas_width + left as usize) * 4;
        canvas[target..target + row_len].copy_from_slice(&rect[source..source + row_len]);
    }
    Ok(())
}

fn rounded_ratio_ms(numerator: u32, denominator: u32) -> u64 {
    if denominator == 0 {
        return 0;
    }
    (u64::from(numerator) + u64::from(denominator) / 2) / u64::from(denominator)
}
