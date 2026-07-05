use super::{
    prepare_image_with_options, DecodeBackend, DecodeOptions, DecodeStrategy, MAX_TARGET_LONG_EDGE,
};
use crate::core::state::{DecoderPreference, DecoderPreferences};
use image::{
    DynamicImage, GrayAlphaImage, GrayImage, ImageFormat, LumaA, RgbImage, Rgba, RgbaImage,
};
use std::io::Cursor;

fn gray_value(x: u32, y: u32) -> u8 {
    ((x * 3 + y * 7) % 256) as u8
}

fn gray_luma_image(width: u32, height: u32) -> GrayImage {
    GrayImage::from_fn(width, height, |x, y| image::Luma([gray_value(x, y)]))
}

/// The same gray content as `gray_luma_image`, but as an RGB image with R=G=B. Lets a
/// grayscale fixture be re-encoded as a *color* PNG so the Default decode path takes the RGBA
/// row writer with the identical `sampled_index_map`, giving an algorithm-matched reference.
fn gray_as_rgb_image(width: u32, height: u32) -> RgbImage {
    RgbImage::from_fn(width, height, |x, y| {
        let g = gray_value(x, y);
        image::Rgb([g, g, g])
    })
}

/// Assert that `color_image()`'s direct-gray construction is bit-identical to expanding the
/// retained pixels to RGBA and building the ColorImage that way — the guarantee that keeps the
/// on-screen result unchanged regardless of how pixels are retained.
fn assert_color_image_matches_expanded_rgba(page: &super::PreparedPage) {
    use egui::ColorImage;
    let size = page.image_size();
    let expanded = page.pixels.to_rgba_vec(size[0], size[1]);
    assert_eq!(
        page.color_image(),
        ColorImage::from_rgba_unmultiplied(size, &expanded)
    );
}

fn encode(image: DynamicImage, format: ImageFormat) -> Vec<u8> {
    let mut bytes = Vec::new();
    image
        .write_to(&mut Cursor::new(&mut bytes), format)
        .expect("encode fixture");
    bytes
}

fn options_with_png(pref: DecoderPreference) -> DecodeOptions {
    DecodeOptions {
        strategy: DecodeStrategy::Auto,
        decoder_preferences: DecoderPreferences {
            png: pref,
            ..DecoderPreferences::default()
        },
        ..DecodeOptions::default()
    }
}

fn options_with_jpeg(pref: DecoderPreference) -> DecodeOptions {
    DecodeOptions {
        strategy: DecodeStrategy::Auto,
        decoder_preferences: DecoderPreferences {
            jpeg: pref,
            ..DecoderPreferences::default()
        },
        ..DecodeOptions::default()
    }
}

#[test]
fn sampled_gray_png_is_retained_as_luma_and_matches_rgba_reference() {
    // Large grayscale PNG -> the row sampler retains luma. The algorithm-matched reference is
    // the SAME content re-encoded as a color (RGB, R=G=B) PNG, which takes the RGBA row
    // sampler with the identical index maps — so the ColorImages must be bit-identical.
    let side = 2600u32;
    let gray_bytes = encode(
        DynamicImage::ImageLuma8(gray_luma_image(side, side)),
        ImageFormat::Png,
    );
    let rgb_bytes = encode(
        DynamicImage::ImageRgb8(gray_as_rgb_image(side, side)),
        ImageFormat::Png,
    );

    let luma = prepare_image_with_options(
        &gray_bytes,
        1024,
        options_with_png(DecoderPreference::Default),
    )
    .expect("luma png");
    assert!(luma.pixels.is_luma(), "backend {:?}", luma.decode_backend);
    assert_eq!(luma.decode_backend, DecodeBackend::PngSampled);
    assert_eq!(luma.byte_size, luma.display_width * luma.display_height);
    assert_color_image_matches_expanded_rgba(&luma);

    let reference = prepare_image_with_options(
        &rgb_bytes,
        1024,
        options_with_png(DecoderPreference::Default),
    )
    .expect("rgba png");
    assert!(!reference.pixels.is_luma());
    assert_eq!(reference.decode_backend, DecodeBackend::PngSampled);
    assert_eq!(luma.image_size(), reference.image_size());
    assert_eq!(luma.color_image(), reference.color_image());
}

