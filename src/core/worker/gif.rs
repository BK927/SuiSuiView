use super::{
    clamp_target_long_edge, decoded_byte_size, display_dimensions, prepared_page_from_rgba,
    reject_oversized_original, sampled_source_index, DecodeBackend, PreparedPage,
    GIF_SAMPLED_MIN_RATIO,
};
use gif::{ColorOutput as GifColorOutput, DecodeOptions as GifDecodeOptions};
use std::io::Cursor;

pub(super) fn prepare_image_with_sampled_gif(
    bytes: &[u8],
    target_long_edge: u32,
) -> Result<Option<PreparedPage>, String> {
    if !is_gif(bytes) {
        return Ok(None);
    }

    let target_long_edge = clamp_target_long_edge(target_long_edge);
    let mut options = GifDecodeOptions::new();
    options.set_color_output(GifColorOutput::Indexed);
    let mut reader = options
        .read_info(Cursor::new(bytes))
        .map_err(|error| error.to_string())?;
    let width = u32::from(reader.width());
    let height = u32::from(reader.height());
    reject_oversized_original(width, height)?;

    let (display_width, display_height) = display_dimensions(width, height, target_long_edge)?;
    if width.max(height) < target_long_edge * GIF_SAMPLED_MIN_RATIO {
        return Ok(None);
    }

    let global_palette = reader.global_palette().map(|palette| palette.to_vec());
    let Some(frame) = reader
        .read_next_frame()
        .map_err(|error| error.to_string())?
    else {
        return Ok(None);
    };
    if frame.left != 0
        || frame.top != 0
        || u32::from(frame.width) != width
        || u32::from(frame.height) != height
    {
        return Ok(None);
    }

    let palette = frame
        .palette
        .as_deref()
        .or(global_palette.as_deref())
        .ok_or_else(|| "GIF frame did not include a palette".to_owned())?;
    let raw = sample_indexed_gif_to_rgba(
        frame.buffer.as_ref(),
        palette,
        frame.transparent,
        width,
        height,
        display_width,
        display_height,
    )?;

    prepared_page_from_rgba(
        raw,
        width,
        height,
        display_width,
        display_height,
        target_long_edge,
        DecodeBackend::GifSampled,
    )
    .map(Some)
}

fn is_gif(bytes: &[u8]) -> bool {
    bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a")
}

fn sample_indexed_gif_to_rgba(
    indices: &[u8],
    palette: &[u8],
    transparent: Option<u8>,
    width: u32,
    height: u32,
    display_width: u32,
    display_height: u32,
) -> Result<Vec<u8>, String> {
    let source_pixels = (width as usize)
        .checked_mul(height as usize)
        .ok_or_else(|| "GIF dimensions overflow memory limits".to_owned())?;
    if indices.len() != source_pixels {
        return Err("GIF frame buffer did not match dimensions".to_owned());
    }

    let byte_size = decoded_byte_size(display_width, display_height)?;
    let mut raw = vec![0u8; byte_size];
    let source_width = width as usize;
    let source_height = height as usize;
    let display_width = display_width as usize;
    let display_height = display_height as usize;

    for out_y in 0..display_height {
        let source_y = sampled_source_index(out_y, display_height, source_height);
        for out_x in 0..display_width {
            let source_x = sampled_source_index(out_x, display_width, source_width);
            let palette_index = usize::from(indices[source_y * source_width + source_x]);
            let palette_offset = palette_index
                .checked_mul(3)
                .ok_or_else(|| "GIF palette index overflows memory limits".to_owned())?;
            if palette_offset + 2 >= palette.len() {
                return Err("GIF frame referenced a missing palette entry".to_owned());
            }

            let target_offset = (out_y * display_width + out_x) * 4;
            raw[target_offset] = palette[palette_offset];
            raw[target_offset + 1] = palette[palette_offset + 1];
            raw[target_offset + 2] = palette[palette_offset + 2];
            raw[target_offset + 3] = if transparent == Some(palette_index as u8) {
                0
            } else {
                255
            };
        }
    }

    Ok(raw)
}
