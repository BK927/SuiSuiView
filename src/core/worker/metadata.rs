use super::{image_reader, jpeg, retained_page_byte_size, PreparedPage};
#[cfg(test)]
use eframe::egui::ColorImage;
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
    debug_assert_eq!(page.rgba.len(), source_size[0] * source_size[1] * 4);
    let Some((output_size, output_rgba)) =
        transform_rgba_pixels(source_size, &page.rgba, transform)
    else {
        return page;
    };
    let rgba = Arc::<[u8]>::from(output_rgba.into_boxed_slice());
    page.byte_size = retained_page_byte_size(rgba.len());
    page.rgba = rgba;
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

fn transform_rgba_pixels(
    size: [usize; 2],
    rgba: &[u8],
    transform: OrientationTransform,
) -> Option<([usize; 2], Vec<u8>)> {
    let [width, height] = size;
    if rgba.len() != width.checked_mul(height)?.checked_mul(4)? {
        return None;
    }

    if transform == OrientationTransform::default() {
        return Some((size, rgba.to_vec()));
    }

    let rotation = transform.rotation_quadrants % 4;
    let output_size = if rotation % 2 == 1 {
        [height, width]
    } else {
        [width, height]
    };
    let [out_width, out_height] = output_size;
    let mut output = vec![0; out_width * out_height * 4];
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
            let src_offset = (src_y * width + src_x) * 4;
            let dst_offset = (dst_y * out_width + dst_x) * 4;
            output[dst_offset..dst_offset + 4].copy_from_slice(&rgba[src_offset..src_offset + 4]);
        }
    }
    Some((output_size, output))
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
        orient_color_image, orientation_swaps_dimensions, orientation_transform,
        transform_rgba_pixels,
    };
    use crate::core::gpu_effect::color_image_to_rgba;
    use eframe::egui::{Color32, ColorImage};
    use image::metadata::Orientation;

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

        for orientation in [
            Orientation::NoTransforms,
            Orientation::Rotate90,
            Orientation::Rotate180,
            Orientation::Rotate270,
            Orientation::FlipHorizontal,
            Orientation::FlipVertical,
            Orientation::Rotate90FlipH,
            Orientation::Rotate270FlipH,
        ] {
            let reference = orient_color_image(&image, orientation);
            let (size, oriented_rgba) =
                transform_rgba_pixels(image.size, &rgba, orientation_transform(orientation))
                    .expect("valid RGBA test image should transform");

            assert_eq!(size, reference.size, "{orientation:?}");
            assert_eq!(
                oriented_rgba,
                color_image_to_rgba(&reference),
                "{orientation:?}"
            );
        }
    }
}