#[test]
fn original_inspection_gray_png_is_retained_as_luma() {
    // Original-inspection target (> MAX_TARGET_LONG_EDGE) exercises the exact-rows luma writer.
    // Reference is the same content as a color PNG through the RGBA exact-rows writer.
    let side = 64u32;
    let gray_bytes = encode(
        DynamicImage::ImageLuma8(gray_luma_image(side, side)),
        ImageFormat::Png,
    );
    let rgb_bytes = encode(
        DynamicImage::ImageRgb8(gray_as_rgb_image(side, side)),
        ImageFormat::Png,
    );
    let target = MAX_TARGET_LONG_EDGE + 1;

    let luma = prepare_image_with_options(
        &gray_bytes,
        target,
        options_with_png(DecoderPreference::Default),
    )
    .expect("luma png");
    assert!(luma.pixels.is_luma());
    assert_eq!(luma.decode_backend, DecodeBackend::PngExactRows);
    assert_eq!(luma.byte_size, (side * side) as usize);
    assert_color_image_matches_expanded_rgba(&luma);

    let reference = prepare_image_with_options(
        &rgb_bytes,
        target,
        options_with_png(DecoderPreference::Default),
    )
    .expect("rgba png");
    assert_eq!(reference.decode_backend, DecodeBackend::PngExactRows);
    assert_eq!(luma.color_image(), reference.color_image());
}

#[test]
fn grayscale_alpha_png_stays_rgba() {
    // GrayscaleAlpha carries a real alpha channel and must not collapse to single-channel luma.
    let side = 2600u32;
    let image = GrayAlphaImage::from_fn(side, side, |x, y| LumaA([gray_value(x, y), 128]));
    let bytes = encode(DynamicImage::ImageLumaA8(image), ImageFormat::Png);

    let page =
        prepare_image_with_options(&bytes, 1024, options_with_png(DecoderPreference::Default))
            .expect("gray-alpha png");
    assert!(!page.pixels.is_luma());
    assert_eq!(page.byte_size, page.display_width * page.display_height * 4);
}

#[test]
fn scaled_gray_jpeg_is_retained_as_luma() {
    // Large grayscale JPEG -> the scaled decoder reports L8 and we retain luma. JPEG is lossy
    // and a grayscale JPEG has no chroma planes, so there is no algorithm-matched RGBA fixture
    // to diff against; instead we assert the retention (is_luma + byte_size) and that
    // color_image() is bit-identical to expanding the retained luma to RGBA.
    let side = 2304u32;
    let bytes = encode(
        DynamicImage::ImageLuma8(gray_luma_image(side, side)),
        ImageFormat::Jpeg,
    );

    let luma =
        prepare_image_with_options(&bytes, 1024, options_with_jpeg(DecoderPreference::Default))
            .expect("luma jpeg");
    assert!(luma.pixels.is_luma(), "backend {:?}", luma.decode_backend);
    assert_eq!(luma.decode_backend, DecodeBackend::JpegScaled);
    assert_eq!(luma.byte_size, luma.display_width * luma.display_height);
    assert_color_image_matches_expanded_rgba(&luma);
}

#[test]
fn image_crate_gray_is_retained_as_luma() {
    // Forcing the image crate on a grayscale PNG yields DynamicImage::ImageLuma8 -> retained as
    // luma. The reference is the same gray values expanded to RGBA by hand.
    let side = 40u32;
    let bytes = encode(
        DynamicImage::ImageLuma8(gray_luma_image(side, side)),
        ImageFormat::Png,
    );

    let luma = prepare_image_with_options(
        &bytes,
        1024,
        options_with_png(DecoderPreference::ImageCrate),
    )
    .expect("luma png via image crate");
    assert!(luma.pixels.is_luma(), "backend {:?}", luma.decode_backend);
    assert_eq!(luma.decode_backend, DecodeBackend::ImageCrate);
    assert_eq!(luma.byte_size, (side * side) as usize);
    assert_color_image_matches_expanded_rgba(&luma);

    let expected: RgbaImage = RgbaImage::from_fn(side, side, |x, y| {
        let g = gray_value(x, y);
        Rgba([g, g, g, 255])
    });
    assert_eq!(
        luma.pixels.to_rgba_vec(side as usize, side as usize),
        expected.into_raw()
    );
}

