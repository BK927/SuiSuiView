use crate::core::state::CpuScaleFilter;
use fast_image_resize::{
    images::{Image as FastImage, ImageRef as FastImageRef},
    Filter as FastFilter, FilterType as FastFilterType, PixelType as FastPixelType,
    ResizeAlg as FastResizeAlg, ResizeOptions as FastResizeOptions, Resizer as FastResizer,
};
use image::{imageops::FilterType as ImageFilterType, RgbaImage};
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
        fast_filter_type, fast_resize_alg, image_filter_type, resize_rgba_fast, FastFilterType,
        FastResizeAlg,
    };
    use crate::core::state::CpuScaleFilter;
    use image::{Rgba, RgbaImage};

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
}
