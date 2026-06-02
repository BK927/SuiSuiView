use crate::core::i18n::I18n;
use eframe::egui::{Color32, ColorImage, Vec2};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct ViewTransform {
    pub rotation_quadrants: u8,
    pub flip_horizontal: bool,
    pub flip_vertical: bool,
}

impl ViewTransform {
    pub fn rotated_cw(self) -> Self {
        Self {
            rotation_quadrants: (self.rotation_quadrants + 1) % 4,
            ..self
        }
    }

    pub fn rotated_ccw(self) -> Self {
        Self {
            rotation_quadrants: (self.rotation_quadrants + 3) % 4,
            ..self
        }
    }

    pub fn with_rotation(self, rotation_quadrants: u8) -> Self {
        Self {
            rotation_quadrants: rotation_quadrants % 4,
            ..self
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ImageFilter {
    #[default]
    None,
    Smooth,
    SmoothSharpen,
    RcasSharpen,
}

impl ImageFilter {
    pub fn label(self) -> &'static str {
        match self {
            Self::None => "Filter off",
            Self::Smooth => "Smooth",
            Self::SmoothSharpen => "Smooth+sharp",
            Self::RcasSharpen => "RCAS sharpen",
        }
    }

    pub fn label_i18n(self, i18n: I18n) -> String {
        match self {
            Self::None => i18n.text("label.filter.none"),
            Self::Smooth => i18n.text("label.filter.smooth"),
            Self::SmoothSharpen => i18n.text("label.filter.smooth_sharpen"),
            Self::RcasSharpen => i18n.text("label.filter.rcas"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct ViewEffects {
    pub transform: ViewTransform,
    pub filter: ImageFilter,
    pub gamma: bool,
    pub invert_colors: bool,
}

pub fn apply_effects_to_image(image: &ColorImage, effects: ViewEffects) -> ColorImage {
    let mut output = transform_image(image, effects.transform);
    output = match effects.filter {
        ImageFilter::None => output,
        ImageFilter::Smooth => smooth_image(&output),
        ImageFilter::SmoothSharpen => smooth_sharpen_image(&output),
        ImageFilter::RcasSharpen => rcas_sharpen_image(&output),
    };
    if effects.gamma || effects.invert_colors {
        output = adjust_gamma_and_invert(&output, effects.gamma, effects.invert_colors);
    }
    output
}

fn transform_image(image: &ColorImage, transform: ViewTransform) -> ColorImage {
    if transform == ViewTransform::default() {
        return image.clone();
    }

    let [width, height] = image.size;
    let rotation = transform.rotation_quadrants % 4;
    let output_size = if rotation % 2 == 1 {
        [height, width]
    } else {
        [width, height]
    };
    let [out_width, out_height] = output_size;
    let mut pixels = Vec::with_capacity(out_width * out_height);
    for dst_y in 0..out_height {
        for dst_x in 0..out_width {
            let rotated_x = if transform.flip_horizontal {
                out_width - 1 - dst_x
            } else {
                dst_x
            };
            let rotated_y = if transform.flip_vertical {
                out_height - 1 - dst_y
            } else {
                dst_y
            };
            let (src_x, src_y) = match rotation {
                0 => (rotated_x, rotated_y),
                1 => (rotated_y, height - 1 - rotated_x),
                2 => (width - 1 - rotated_x, height - 1 - rotated_y),
                3 => (width - 1 - rotated_y, rotated_x),
                _ => unreachable!(),
            };
            pixels.push(image.pixels[src_y * width + src_x]);
        }
    }
    ColorImage::new(output_size, pixels)
}

fn smooth_image(image: &ColorImage) -> ColorImage {
    let [width, height] = image.size;
    if width < 2 || height < 2 {
        return image.clone();
    }
    let mut pixels = image.pixels.clone();
    for y in 0..height {
        for x in 0..width {
            pixels[y * width + x] = weighted_average_pixel(image, x, y);
        }
    }
    ColorImage::new(image.size, pixels)
}

fn smooth_sharpen_image(image: &ColorImage) -> ColorImage {
    let [width, height] = image.size;
    if width < 2 || height < 2 {
        return image.clone();
    }
    let mut pixels = Vec::with_capacity(image.pixels.len());
    for y in 0..height {
        for x in 0..width {
            let original = image.pixels[y * width + x];
            let blurred = weighted_average_pixel(image, x, y);
            pixels.push(sharpen_pixel(original, blurred));
        }
    }
    ColorImage::new(image.size, pixels)
}

fn rcas_sharpen_image(image: &ColorImage) -> ColorImage {
    let [width, height] = image.size;
    if width < 2 || height < 2 {
        return image.clone();
    }
    let mut pixels = Vec::with_capacity(image.pixels.len());
    for y in 0..height {
        for x in 0..width {
            pixels.push(rcas_sharpen_pixel(image, x, y));
        }
    }
    ColorImage::new(image.size, pixels)
}

fn weighted_average_pixel(image: &ColorImage, x: usize, y: usize) -> Color32 {
    let [width, height] = image.size;
    let mut r = 0u32;
    let mut g = 0u32;
    let mut b = 0u32;
    let mut a = 0u32;
    let mut total = 0u32;
    for yy in y.saturating_sub(1)..=(y + 1).min(height - 1) {
        for xx in x.saturating_sub(1)..=(x + 1).min(width - 1) {
            let weight = if xx == x && yy == y {
                4
            } else if xx == x || yy == y {
                2
            } else {
                1
            };
            let pixel = image.pixels[yy * width + xx];
            r += pixel.r() as u32 * weight;
            g += pixel.g() as u32 * weight;
            b += pixel.b() as u32 * weight;
            a += pixel.a() as u32 * weight;
            total += weight;
        }
    }
    Color32::from_rgba_unmultiplied(
        (r / total) as u8,
        (g / total) as u8,
        (b / total) as u8,
        (a / total) as u8,
    )
}

fn rcas_sharpen_pixel(image: &ColorImage, x: usize, y: usize) -> Color32 {
    let [width, height] = image.size;
    let center = image.pixels[y * width + x];
    let left = image.pixels[y * width + x.saturating_sub(1)];
    let right = image.pixels[y * width + (x + 1).min(width - 1)];
    let up = image.pixels[y.saturating_sub(1) * width + x];
    let down = image.pixels[(y + 1).min(height - 1) * width + x];

    let min_luma = luma(left)
        .min(luma(right))
        .min(luma(up))
        .min(luma(down))
        .min(luma(center));
    let max_luma = luma(left)
        .max(luma(right))
        .max(luma(up))
        .max(luma(down))
        .max(luma(center));
    let contrast = ((max_luma - min_luma) / 255.0).clamp(0.0, 1.0);
    let amount = 0.18 + (1.0 - contrast) * 0.22;

    let sharpen = |value: u8, a: u8, b: u8, c: u8, d: u8| {
        let average = (a as f32 + b as f32 + c as f32 + d as f32) * 0.25;
        (value as f32 + (value as f32 - average) * amount)
            .round()
            .clamp(0.0, 255.0) as u8
    };

    Color32::from_rgba_unmultiplied(
        sharpen(center.r(), left.r(), right.r(), up.r(), down.r()),
        sharpen(center.g(), left.g(), right.g(), up.g(), down.g()),
        sharpen(center.b(), left.b(), right.b(), up.b(), down.b()),
        center.a(),
    )
}

fn luma(pixel: Color32) -> f32 {
    pixel.r() as f32 * 0.299 + pixel.g() as f32 * 0.587 + pixel.b() as f32 * 0.114
}

fn sharpen_pixel(original: Color32, blurred: Color32) -> Color32 {
    fn channel(original: u8, blurred: u8) -> u8 {
        (original as f32 * 1.55 - blurred as f32 * 0.55).clamp(0.0, 255.0) as u8
    }
    Color32::from_rgba_unmultiplied(
        channel(original.r(), blurred.r()),
        channel(original.g(), blurred.g()),
        channel(original.b(), blurred.b()),
        original.a(),
    )
}

fn adjust_gamma_and_invert(image: &ColorImage, gamma: bool, invert: bool) -> ColorImage {
    let pixels = image
        .pixels
        .iter()
        .map(|pixel| {
            let mut r = pixel.r();
            let mut g = pixel.g();
            let mut b = pixel.b();
            if gamma {
                r = gamma_channel(r);
                g = gamma_channel(g);
                b = gamma_channel(b);
            }
            if invert {
                r = 255 - r;
                g = 255 - g;
                b = 255 - b;
            }
            Color32::from_rgba_unmultiplied(r, g, b, pixel.a())
        })
        .collect();
    ColorImage::new(image.size, pixels)
}

fn gamma_channel(value: u8) -> u8 {
    let normalized = value as f32 / 255.0;
    (normalized.powf(1.0 / 1.2) * 255.0)
        .round()
        .clamp(0.0, 255.0) as u8
}

pub fn transformed_page_size(width: f32, height: f32, transform: ViewTransform) -> Vec2 {
    if transform.rotation_quadrants % 2 == 1 {
        Vec2::new(height, width)
    } else {
        Vec2::new(width, height)
    }
}

pub fn transform_status_suffix(transform: ViewTransform) -> String {
    let mut parts = Vec::new();
    if transform.rotation_quadrants != 0 {
        parts.push(format!(
            "rot {}deg",
            transform.rotation_quadrants as u16 * 90
        ));
    }
    if transform.flip_horizontal {
        parts.push("flip H".to_owned());
    }
    if transform.flip_vertical {
        parts.push("flip V".to_owned());
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!(", {}", parts.join(", "))
    }
}

pub fn compose_images_horizontally(images: &[ColorImage], gap: usize) -> Option<ColorImage> {
    if images.is_empty() {
        return None;
    }
    let width = images.iter().map(|image| image.size[0]).sum::<usize>()
        + gap * images.len().saturating_sub(1);
    let height = images
        .iter()
        .map(|image| image.size[1])
        .max()
        .unwrap_or_default();
    let mut pixels = vec![Color32::TRANSPARENT; width * height];
    let mut cursor_x = 0usize;
    for image in images {
        let top = (height - image.size[1]) / 2;
        for y in 0..image.size[1] {
            for x in 0..image.size[0] {
                pixels[(top + y) * width + cursor_x + x] = image.pixels[y * image.size[0] + x];
            }
        }
        cursor_x += image.size[0] + gap;
    }
    Some(ColorImage::new([width, height], pixels))
}
