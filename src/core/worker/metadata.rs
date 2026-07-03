use super::{image_reader, jpeg, retained_page_byte_size, PagePixels, PreparedPage};
#[cfg(test)]
use egui::ColorImage;
use image::{metadata::Orientation, ImageDecoder};
use lcms2::{Intent, PixelFormat as LcmsPixelFormat, Profile, Transform};
use std::io::{BufReader, Cursor};
use std::sync::Arc;

pub(super) struct ImageMetadata {
    pub(super) icc_profile: Result<Option<Vec<u8>>, String>,
    pub(super) orientation: Option<Orientation>,
}

impl Default for ImageMetadata {
    fn default() -> Self {
        Self {
            icc_profile: Ok(None),
            orientation: None,
        }
    }
}

pub(super) fn read_image_metadata(
    bytes: &[u8],
    need_icc: bool,
    need_orientation: bool,
) -> ImageMetadata {
    let mut metadata = ImageMetadata::default();
    if !need_icc && !need_orientation {
        return metadata;
    }

    match image_reader(bytes)
        .and_then(|reader| reader.into_decoder().map_err(|error| error.to_string()))
    {
        Ok(mut decoder) => {
            if need_icc {
                metadata.icc_profile = decoder.icc_profile().map_err(|error| error.to_string());
            }
            if need_orientation {
                metadata.orientation = decoder
                    .orientation()
                    .ok()
                    .filter(|orientation| *orientation != Orientation::NoTransforms);
            }
        }
        Err(error) => {
            if need_icc {
                metadata.icc_profile = Err(error);
            }
        }
    }

    if need_orientation && metadata.orientation.is_none() && jpeg::is_jpeg(bytes) {
        metadata.orientation = exif_orientation_from_container(bytes)
            .filter(|orientation| *orientation != Orientation::NoTransforms);
    }

    metadata
}

fn exif_orientation_from_container(bytes: &[u8]) -> Option<Orientation> {
    let mut reader = BufReader::new(Cursor::new(bytes));
    let exif = exif::Reader::new().read_from_container(&mut reader).ok()?;
    let field = exif.get_field(exif::Tag::Orientation, exif::In::PRIMARY)?;
    let value = field.value.get_uint(0)?;
    Orientation::from_exif(u8::try_from(value).ok()?)
}

pub(super) fn apply_embedded_icc_to_rgba(raw: &mut [u8], profile: &[u8]) -> Result<(), String> {
    if raw.is_empty() {
        return Ok(());
    }
    let source = Profile::new_icc(profile).map_err(|error| error.to_string())?;
    let srgb = Profile::new_srgb();
    let transform = Transform::<u8, u8>::new(
        &source,
        LcmsPixelFormat::RGBA_8,
        &srgb,
        LcmsPixelFormat::RGBA_8,
        Intent::Perceptual,
    )
    .map_err(|error| error.to_string())?;
    transform.transform_in_place(raw);
    Ok(())
}

