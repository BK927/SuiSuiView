use crate::core::state::ResizeFilter;
use fast_image_resize::{
    images::{Image as FastImage, ImageRef as FastImageRef},
    FilterType as FastFilterType, PixelType as FastPixelType, ResizeAlg as FastResizeAlg,
    ResizeOptions as FastResizeOptions, Resizer as FastResizer,
};
use image::{imageops::FilterType as ImageFilterType, RgbaImage};

pub(super) fn resize_rgba(
    image: &RgbaImage,
    width: u32,
    height: u32,
    resize_filter: ResizeFilter,
) -> RgbaImage {
    resize_rgba_fast(image, width, height, resize_filter).unwrap_or_else(|| {
        image::imageops::resize(image, width, height, image_filter_type(resize_filter))
    })
}

fn resize_rgba_fast(
    image: &RgbaImage,
    width: u32,
    height: u32,
    resize_filter: ResizeFilter,
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

pub(super) fn image_filter_type(resize_filter: ResizeFilter) -> ImageFilterType {
    match resize_filter {
        ResizeFilter::Bicubic => ImageFilterType::CatmullRom,
        ResizeFilter::Lanczos3 => ImageFilterType::Lanczos3,
        ResizeFilter::FastTriangle => ImageFilterType::Triangle,
        ResizeFilter::Nearest => ImageFilterType::Nearest,
    }
}

fn fast_resize_alg(resize_filter: ResizeFilter) -> FastResizeAlg {
    match resize_filter {
        ResizeFilter::Bicubic => FastResizeAlg::Convolution(FastFilterType::CatmullRom),
        ResizeFilter::Lanczos3 => FastResizeAlg::Convolution(FastFilterType::Lanczos3),
        ResizeFilter::FastTriangle => FastResizeAlg::Convolution(FastFilterType::Bilinear),
        ResizeFilter::Nearest => FastResizeAlg::Nearest,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        fast_resize_alg, image_filter_type, resize_rgba_fast, FastFilterType, FastResizeAlg,
    };
    use crate::core::state::ResizeFilter;
    use image::{Rgba, RgbaImage};

    #[test]
    fn resize_filters_map_to_image_filters() {
        assert_eq!(
            image_filter_type(ResizeFilter::Bicubic),
            image::imageops::FilterType::CatmullRom
        );
        assert_eq!(
            image_filter_type(ResizeFilter::Lanczos3),
            image::imageops::FilterType::Lanczos3
        );
        assert_eq!(
            image_filter_type(ResizeFilter::FastTriangle),
            image::imageops::FilterType::Triangle
        );
        assert_eq!(
            image_filter_type(ResizeFilter::Nearest),
            image::imageops::FilterType::Nearest
        );
    }

    #[test]
    fn resize_filters_map_to_fast_resize_algorithms() {
        assert!(matches!(
            fast_resize_alg(ResizeFilter::Bicubic),
            FastResizeAlg::Convolution(FastFilterType::CatmullRom)
        ));
        assert!(matches!(
            fast_resize_alg(ResizeFilter::Lanczos3),
            FastResizeAlg::Convolution(FastFilterType::Lanczos3)
        ));
        assert!(matches!(
            fast_resize_alg(ResizeFilter::FastTriangle),
            FastResizeAlg::Convolution(FastFilterType::Bilinear)
        ));
        assert!(matches!(
            fast_resize_alg(ResizeFilter::Nearest),
            FastResizeAlg::Nearest
        ));
    }

    #[test]
    fn fast_rgba_resize_returns_requested_size() {
        let mut image = RgbaImage::new(64, 32);
        for (x, y, pixel) in image.enumerate_pixels_mut() {
            *pixel = Rgba([x as u8, y as u8, 128, 255]);
        }

        let resized = resize_rgba_fast(&image, 16, 8, ResizeFilter::Bicubic).unwrap();

        assert_eq!(resized.dimensions(), (16, 8));
        assert_eq!(resized.as_raw().len(), 16 * 8 * 4);
    }
}
