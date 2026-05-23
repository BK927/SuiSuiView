use super::{color_image_to_rgba, UpscaleQualityVisual};
use eframe::egui::ColorImage;
use image::{imageops::FilterType, Rgba, RgbaImage};
use std::fs;
use std::path::Path;

pub(super) fn write_page_visuals(
    root: &Path,
    page_index: usize,
    images: &[(String, ColorImage)],
) -> Result<(String, Vec<UpscaleQualityVisual>), String> {
    let page_dir = root.join(format!("page-{page_index:04}"));
    fs::create_dir_all(&page_dir).map_err(|error| error.to_string())?;

    let mut visuals = Vec::with_capacity(images.len());
    let mut thumbs = Vec::with_capacity(images.len());
    for (order, (method, image)) in images.iter().enumerate() {
        let file_name = format!("{order:02}-{method}.png");
        let path = page_dir.join(&file_name);
        let rgba = color_image_to_rgba_image(image)?;
        save_rgba_image(&rgba, &path)?;
        visuals.push(UpscaleQualityVisual {
            method: method.clone(),
            path: path.display().to_string(),
        });
        thumbs.push((method.clone(), thumbnail_rgba(&rgba, 320, 520)));
    }

    let sheet = contact_sheet(&thumbs);
    let sheet_path = root.join(format!("page-{page_index:04}-contact.png"));
    sheet
        .save(&sheet_path)
        .map_err(|error| format!("failed to write {}: {error}", sheet_path.display()))?;
    Ok((sheet_path.display().to_string(), visuals))
}

