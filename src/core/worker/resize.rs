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

    /// Manual measurement harness for the CPU downscale filter decision (run
    /// with `--ignored` and SUISUIVIEW_BENCH_CBZ pointing at a webtoon CBZ):
    /// times every CpuScaleFilter over real pages at reading-relevant ratios
    /// and reports each filter's SSIM against the Lanczos3 pivot.
    #[test]
    #[ignore = "manual bench; needs SUISUIVIEW_BENCH_CBZ"]
    fn bench_cpu_downscale_filters() {
        use crate::core::upscale_quality::compare_images;
        use egui::ColorImage;
        use std::io::Read as _;
        use std::time::Instant;

        let Ok(cbz) = std::env::var("SUISUIVIEW_BENCH_CBZ") else {
            eprintln!("SUISUIVIEW_BENCH_CBZ not set; skipping");
            return;
        };
        let file = std::fs::File::open(&cbz).expect("open cbz");
        let mut archive = zip::ZipArchive::new(file).expect("read cbz");
        let mut names: Vec<String> = (0..archive.len())
            .map(|i| archive.by_index(i).unwrap().name().to_owned())
            .filter(|n| n.ends_with(".jpg") || n.ends_with(".png") || n.ends_with(".webp"))
            .collect();
        names.sort();
        let picks: Vec<&String> = [0usize, 8, 20, 40, 60, 80]
            .iter()
            .filter_map(|&i| names.get(i))
            .collect();
        let mut pages: Vec<RgbaImage> = Vec::new();
        for name in picks {
            let mut entry = archive.by_name(name).expect("entry");
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).expect("read entry");
            let img = image::load_from_memory(&bytes).expect("decode").to_rgba8();
            pages.push(img);
        }
        eprintln!("pages: {} loaded from {cbz}", pages.len());

        let ratios = [0.85f32, 0.6, 0.35];
        let to_color = |img: &RgbaImage| {
            ColorImage::from_rgba_unmultiplied(
                [img.width() as usize, img.height() as usize],
                img.as_raw(),
            )
        };

        // Pivot outputs (Lanczos3) per (page, ratio).
        let mut pivots: Vec<Vec<ColorImage>> = Vec::new();
        for page in &pages {
            let mut per_ratio = Vec::new();
            for &ratio in &ratios {
                let w = ((page.width() as f32 * ratio) as u32).max(1);
                let h = ((page.height() as f32 * ratio) as u32).max(1);
                per_ratio.push(to_color(&super::resize_rgba(
                    page,
                    w,
                    h,
                    CpuScaleFilter::Lanczos3,
                )));
            }
            pivots.push(per_ratio);
        }

        eprintln!("filter        total_ms   min_ssim_vs_L3(0.85/0.6/0.35)");
        for filter in CpuScaleFilter::ALL {
            // Timing: best of 3 full sweeps (all pages x all ratios).
            let mut best_ms = f64::MAX;
            for _ in 0..3 {
                let started = Instant::now();
                for page in &pages {
                    for &ratio in &ratios {
                        let w = ((page.width() as f32 * ratio) as u32).max(1);
                        let h = ((page.height() as f32 * ratio) as u32).max(1);
                        std::hint::black_box(super::resize_rgba(page, w, h, filter));
                    }
                }
                best_ms = best_ms.min(started.elapsed().as_secs_f64() * 1000.0);
            }
            // Quality: min SSIM vs the Lanczos3 pivot per ratio, across pages.
            let mut min_ssim = [1.0f64; 3];
            for (page_idx, page) in pages.iter().enumerate() {
                for (ratio_idx, &ratio) in ratios.iter().enumerate() {
                    let w = ((page.width() as f32 * ratio) as u32).max(1);
                    let h = ((page.height() as f32 * ratio) as u32).max(1);
                    let out = to_color(&super::resize_rgba(page, w, h, filter));
                    let metrics =
                        compare_images(&pivots[page_idx][ratio_idx], &out).expect("compare");
                    min_ssim[ratio_idx] = min_ssim[ratio_idx].min(metrics.ssim);
                }
            }
            eprintln!(
                "{:<12} {:>8.1}   {:.4} / {:.4} / {:.4}",
                filter.label(),
                best_ms,
                min_ssim[0],
                min_ssim[1],
                min_ssim[2]
            );
        }
    }
}
