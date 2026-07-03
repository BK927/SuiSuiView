use super::metadata::apply_embedded_icc_to_rgba;
use super::{
    clamp_target_long_edge, display_dimensions_with_upscale, image_filter_type, image_reader,
    prepared_page_from_luma, prepared_page_from_rgba, reject_oversized_original, resize_luma,
    resize_rgba, DecodeBackend, DecodeOptions, PreparedPage,
};
use image::DynamicImage;

pub(super) fn prepare_image_with_image_crate(
    bytes: &[u8],
    target_long_edge: u32,
    options: DecodeOptions,
) -> Result<PreparedPage, String> {
    prepare_image_with_image_crate_and_icc(
        bytes,
        target_long_edge,
        options,
        options.allow_display_upscale,
        None,
    )
}

pub(super) fn prepare_image_with_image_crate_and_icc(
    bytes: &[u8],
    target_long_edge: u32,
    options: DecodeOptions,
    allow_display_upscale: bool,
    icc_profile: Option<&[u8]>,
) -> Result<PreparedPage, String> {
    let target_long_edge = clamp_target_long_edge(target_long_edge);
    let (width, height) = image_reader(bytes)?
        .into_dimensions()
        .map_err(|error| error.to_string())?;
    reject_oversized_original(width, height)?;

    let image = image_reader(bytes)?
        .decode()
        .map_err(|error| error.to_string())?;
    let (display_width, display_height) =
        display_dimensions_with_upscale(width, height, target_long_edge, allow_display_upscale)?;
    let resize_filter = options.scale_filter_for(width, height, display_width, display_height);

    // Retain grayscale (Luma8) images as 1 byte/px, but only when no ICC transform applies: the
    // lcms path below operates on RGBA and would otherwise be skipped, changing the pixels for
    // color-managed gray images. Resizing goes through `resize_luma`, which mirrors the RGBA fast
    // resizer so the on-screen result is unchanged.
    if icc_profile.is_none() {
        if let DynamicImage::ImageLuma8(luma) = &image {
            let display = if display_width == width && display_height == height {
                luma.clone()
            } else {
                resize_luma(luma, display_width, display_height, resize_filter)
            };
            return prepared_page_from_luma(
                display.into_raw(),
                width,
                height,
                display_width,
                display_height,
                target_long_edge,
                DecodeBackend::ImageCrate,
            );
        }
    }

    let display_rgba = if display_width == width && display_height == height {
        image.into_rgba8()
    } else if should_resize_before_rgba(&image) {
        image
            .resize_exact(
                display_width,
                display_height,
                image_filter_type(resize_filter),
            )
            .into_rgba8()
    } else {
        let rgba = image.into_rgba8();
        resize_rgba(&rgba, display_width, display_height, resize_filter)
    };
    let mut raw = display_rgba.into_raw();
    let mut notice = None;
    if let Some(profile) = icc_profile {
        if let Err(error) = apply_embedded_icc_to_rgba(&mut raw, profile) {
            notice = Some(format!(
                "ICC profile could not be applied; assuming sRGB: {error}"
            ));
        }
    }

    let mut page = prepared_page_from_rgba(
        raw,
        width,
        height,
        display_width,
        display_height,
        target_long_edge,
        DecodeBackend::ImageCrate,
    )?;
    page.notice = notice;
    Ok(page)
}

fn should_resize_before_rgba(image: &DynamicImage) -> bool {
    matches!(
        image,
        DynamicImage::ImageLuma8(_)
            | DynamicImage::ImageLumaA8(_)
            | DynamicImage::ImageRgb8(_)
            | DynamicImage::ImageRgba8(_)
    )
}

#[cfg(test)]
mod tests {
    use super::should_resize_before_rgba;
    use image::{DynamicImage, ImageBuffer, Rgb};

    #[test]
    fn resize_before_rgba_is_limited_to_8_bit_images() {
        let rgb8 = DynamicImage::ImageRgb8(ImageBuffer::new(2, 2));
        let rgba8 = DynamicImage::ImageRgba8(ImageBuffer::new(2, 2));
        let rgb16 = DynamicImage::ImageRgb16(ImageBuffer::from_pixel(2, 2, Rgb([1, 2, 3])));
        let rgba32f = DynamicImage::ImageRgba32F(ImageBuffer::from_pixel(
            2,
            2,
            image::Rgba([0.0, 0.1, 0.2, 1.0]),
        ));

        assert!(should_resize_before_rgba(&rgb8));
        assert!(should_resize_before_rgba(&rgba8));
        assert!(!should_resize_before_rgba(&rgb16));
        assert!(!should_resize_before_rgba(&rgba32f));
    }
}
