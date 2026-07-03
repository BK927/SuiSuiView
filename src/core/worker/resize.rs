use crate::core::state::CpuScaleFilter;
use fast_image_resize::{
    images::{Image as FastImage, ImageRef as FastImageRef},
    Filter as FastFilter, FilterType as FastFilterType, PixelType as FastPixelType,
    ResizeAlg as FastResizeAlg, ResizeOptions as FastResizeOptions, Resizer as FastResizer,
};
use image::{imageops::FilterType as ImageFilterType, GrayImage, RgbaImage};
use std::f64::consts::PI;

pub(super) fn resize_rgba(
    image: &RgbaImage,
    width: u32,
    height: u32,
    resize_filter: CpuScaleFilter,
) -> RgbaImage {
    resize_rgba_fast(image, width, height, resize_filter).unwrap_or_else(|| {
        image::imageops::resize(image, width, height, image_filter_type(resize_filter))
    })
}

/// Resize a single-channel gray image, producing pixels bit-identical to expanding the input to
/// gray-triplet RGBA, running [`resize_rgba`], and taking any one channel. The `fast_image_resize`
/// backend is compiled with `only_u8x4`, so its dynamic resizer rejects the `U8` pixel type; to
/// keep the luma result identical to the RGBA hot path (visual-no-change guarantee) we expand to
/// RGBA, reuse the exact same resizer, then collapse back to one channel. This runs once per page
/// at decode time; the retained cache stays 1 byte/px.
pub(super) fn resize_luma(
    image: &GrayImage,
    width: u32,
    height: u32,
    resize_filter: CpuScaleFilter,
) -> GrayImage {
    resize_luma_via_rgba(image, width, height, resize_filter).unwrap_or_else(|| {
        image::imageops::resize(image, width, height, image_filter_type(resize_filter))
    })
}

fn resize_luma_via_rgba(
    image: &GrayImage,
    width: u32,
    height: u32,
    resize_filter: CpuScaleFilter,
) -> Option<GrayImage> {
    let mut rgba = Vec::with_capacity((image.as_raw().len()).checked_mul(4)?);
    for &gray in image.as_raw() {
        rgba.extend_from_slice(&[gray, gray, gray, 255]);
    }
    let rgba = RgbaImage::from_raw(image.width(), image.height(), rgba)?;
    let resized = resize_rgba_fast(&rgba, width, height, resize_filter)?;
    let gray: Vec<u8> = resized.as_raw().chunks_exact(4).map(|px| px[0]).collect();
    GrayImage::from_raw(width, height, gray)
}

fn resize_rgba_fast(
    image: &RgbaImage,
    width: u32,
    height: u32,
    resize_filter: CpuScaleFilter,
) -> Option<RgbaImage> {
    let source = FastImageRef::new(
        image.width(),
        image.height(),
        image.as_raw(),
        FastPixelType::U8x4,
    )
    .ok()?;
    let mut destination = FastImage::new(width, height, FastPixelType::U8x4);
    let options = FastResizeOptions::new()
        .resize_alg(fast_resize_alg(resize_filter))
        // Match image::imageops behavior by resizing RGBA channels directly.
        .use_alpha(false);

    FastResizer::new()
        .resize(&source, &mut destination, Some(&options))
        .ok()?;

    RgbaImage::from_raw(width, height, destination.into_vec())
}

pub(super) fn image_filter_type(resize_filter: CpuScaleFilter) -> ImageFilterType {
    match resize_filter {
        CpuScaleFilter::Nearest => ImageFilterType::Nearest,
        CpuScaleFilter::Box | CpuScaleFilter::Bilinear => ImageFilterType::Triangle,
        CpuScaleFilter::Hamming | CpuScaleFilter::CatmullRom | CpuScaleFilter::Mitchell => {
            ImageFilterType::CatmullRom
        }
        CpuScaleFilter::Gaussian => ImageFilterType::Gaussian,
        CpuScaleFilter::Lanczos2 | CpuScaleFilter::Lanczos3 => ImageFilterType::Lanczos3,
    }
}

fn fast_resize_alg(resize_filter: CpuScaleFilter) -> FastResizeAlg {
    match resize_filter {
        CpuScaleFilter::Nearest => FastResizeAlg::Nearest,
        _ => FastResizeAlg::Convolution(fast_filter_type(resize_filter)),
    }
}

fn fast_filter_type(resize_filter: CpuScaleFilter) -> FastFilterType {
    match resize_filter {
        CpuScaleFilter::Nearest => FastFilterType::Box,
        CpuScaleFilter::Box => FastFilterType::Box,
        CpuScaleFilter::Bilinear => FastFilterType::Bilinear,
        CpuScaleFilter::Hamming => FastFilterType::Hamming,
        CpuScaleFilter::CatmullRom => FastFilterType::CatmullRom,
        CpuScaleFilter::Mitchell => FastFilterType::Mitchell,
        CpuScaleFilter::Gaussian => FastFilterType::Gaussian,
        CpuScaleFilter::Lanczos2 => FastFilterType::Custom(
            FastFilter::new("Lanczos2", lanczos2_filter, 2.0)
                .expect("Lanczos2 support radius should be valid"),
        ),
        CpuScaleFilter::Lanczos3 => FastFilterType::Lanczos3,
    }
}