pub(super) fn apply_exif_orientation_to_page(
    mut page: PreparedPage,
    orientation: Orientation,
) -> PreparedPage {
    if orientation == Orientation::NoTransforms {
        return page;
    }

    let transform = orientation_transform(orientation);
    let source_size = [page.display_width, page.display_height];
    let bpp = page.pixels.bytes_per_pixel();
    debug_assert_eq!(
        page.pixels.byte_len(),
        source_size[0] * source_size[1] * bpp
    );
    let Some((output_size, output_bytes)) =
        transform_pixels(source_size, page.pixels.as_slice(), bpp, transform)
    else {
        return page;
    };
    let bytes = Arc::<[u8]>::from(output_bytes.into_boxed_slice());
    page.byte_size = retained_page_byte_size(bytes.len());
    page.pixels = match &page.pixels {
        PagePixels::Rgba(_) => PagePixels::Rgba(bytes),
        PagePixels::Luma(_) => PagePixels::Luma(bytes),
    };
    page.display_width = output_size[0];
    page.display_height = output_size[1];
    if orientation_swaps_dimensions(orientation) {
        std::mem::swap(&mut page.original_width, &mut page.original_height);
    }
    page
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct OrientationTransform {
    rotation_quadrants: u8,
    flip_horizontal: bool,
    flip_vertical: bool,
}

#[cfg(test)]
fn orient_color_image(image: &ColorImage, orientation: Orientation) -> ColorImage {
    transform_color_image(image, orientation_transform(orientation))
}

fn orientation_transform(orientation: Orientation) -> OrientationTransform {
    match orientation {
        Orientation::NoTransforms => OrientationTransform::default(),
        Orientation::Rotate90 => OrientationTransform {
            rotation_quadrants: 1,
            ..OrientationTransform::default()
        },
        Orientation::Rotate180 => OrientationTransform {
            rotation_quadrants: 2,
            ..OrientationTransform::default()
        },
        Orientation::Rotate270 => OrientationTransform {
            rotation_quadrants: 3,
            ..OrientationTransform::default()
        },
        Orientation::FlipHorizontal => OrientationTransform {
            flip_horizontal: true,
            ..OrientationTransform::default()
        },
        Orientation::FlipVertical => OrientationTransform {
            flip_vertical: true,
            ..OrientationTransform::default()
        },
        Orientation::Rotate90FlipH => OrientationTransform {
            rotation_quadrants: 1,
            flip_horizontal: true,
            ..OrientationTransform::default()
        },
        Orientation::Rotate270FlipH => OrientationTransform {
            rotation_quadrants: 3,
            flip_horizontal: true,
            ..OrientationTransform::default()
        },
    }
}

#[cfg(test)]
fn transform_color_image(image: &ColorImage, transform: OrientationTransform) -> ColorImage {
    if transform == OrientationTransform::default() {
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

/// Apply an EXIF orientation to a tightly packed pixel buffer of `bpp` bytes per pixel. The index
/// remap is identical for any `bpp`; only the per-pixel stride differs. RGBA (bpp == 4) keeps the
/// hand-optimized row/transpose fast paths; other channel counts (e.g. luma bpp == 1) use the
/// generic pixel-copy loop.
fn transform_pixels(
    size: [usize; 2],
    bytes: &[u8],
    bpp: usize,
    transform: OrientationTransform,
) -> Option<([usize; 2], Vec<u8>)> {
    let [width, height] = size;
    if bytes.len() != width.checked_mul(height)?.checked_mul(bpp)? {
        return None;
    }

    if bpp == 4 {
        let rotation = transform.rotation_quadrants % 4;
        let output = match (rotation, transform.flip_horizontal, transform.flip_vertical) {
            (0, false, false) => (size, bytes.to_vec()),
            (0, true, false) => (size, flip_rgba_horizontal(width, height, bytes)?),
            (0, false, true) => (size, flip_rgba_vertical(width, height, bytes)?),
            (2, false, false) => (size, rotate_rgba_180(width, height, bytes)?),
            (1, false, false) => ([height, width], rotate_rgba_90(width, height, bytes)?),
            (3, false, false) => ([height, width], rotate_rgba_270(width, height, bytes)?),
            (1, true, false) => ([height, width], transpose_rgba(width, height, bytes)?),
            (3, true, false) => ([height, width], transpose_rgba_anti(width, height, bytes)?),
            _ => transform_pixels_generic(size, bytes, bpp, transform)?,
        };
        return Some(output);
    }

    transform_pixels_generic(size, bytes, bpp, transform)
}

fn transform_pixels_generic(
    size: [usize; 2],
    bytes: &[u8],
    bpp: usize,
    transform: OrientationTransform,
) -> Option<([usize; 2], Vec<u8>)> {
    let [width, height] = size;
    let rotation = transform.rotation_quadrants % 4;
    let output_size = if rotation % 2 == 1 {
        [height, width]
    } else {
        [width, height]
    };
    let [out_width, out_height] = output_size;
    let mut output = vec![0; out_width.checked_mul(out_height)?.checked_mul(bpp)?];
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
            copy_pixel(
                bytes,
                &mut output,
                width,
                out_width,
                bpp,
                src_x,
                src_y,
                dst_x,
                dst_y,
            );
        }
    }
    Some((output_size, output))
}

fn flip_rgba_horizontal(width: usize, height: usize, rgba: &[u8]) -> Option<Vec<u8>> {
    let row_bytes = width.checked_mul(4)?;
    let mut output = vec![0; row_bytes.checked_mul(height)?];
    for y in 0..height {
        let row_start = y.checked_mul(row_bytes)?;
        copy_reversed_rgba_row(
            &rgba[row_start..row_start + row_bytes],
            &mut output[row_start..row_start + row_bytes],
        );
    }
    Some(output)
}

fn flip_rgba_vertical(width: usize, height: usize, rgba: &[u8]) -> Option<Vec<u8>> {
    let row_bytes = width.checked_mul(4)?;
    let mut output = vec![0; row_bytes.checked_mul(height)?];
    for dst_y in 0..height {
        let src_y = height - 1 - dst_y;
        let src_start = src_y.checked_mul(row_bytes)?;
        let dst_start = dst_y.checked_mul(row_bytes)?;
        output[dst_start..dst_start + row_bytes]
            .copy_from_slice(&rgba[src_start..src_start + row_bytes]);
    }
    Some(output)
}

fn rotate_rgba_180(width: usize, height: usize, rgba: &[u8]) -> Option<Vec<u8>> {
    let row_bytes = width.checked_mul(4)?;
    let mut output = vec![0; row_bytes.checked_mul(height)?];
    for dst_y in 0..height {
        let src_y = height - 1 - dst_y;
        let src_start = src_y.checked_mul(row_bytes)?;
        let dst_start = dst_y.checked_mul(row_bytes)?;
        copy_reversed_rgba_row(
            &rgba[src_start..src_start + row_bytes],
            &mut output[dst_start..dst_start + row_bytes],
        );
    }
    Some(output)
}

fn rotate_rgba_90(width: usize, height: usize, rgba: &[u8]) -> Option<Vec<u8>> {
    let out_width = height;
    let out_height = width;
    let mut output = vec![0; out_width.checked_mul(out_height)?.checked_mul(4)?];
    for src_y in 0..height {
        for src_x in 0..width {
            let dst_x = height - 1 - src_y;
            let dst_y = src_x;
            copy_pixel(
                rgba,
                &mut output,
                width,
                out_width,
                4,
                src_x,
                src_y,
                dst_x,
                dst_y,
            );
        }
    }
    Some(output)
}

fn rotate_rgba_270(width: usize, height: usize, rgba: &[u8]) -> Option<Vec<u8>> {
    let out_width = height;
    let out_height = width;
    let mut output = vec![0; out_width.checked_mul(out_height)?.checked_mul(4)?];
    for src_y in 0..height {
        for src_x in 0..width {
            let dst_x = src_y;
            let dst_y = width - 1 - src_x;
            copy_pixel(
                rgba,
                &mut output,
                width,
                out_width,
                4,
                src_x,
                src_y,
                dst_x,
                dst_y,
            );
        }
    }
    Some(output)
}

fn transpose_rgba(width: usize, height: usize, rgba: &[u8]) -> Option<Vec<u8>> {
    let out_width = height;
    let out_height = width;
    let mut output = vec![0; out_width.checked_mul(out_height)?.checked_mul(4)?];
    for src_y in 0..height {
        for src_x in 0..width {
            copy_pixel(
                rgba,
                &mut output,
                width,
                out_width,
                4,
                src_x,
                src_y,
                src_y,
                src_x,
            );
        }
    }
    Some(output)
}

fn transpose_rgba_anti(width: usize, height: usize, rgba: &[u8]) -> Option<Vec<u8>> {
    let out_width = height;
    let out_height = width;
    let mut output = vec![0; out_width.checked_mul(out_height)?.checked_mul(4)?];
    for src_y in 0..height {
        for src_x in 0..width {
            let dst_x = height - 1 - src_y;
            let dst_y = width - 1 - src_x;
            copy_pixel(
                rgba,
                &mut output,
                width,
                out_width,
                4,
                src_x,
                src_y,
                dst_x,
                dst_y,
            );
        }
    }
    Some(output)
}

fn copy_reversed_rgba_row(src: &[u8], dst: &mut [u8]) {
    for (src_pixel, dst_pixel) in src.chunks_exact(4).rev().zip(dst.chunks_exact_mut(4)) {
        dst_pixel.copy_from_slice(src_pixel);
    }
}

#[allow(clippy::too_many_arguments)]
fn copy_pixel(
    src: &[u8],
    dst: &mut [u8],
    src_width: usize,
    dst_width: usize,
    bpp: usize,
    src_x: usize,
    src_y: usize,
    dst_x: usize,
    dst_y: usize,
) {
    let src_offset = (src_y * src_width + src_x) * bpp;
    let dst_offset = (dst_y * dst_width + dst_x) * bpp;
    dst[dst_offset..dst_offset + bpp].copy_from_slice(&src[src_offset..src_offset + bpp]);
}

fn orientation_swaps_dimensions(orientation: Orientation) -> bool {
    matches!(
        orientation,
        Orientation::Rotate90
            | Orientation::Rotate270
            | Orientation::Rotate90FlipH
            | Orientation::Rotate270FlipH
    )
}

#[cfg(test)]
mod tests {
    use super::{
        orient_color_image, orientation_swaps_dimensions, orientation_transform, transform_pixels,
    };
    use crate::core::gpu_effect::color_image_to_rgba;
    use egui::{Color32, ColorImage};
    use image::metadata::Orientation;

    const ALL_ORIENTATIONS: [Orientation; 8] = [
        Orientation::NoTransforms,
        Orientation::Rotate90,
        Orientation::Rotate180,
        Orientation::Rotate270,
        Orientation::FlipHorizontal,
        Orientation::FlipVertical,
        Orientation::Rotate90FlipH,
        Orientation::Rotate270FlipH,
    ];

    #[test]
    fn exif_orientation_swaps_and_rotates_pixels() {
        let image = ColorImage::new(
            [2, 1],
            vec![Color32::from_rgb(10, 20, 30), Color32::from_rgb(200, 0, 10)],
        );

        let output = orient_color_image(&image, Orientation::Rotate90);

        assert_eq!(output.size, [1, 2]);
        assert_eq!(output.pixels[0], Color32::from_rgb(10, 20, 30));
        assert_eq!(output.pixels[1], Color32::from_rgb(200, 0, 10));
        assert!(orientation_swaps_dimensions(Orientation::Rotate90));
        assert!(orientation_swaps_dimensions(Orientation::Rotate270));
        assert!(!orientation_swaps_dimensions(Orientation::Rotate180));
    }

    #[test]
    fn rgba_orientation_matches_color_image_reference() {
        let image = ColorImage::new(
            [2, 3],
            vec![
                Color32::from_rgb(10, 0, 0),
                Color32::from_rgb(20, 0, 0),
                Color32::from_rgb(30, 0, 0),
                Color32::from_rgb(40, 0, 0),
                Color32::from_rgb(50, 0, 0),
                Color32::from_rgb(60, 0, 0),
            ],
        );
        let rgba = color_image_to_rgba(&image);

        for orientation in ALL_ORIENTATIONS {
            let reference = orient_color_image(&image, orientation);
            let (size, oriented_rgba) =
                transform_pixels(image.size, &rgba, 4, orientation_transform(orientation))
                    .expect("valid RGBA test image should transform");

            assert_eq!(size, reference.size, "{orientation:?}");
            assert_eq!(
                oriented_rgba,
                color_image_to_rgba(&reference),
                "{orientation:?}"
            );
        }
    }

    #[test]
    fn luma_orientation_matches_expanded_rgba_transform() {
        // Rotating a 1-byte/px luma buffer must equal expanding to gray-triplet RGBA and rotating
        // that (then the shared channel matches). Verified against the RGBA reference transform for
        // every orientation and both square-swapping and non-swapping shapes.
        let size = [3usize, 2usize];
        let luma: Vec<u8> = (0..(size[0] * size[1]) as u8).map(|v| v * 17).collect();
        let rgba: Vec<u8> = luma.iter().flat_map(|&g| [g, g, g, 255]).collect();

        for orientation in ALL_ORIENTATIONS {
            let transform = orientation_transform(orientation);
            let (luma_size, luma_out) =
                transform_pixels(size, &luma, 1, transform).expect("luma transform");
            let (rgba_size, rgba_out) =
                transform_pixels(size, &rgba, 4, transform).expect("rgba transform");

            assert_eq!(luma_size, rgba_size, "{orientation:?}");
            let expanded: Vec<u8> = luma_out.iter().flat_map(|&g| [g, g, g, 255]).collect();
            assert_eq!(expanded, rgba_out, "{orientation:?}");
        }
    }
}