#[test]
fn icc_gray_image_stays_rgba() {
    // When an ICC profile is present and ICC application is enabled, the gray image is routed
    // through the ICC (RGBA) path and must NOT be retained as luma — the lcms transform would
    // otherwise be skipped and change the pixels.
    let side = 40u32;
    let mut luma = gray_luma_image(side, side);
    let png_with_icc = encode_gray_png_with_srgb_icc(&mut luma);

    let page = prepare_image_with_options(
        &png_with_icc,
        1024,
        DecodeOptions {
            strategy: DecodeStrategy::ImageCrate,
            apply_embedded_icc: true,
            ..DecodeOptions::default()
        },
    )
    .expect("icc gray png");
    assert!(!page.pixels.is_luma());
    assert_eq!(page.byte_size, page.display_width * page.display_height * 4);
}

#[test]
fn luma_page_reports_quarter_byte_size_of_equivalent_rgba() {
    // Cache-budget accounting keys off `byte_size`. A grayscale page retained as luma must cost
    // exactly a quarter of the same-dimensioned color page, so the eviction math stays honest.
    let side = 40u32;
    let gray_png = encode(
        DynamicImage::ImageLuma8(gray_luma_image(side, side)),
        ImageFormat::Png,
    );
    let color_png = encode(
        DynamicImage::ImageRgb8(gray_as_rgb_image(side, side)),
        ImageFormat::Png,
    );

    let luma = prepare_image_with_options(
        &gray_png,
        1024,
        options_with_png(DecoderPreference::ImageCrate),
    )
    .expect("luma png");
    let rgba = prepare_image_with_options(
        &color_png,
        1024,
        options_with_png(DecoderPreference::ImageCrate),
    )
    .expect("rgba png");

    assert!(luma.pixels.is_luma());
    assert!(!rgba.pixels.is_luma());
    assert_eq!(luma.image_size(), rgba.image_size());
    assert_eq!(rgba.byte_size, luma.byte_size * 4);
    assert_eq!(luma.byte_size, (side * side) as usize);
}

#[test]
fn color_image_crate_source_stays_rgba() {
    // A genuinely colored image decoded via the image crate must remain RGBA.
    let side = 40u32;
    let image = RgbImage::from_fn(side, side, |x, y| {
        image::Rgb([(x % 256) as u8, (y % 256) as u8, 200])
    });
    let bytes = encode(DynamicImage::ImageRgb8(image), ImageFormat::Png);

    let page = prepare_image_with_options(
        &bytes,
        1024,
        options_with_png(DecoderPreference::ImageCrate),
    )
    .expect("rgb png");
    assert!(!page.pixels.is_luma());
    assert_eq!(page.byte_size, page.display_width * page.display_height * 4);
}

/// Encode a grayscale PNG that carries an sRGB ICC profile chunk, so the ICC-application path
/// is exercised. Uses the `png` crate directly since `image`'s encoder does not attach ICC.
fn encode_gray_png_with_srgb_icc(image: &mut GrayImage) -> Vec<u8> {
    let srgb = lcms2::Profile::new_srgb();
    let icc = srgb.icc().expect("serialize sRGB ICC profile");

    let mut info = png::Info::with_size(image.width(), image.height());
    info.color_type = png::ColorType::Grayscale;
    info.bit_depth = png::BitDepth::Eight;
    info.icc_profile = Some(std::borrow::Cow::Owned(icc));

    let mut bytes = Vec::new();
    {
        let encoder = png::Encoder::with_info(&mut bytes, info).expect("png encoder with ICC info");
        let mut writer = encoder.write_header().expect("png header");
        writer.write_image_data(image.as_raw()).expect("png data");
    }
    bytes
}