pub(super) fn sanitize_name(label: &str) -> String {
    label
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

fn save_rgba_image(rgba: &RgbaImage, path: &Path) -> Result<(), String> {
    rgba.save(path)
        .map_err(|error| format!("failed to write {}: {error}", path.display()))
}

fn color_image_to_rgba_image(image: &ColorImage) -> Result<RgbaImage, String> {
    RgbaImage::from_raw(
        image.size[0] as u32,
        image.size[1] as u32,
        color_image_to_rgba(image),
    )
    .ok_or_else(|| "ColorImage dimensions do not match RGBA bytes".to_owned())
}

fn thumbnail_rgba(image: &RgbaImage, max_width: u32, max_height: u32) -> RgbaImage {
    let width = image.width().max(1);
    let height = image.height().max(1);
    let scale = (max_width as f32 / width as f32)
        .min(max_height as f32 / height as f32)
        .min(1.0);
    let thumb_width = (width as f32 * scale).round().max(1.0) as u32;
    let thumb_height = (height as f32 * scale).round().max(1.0) as u32;
    image::imageops::resize(image, thumb_width, thumb_height, FilterType::Triangle)
}

fn contact_sheet(items: &[(String, RgbaImage)]) -> RgbaImage {
    const GUTTER: u32 = 16;
    let image_width = items
        .iter()
        .map(|(_, image)| RgbaImage::width(image))
        .max()
        .unwrap_or(1);
    let image_height = items
        .iter()
        .map(|(_, image)| RgbaImage::height(image))
        .max()
        .unwrap_or(1);
    let label_scale = label_scale_for(image_height);
    let label_width = items
        .iter()
        .map(|(label, _)| text_width(label, label_scale))
        .max()
        .unwrap_or(1);
    let row_height = image_height.max(text_height(label_scale)) + GUTTER;
    let width = GUTTER * 3 + image_width + label_width;
    let height = GUTTER + row_height * items.len().max(1) as u32;
    let mut sheet =
        RgbaImage::from_pixel(width.max(1), height.max(1), image::Rgba([32, 32, 32, 255]));
    for (row, (label, image)) in items.iter().enumerate() {
        let row_top = GUTTER + row_height * row as u32;
        let image_y = row_top + (row_height - GUTTER - image.height()) / 2;
        let label_x = GUTTER * 2 + image_width;
        let label_y = row_top + (row_height - GUTTER - text_height(label_scale)) / 2;
        image::imageops::overlay(&mut sheet, image, i64::from(GUTTER), i64::from(image_y));
        draw_text(
            &mut sheet,
            label,
            label_x,
            label_y,
            label_scale,
            Rgba([242, 242, 242, 255]),
        );
    }
    sheet
}

fn label_scale_for(image_height: u32) -> u32 {
    (image_height / 72).clamp(4, 12)
}

fn text_width(text: &str, scale: u32) -> u32 {
    let glyph_count = text.chars().count().max(1) as u32;
    glyph_count * 6 * scale - scale
}

fn text_height(scale: u32) -> u32 {
    7 * scale
}

fn draw_text(image: &mut RgbaImage, text: &str, x: u32, y: u32, scale: u32, color: Rgba<u8>) {
    let mut cursor = x;
    for ch in text.chars() {
        draw_glyph(image, ch.to_ascii_uppercase(), cursor, y, scale, color);
        cursor += 6 * scale;
    }
}

fn draw_glyph(image: &mut RgbaImage, ch: char, x: u32, y: u32, scale: u32, color: Rgba<u8>) {
    let glyph = glyph_rows(ch);
    for (row, pattern) in glyph.iter().enumerate() {
        for (col, pixel) in pattern.as_bytes().iter().enumerate() {
            if *pixel == b'1' {
                fill_rect(
                    image,
                    x + col as u32 * scale,
                    y + row as u32 * scale,
                    scale,
                    scale,
                    color,
                );
            }
        }
    }
}

fn fill_rect(image: &mut RgbaImage, x: u32, y: u32, width: u32, height: u32, color: Rgba<u8>) {
    for yy in y..(y + height).min(image.height()) {
        for xx in x..(x + width).min(image.width()) {
            image.put_pixel(xx, yy, color);
        }
    }
}

fn glyph_rows(ch: char) -> [&'static str; 7] {
    match ch {
        'A' => [
            "01110", "10001", "10001", "11111", "10001", "10001", "10001",
        ],
        'B' => [
            "11110", "10001", "10001", "11110", "10001", "10001", "11110",
        ],
        'C' => [
            "01111", "10000", "10000", "10000", "10000", "10000", "01111",
        ],
        'D' => [
            "11110", "10001", "10001", "10001", "10001", "10001", "11110",
        ],
        'E' => [
            "11111", "10000", "10000", "11110", "10000", "10000", "11111",
        ],
        'F' => [
            "11111", "10000", "10000", "11110", "10000", "10000", "10000",
        ],
        'G' => [
            "01111", "10000", "10000", "10111", "10001", "10001", "01111",
        ],
        'H' => [
            "10001", "10001", "10001", "11111", "10001", "10001", "10001",
        ],
        'I' => [
            "11111", "00100", "00100", "00100", "00100", "00100", "11111",
        ],
        'J' => [
            "00111", "00010", "00010", "00010", "10010", "10010", "01100",
        ],
        'K' => [
            "10001", "10010", "10100", "11000", "10100", "10010", "10001",
        ],
        'L' => [
            "10000", "10000", "10000", "10000", "10000", "10000", "11111",
        ],
        'M' => [
            "10001", "11011", "10101", "10101", "10001", "10001", "10001",
        ],
        'N' => [
            "10001", "11001", "10101", "10011", "10001", "10001", "10001",
        ],
        'O' => [
            "01110", "10001", "10001", "10001", "10001", "10001", "01110",
        ],
        'P' => [
            "11110", "10001", "10001", "11110", "10000", "10000", "10000",
        ],
        'Q' => [
            "01110", "10001", "10001", "10001", "10101", "10010", "01101",
        ],
        'R' => [
            "11110", "10001", "10001", "11110", "10100", "10010", "10001",
        ],
        'S' => [
            "01111", "10000", "10000", "01110", "00001", "00001", "11110",
        ],
        'T' => [
            "11111", "00100", "00100", "00100", "00100", "00100", "00100",
        ],
        'U' => [
            "10001", "10001", "10001", "10001", "10001", "10001", "01110",
        ],
        'V' => [
            "10001", "10001", "10001", "10001", "01010", "01010", "00100",
        ],
        'W' => [
            "10001", "10001", "10001", "10101", "10101", "11011", "10001",
        ],
        'X' => [
            "10001", "01010", "00100", "00100", "00100", "01010", "10001",
        ],
        'Y' => [
            "10001", "01010", "00100", "00100", "00100", "00100", "00100",
        ],
        'Z' => [
            "11111", "00001", "00010", "00100", "01000", "10000", "11111",
        ],
        '0' => [
            "01110", "10001", "10011", "10101", "11001", "10001", "01110",
        ],
        '1' => [
            "00100", "01100", "00100", "00100", "00100", "00100", "01110",
        ],
        '2' => [
            "01110", "10001", "00001", "00010", "00100", "01000", "11111",
        ],
        '3' => [
            "11110", "00001", "00001", "01110", "00001", "00001", "11110",
        ],
        '4' => [
            "00010", "00110", "01010", "10010", "11111", "00010", "00010",
        ],
        '5' => [
            "11111", "10000", "10000", "11110", "00001", "00001", "11110",
        ],
        '6' => [
            "01110", "10000", "10000", "11110", "10001", "10001", "01110",
        ],
        '7' => [
            "11111", "00001", "00010", "00100", "01000", "01000", "01000",
        ],
        '8' => [
            "01110", "10001", "10001", "01110", "10001", "10001", "01110",
        ],
        '9' => [
            "01110", "10001", "10001", "01111", "00001", "00001", "01110",
        ],
        '+' => [
            "00000", "00100", "00100", "11111", "00100", "00100", "00000",
        ],
        '-' => [
            "00000", "00000", "00000", "11111", "00000", "00000", "00000",
        ],
        '/' => [
            "00001", "00010", "00010", "00100", "01000", "01000", "10000",
        ],
        '.' => [
            "00000", "00000", "00000", "00000", "00000", "01100", "01100",
        ],
        '_' => [
            "00000", "00000", "00000", "00000", "00000", "00000", "11111",
        ],
        ' ' => [
            "00000", "00000", "00000", "00000", "00000", "00000", "00000",
        ],
        _ => [
            "11111", "00001", "00010", "00100", "00100", "00000", "00100",
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::{contact_sheet, sanitize_name};
    use image::{Rgba, RgbaImage};

    #[test]
    fn visual_file_names_are_stable() {
        assert_eq!(sanitize_name("WGSL FSR1 EASU+RCAS"), "wgsl-fsr1-easu-rcas");
    }

    #[test]
    fn contact_sheet_stacks_entries_vertically_with_labels() {
        let image = RgbaImage::from_pixel(40, 60, Rgba([255, 255, 255, 255]));
        let sheet = contact_sheet(&[
            ("cpu-bicubic".to_owned(), image.clone()),
            ("wgsl-fsr-style".to_owned(), image),
        ]);
        assert!(sheet.height() > 60 * 2);
        assert!(sheet.width() > 40);
    }
}