fn sinc_filter(x: f64) -> f64 {
    if x == 0.0 {
        1.0
    } else {
        let x = x * PI;
        x.sin() / x
    }
}

fn lanczos2_filter(x: f64) -> f64 {
    if (-2.0..2.0).contains(&x) {
        sinc_filter(x) * sinc_filter(x / 2.0)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::{
        fast_filter_type, fast_resize_alg, image_filter_type, resize_luma_via_rgba,
        resize_rgba_fast, FastFilterType, FastResizeAlg,
    };
    use crate::core::state::CpuScaleFilter;
    use image::{GrayImage, Luma, Rgba, RgbaImage};

    #[test]
    fn cpu_scale_filters_map_to_image_filters() {
        assert_eq!(
            image_filter_type(CpuScaleFilter::CatmullRom),
            image::imageops::FilterType::CatmullRom
        );
        assert_eq!(
            image_filter_type(CpuScaleFilter::Lanczos3),
            image::imageops::FilterType::Lanczos3
        );
        assert_eq!(
            image_filter_type(CpuScaleFilter::Bilinear),
            image::imageops::FilterType::Triangle
        );
        assert_eq!(
            image_filter_type(CpuScaleFilter::Nearest),
            image::imageops::FilterType::Nearest
        );
    }

    #[test]
    fn cpu_scale_filters_map_to_fast_resize_algorithms() {
        assert!(matches!(
            fast_resize_alg(CpuScaleFilter::CatmullRom),
            FastResizeAlg::Convolution(FastFilterType::CatmullRom)
        ));
        assert!(matches!(
            fast_resize_alg(CpuScaleFilter::Lanczos3),
            FastResizeAlg::Convolution(FastFilterType::Lanczos3)
        ));
        assert!(matches!(
            fast_resize_alg(CpuScaleFilter::Bilinear),
            FastResizeAlg::Convolution(FastFilterType::Bilinear)
        ));
        assert!(matches!(
            fast_resize_alg(CpuScaleFilter::Nearest),
            FastResizeAlg::Nearest
        ));
    }

    #[test]
    fn lanczos2_uses_custom_fast_resize_filter() {
        let FastFilterType::Custom(filter) = fast_filter_type(CpuScaleFilter::Lanczos2) else {
            panic!("Lanczos2 should use a custom convolution filter");
        };

        assert_eq!(filter.name(), "Lanczos2");
        assert_eq!(filter.support(), 2.0);
    }

    #[test]
    fn fast_rgba_resize_returns_requested_size() {
        let mut image = RgbaImage::new(64, 32);
        for (x, y, pixel) in image.enumerate_pixels_mut() {
            *pixel = Rgba([x as u8, y as u8, 128, 255]);
        }

        let resized = resize_rgba_fast(&image, 16, 8, CpuScaleFilter::Hamming).unwrap();

        assert_eq!(resized.dimensions(), (16, 8));
        assert_eq!(resized.as_raw().len(), 16 * 8 * 4);
    }

    #[test]
    fn fast_luma_resize_returns_requested_size() {
        let mut image = GrayImage::new(64, 32);
        for (x, _y, pixel) in image.enumerate_pixels_mut() {
            *pixel = Luma([x as u8]);
        }

        let resized = resize_luma_via_rgba(&image, 16, 8, CpuScaleFilter::Hamming).unwrap();

        assert_eq!(resized.dimensions(), (16, 8));
        assert_eq!(resized.as_raw().len(), 16 * 8);
    }

    #[test]
    fn luma_resize_matches_rgba_red_channel() {
        // Expanding luma to RGBA then resizing must be bit-identical (on the shared channel) to
        // resizing the luma buffer directly, for every non-nearest filter. The RGBA resizer runs
        // with use_alpha(false) so the gray-triplet channels are convolved independently, exactly
        // like the single luma channel.
        let width = 40u32;
        let height = 24u32;
        let mut luma = GrayImage::new(width, height);
        for (x, y, pixel) in luma.enumerate_pixels_mut() {
            *pixel = Luma([((x * 7 + y * 13) % 256) as u8]);
        }
        let mut rgba = RgbaImage::new(width, height);
        for (x, y, pixel) in rgba.enumerate_pixels_mut() {
            let gray = luma.get_pixel(x, y)[0];
            *pixel = Rgba([gray, gray, gray, 255]);
        }

        for filter in [
            CpuScaleFilter::Hamming,
            CpuScaleFilter::CatmullRom,
            CpuScaleFilter::Bilinear,
            CpuScaleFilter::Lanczos3,
            CpuScaleFilter::Nearest,
        ] {
            for (dst_w, dst_h) in [(20, 12), (13, 9), (64, 40)] {
                let luma_resized = resize_luma_via_rgba(&luma, dst_w, dst_h, filter).unwrap();
                let rgba_resized = resize_rgba_fast(&rgba, dst_w, dst_h, filter).unwrap();
                let expected_red: Vec<u8> = rgba_resized
                    .as_raw()
                    .chunks_exact(4)
                    .map(|px| px[0])
                    .collect();
                assert_eq!(
                    luma_resized.as_raw(),
                    expected_red.as_slice(),
                    "filter {filter:?} to {dst_w}x{dst_h}"
                );
            }
        }
    }
}
