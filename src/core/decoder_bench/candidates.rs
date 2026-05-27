#[cfg(feature = "bench-native-wuffs")]
use super::wuffs;
use super::{animated, DeferredCandidate};
use crate::core::decoder_backend;
use image::ImageFormat;
use jpeg_decoder::{Decoder as JpegDecoder, PixelFormat as JpegPixelFormat};
use std::io::Cursor;

const MAX_BENCH_DIMENSION: usize = 20_000;
const MAX_ANIMATION_RGBA_BYTES: usize = 512 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BenchFormat {
    Jpeg,
    Png,
    Webp,
    Gif,
    Avif,
    Svg,
    Bmp,
    Ico,
}

impl BenchFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Jpeg => "jpeg",
            Self::Png => "png",
            Self::Webp => "webp",
            Self::Gif => "gif",
            Self::Avif => "avif",
            Self::Svg => "svg",
            Self::Bmp => "bmp",
            Self::Ico => "ico",
        }
    }

    pub fn exact_reference(self) -> bool {
        matches!(self, Self::Png | Self::Gif | Self::Bmp | Self::Ico)
    }

    pub fn allows_checksum_consensus(self) -> bool {
        matches!(self, Self::Png)
    }
}

pub struct CandidateDecoder {
    pub name: &'static str,
    pub note: &'static str,
    pub output_pixel_format: &'static str,
    pub allocation_note: &'static str,
    pub decode: fn(&[u8]) -> Result<DecodedImage, String>,
}

pub struct DecodedImage {
    pub width: u32,
    pub height: u32,
    pub frames_decoded: u32,
    pub total_duration_ms: u64,
    pub pixels: Vec<u8>,
}

impl DecodedImage {
    pub(super) fn still(width: u32, height: u32, pixels: Vec<u8>) -> Self {
        Self {
            width,
            height,
            frames_decoded: 1,
            total_duration_ms: 0,
            pixels,
        }
    }

    pub(super) fn animation(
        width: u32,
        height: u32,
        frames_decoded: u32,
        total_duration_ms: u64,
        pixels: Vec<u8>,
    ) -> Result<Self, String> {
        let expected_len = checked_animation_rgba_len(width, height, frames_decoded)?;
        expect_len(pixels.len(), expected_len, "animation RGBA")?;
        Ok(Self {
            width,
            height,
            frames_decoded,
            total_duration_ms,
            pixels,
        })
    }

    pub fn decoded_pixels(&self) -> u64 {
        u64::from(self.width) * u64::from(self.height) * u64::from(self.frames_decoded.max(1))
    }
}

pub fn detect_format(bytes: &[u8]) -> Option<BenchFormat> {
    if bytes.len() >= 3 && bytes[0] == 0xff && bytes[1] == 0xd8 && bytes[2] == 0xff {
        return Some(BenchFormat::Jpeg);
    }
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some(BenchFormat::Png);
    }
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return Some(BenchFormat::Webp);
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some(BenchFormat::Gif);
    }
    if is_avif_signature(bytes) {
        return Some(BenchFormat::Avif);
    }
    if is_svg_signature(bytes) {
        return Some(BenchFormat::Svg);
    }
    if bytes.starts_with(b"BM") {
        return Some(BenchFormat::Bmp);
    }
    if bytes.len() >= 6 && &bytes[0..4] == b"\0\0\x01\0" {
        return Some(BenchFormat::Ico);
    }
    None
}

pub fn candidate_decoders(format: BenchFormat) -> &'static [CandidateDecoder] {
    match format {
        BenchFormat::Jpeg => &[
            CandidateDecoder {
                name: "image-crate-rgba",
                note: "Baseline image crate full decode into RGBA.",
                output_pixel_format: "RGBA8",
                allocation_note: "image crate decode followed by RGBA normalization.",
                decode: decode_image_jpeg,
            },
            CandidateDecoder {
                name: "zune-jpeg-rgba-fast",
                note: "Direct zune-jpeg with fast options and RGBA output.",
                output_pixel_format: "RGBA8",
                allocation_note: "zune-jpeg is asked for RGBA output to avoid a separate RGB expansion pass.",
                decode: decode_zune_jpeg,
            },
            CandidateDecoder {
                name: "jpeg-decoder-rgba",
                note: "Legacy pure-Rust JPEG decoder path converted to RGBA.",
                output_pixel_format: "RGBA8",
                allocation_note: "jpeg-decoder output is converted into a fresh RGBA buffer.",
                decode: decode_jpeg_decoder,
            },
            #[cfg(feature = "bench-native-jpeg-turbo")]
            CandidateDecoder {
                name: "libjpeg-turbo-rgba",
                note: "Native TurboJPEG decode directly into an RGBA buffer.",
                output_pixel_format: "RGBA8",
                allocation_note: "TurboJPEG fills a pre-sized RGBA buffer with alpha added by the native decoder.",
                decode: decode_turbojpeg,
            },
        ],
        BenchFormat::Png => &[
            CandidateDecoder {
                name: "image-crate-rgba",
                note: "Baseline image crate full decode into RGBA.",
                output_pixel_format: "RGBA8",
                allocation_note: "image crate decode followed by RGBA normalization.",
                decode: decode_image_png,
            },
            CandidateDecoder {
                name: "png-crate-rgba",
                note: "Direct png crate decode with color normalization.",
                output_pixel_format: "RGBA8",
                allocation_note: "png crate output buffer is normalized and then converted to RGBA when needed.",
                decode: decode_png_crate,
            },
            CandidateDecoder {
                name: "zune-png-rgba-fast",
                note: "Direct zune-png fast decode with 8-bit alpha output.",
                output_pixel_format: "RGBA8",
                allocation_note: "zune-png is asked to strip to 8-bit and add alpha before RGBA validation.",
                decode: decode_zune_png,
            },
            #[cfg(feature = "bench-native-wuffs")]
            CandidateDecoder {
                name: "wuffs-png-rgba",
                note: "Native Wuffs PNG decode into an RGBA pixel buffer.",
                output_pixel_format: "RGBA8",
                allocation_note: "Wuffs writes directly into a pre-sized RGBA buffer plus its format-specific work buffer.",
                decode: wuffs::decode_png,
            },
        ],
        BenchFormat::Webp => &[
            CandidateDecoder {
                name: "image-crate-rgba",
                note: "Baseline image crate full decode into RGBA.",
                output_pixel_format: "RGBA8",
                allocation_note: "image crate decode followed by RGBA normalization.",
                decode: decode_image_webp_baseline,
            },
            CandidateDecoder {
                name: "image-webp-rgba",
                note: "Direct image-webp first-frame decode into RGB/RGBA.",
                output_pixel_format: "RGBA8",
                allocation_note: "image-webp output is used directly when RGBA and expanded from RGB when opaque.",
                decode: decode_image_webp,
            },
            CandidateDecoder {
                name: "image-webp-all-frames-rgba",
                note: "Direct image-webp decode of every animated frame into RGBA canvases.",
                output_pixel_format: "RGBA8 animation frames",
                allocation_note: "image-webp reuses one frame buffer and appends composed RGBA canvases for validation.",
                decode: animated::decode_image_webp_all_frames,
            },
            #[cfg(feature = "bench-native-webp")]
            CandidateDecoder {
                name: "libwebp-rgba",
                note: "Native libwebp simple decoder through the webp crate.",
                output_pixel_format: "RGBA8",
                allocation_note: "libwebp owns the decoded buffer; RGB output is copied once into RGBA for validation.",
                decode: decode_libwebp,
            },
            #[cfg(feature = "bench-native-webp")]
            CandidateDecoder {
                name: "libwebp-all-frames-rgba",
                note: "Native libwebp animation decoder through the webp crate.",
                output_pixel_format: "RGBA8 animation frames",
                allocation_note: "libwebp composes frames internally; the bench copies each frame into a validation buffer.",
                decode: animated::decode_libwebp_all_frames,
            },
        ],
        BenchFormat::Gif => &[
            CandidateDecoder {
                name: "image-crate-rgba",
                note: "Baseline image crate full decode into RGBA.",
                output_pixel_format: "RGBA8",
                allocation_note: "image crate decode followed by RGBA normalization.",
                decode: decode_image_gif,
            },
            CandidateDecoder {
                name: "gif-first-frame-rgba",
                note: "Direct gif crate first full-frame RGBA decode.",
                output_pixel_format: "RGBA8",
                allocation_note: "gif crate produces an RGBA frame buffer for full-canvas first frames.",
                decode: decode_gif_first_frame,
            },
            CandidateDecoder {
                name: "image-gif-animation-rgba",
                note: "image crate GIF animation decoder with full-frame disposal/composition.",
                output_pixel_format: "RGBA8 animation frames",
                allocation_note: "image collects composed RGBA frames into a validation buffer.",
                decode: animated::decode_image_gif_animation,
            },
            CandidateDecoder {
                name: "gif-animation-rgba",
                note: "Direct gif crate all-frame RGBA decode with bench-side composition.",
                output_pixel_format: "RGBA8 animation frames",
                allocation_note: "gif crate expands frame rectangles; the bench composes and stores RGBA canvases.",
                decode: animated::decode_gif_animation,
            },
            #[cfg(feature = "bench-native-wuffs")]
            CandidateDecoder {
                name: "wuffs-gif-first-frame-rgba",
                note: "Native Wuffs GIF first-frame decode into a full-canvas RGBA pixel buffer.",
                output_pixel_format: "RGBA8",
                allocation_note: "Wuffs writes directly into a pre-sized RGBA buffer plus its LZW work buffer.",
                decode: wuffs::decode_gif_first_frame,
            },
        ],
        BenchFormat::Avif => avif_decoders(),
        BenchFormat::Svg => svg_decoders(),
        BenchFormat::Bmp => &[
            CandidateDecoder {
                name: "image-crate-rgba",
                note: "Baseline image crate full decode into RGBA.",
                output_pixel_format: "RGBA8",
                allocation_note: "image crate decode followed by RGBA normalization.",
                decode: decode_image_bmp,
            },
            CandidateDecoder {
                name: "bmp-fast-rgba",
                note: "Direct 24/32-bit uncompressed BMP parser.",
                output_pixel_format: "RGBA8",
                allocation_note: "direct parser writes one RGBA buffer from BGR/BGRA rows.",
                decode: decode_bmp,
            },
        ],
        BenchFormat::Ico => &[
            CandidateDecoder {
                name: "image-crate-rgba",
                note: "Baseline image crate full decode into RGBA.",
                output_pixel_format: "RGBA8",
                allocation_note: "image crate decode followed by RGBA normalization.",
                decode: decode_image_ico,
            },
            CandidateDecoder {
                name: "ico-fast-rgba",
                note: "Direct ICO directory parser with PNG/BMP payload decode.",
                output_pixel_format: "RGBA8",
                allocation_note: "direct ICO parser selects one payload and delegates PNG/BMP bytes to existing candidates.",
                decode: decode_ico,
            },
        ],
    }
}

pub fn deferred_candidates() -> Vec<DeferredCandidate> {
    vec![
        #[cfg(not(feature = "bench-native-jpeg-turbo"))]
        DeferredCandidate {
            format: "jpeg",
            candidate: "libjpeg-turbo",
            reason: "Build with bench-native-jpeg-turbo or bench-native to measure the TurboJPEG backend.",
        },
        #[cfg(not(feature = "bench-native-wuffs"))]
        DeferredCandidate {
            format: "png",
            candidate: "Wuffs PNG",
            reason: "Build with bench-native-wuffs to measure the Wuffs PNG backend.",
        },
        #[cfg(not(feature = "bench-native-webp"))]
        DeferredCandidate {
            format: "webp",
            candidate: "libwebp",
            reason: "Build with bench-native-webp or bench-native to measure the libwebp backend.",
        },
        #[cfg(not(feature = "bench-native-wuffs"))]
        DeferredCandidate {
            format: "gif",
            candidate: "Wuffs GIF",
            reason: "Build with bench-native-wuffs to measure the Wuffs GIF backend.",
        },
        #[cfg(not(any(
            feature = "bench-avif-native",
            feature = "bench-libavif-native"
        )))]
        DeferredCandidate {
            format: "avif",
            candidate: "image avif-native / libavif+dav1d",
            reason: "AVIF is benchmark-gated; build with bench-avif-native or bench-libavif-native where dav1d/native dependencies are available.",
        },
        #[cfg(not(feature = "bench-svg"))]
        DeferredCandidate {
            format: "svg",
            candidate: "resvg/usvg",
            reason: "Renderer benchmark is feature-gated; build with bench-svg to measure static SVG rendering.",
        },
    ]
}

#[cfg(feature = "bench-avif-native")]
fn avif_decoders() -> &'static [CandidateDecoder] {
    &[
        CandidateDecoder {
            name: "image-avif-native-rgba",
            note: "image crate AVIF native decode path, expected to route through dav1d-backed dependencies.",
            output_pixel_format: "RGBA8",
            allocation_note: "image crate AVIF decode followed by RGBA normalization.",
            decode: decode_image_avif,
        },
        #[cfg(feature = "bench-libavif-native")]
        CandidateDecoder {
            name: "libavif-dav1d-rgba",
            note: "Native libavif decode with dav1d codec feature, converted to RGBA.",
            output_pixel_format: "RGBA8",
            allocation_note: "libavif allocates an RGBA image buffer, then the bench copies it into Vec<u8>.",
            decode: decode_libavif,
        },
    ]
}

#[cfg(all(not(feature = "bench-avif-native"), feature = "bench-libavif-native"))]
fn avif_decoders() -> &'static [CandidateDecoder] {
    &[CandidateDecoder {
        name: "libavif-dav1d-rgba",
        note: "Native libavif decode with dav1d codec feature, converted to RGBA.",
        output_pixel_format: "RGBA8",
        allocation_note:
            "libavif allocates an RGBA image buffer, then the bench copies it into Vec<u8>.",
        decode: decode_libavif,
    }]
}

#[cfg(all(
    not(feature = "bench-avif-native"),
    not(feature = "bench-libavif-native")
))]
fn avif_decoders() -> &'static [CandidateDecoder] {
    &[]
}

#[cfg(feature = "bench-svg")]
fn svg_decoders() -> &'static [CandidateDecoder] {
    &[CandidateDecoder {
        name: "resvg-static-rgba",
        note: "resvg/usvg static SVG parse and raster render.",
        output_pixel_format: "RGBA8 premultiplied",
        allocation_note: "resvg parses the SVG and renders into a fresh tiny-skia pixmap.",
        decode: decode_resvg_svg,
    }]
}

#[cfg(not(feature = "bench-svg"))]
fn svg_decoders() -> &'static [CandidateDecoder] {
    &[]
}

fn decode_image_jpeg(bytes: &[u8]) -> Result<DecodedImage, String> {
    decode_image_crate_as(ImageFormat::Jpeg, bytes)
}

fn decode_image_png(bytes: &[u8]) -> Result<DecodedImage, String> {
    decode_image_crate_as(ImageFormat::Png, bytes)
}

fn decode_image_webp_baseline(bytes: &[u8]) -> Result<DecodedImage, String> {
    decode_image_crate_as(ImageFormat::WebP, bytes)
}

fn decode_image_gif(bytes: &[u8]) -> Result<DecodedImage, String> {
    decode_image_crate_as(ImageFormat::Gif, bytes)
}

fn decode_image_bmp(bytes: &[u8]) -> Result<DecodedImage, String> {
    decode_image_crate_as(ImageFormat::Bmp, bytes)
}

fn decode_image_ico(bytes: &[u8]) -> Result<DecodedImage, String> {
    decode_image_crate_as(ImageFormat::Ico, bytes)
}

#[cfg(feature = "bench-avif-native")]
fn decode_image_avif(bytes: &[u8]) -> Result<DecodedImage, String> {
    decode_image_crate_as(ImageFormat::Avif, bytes)
}

#[cfg(feature = "bench-svg")]
fn decode_resvg_svg(bytes: &[u8]) -> Result<DecodedImage, String> {
    let options = resvg::usvg::Options::default();
    let tree = resvg::usvg::Tree::from_data(bytes, &options).map_err(|error| error.to_string())?;
    let size = tree.size().to_int_size();
    let width = size.width();
    let height = size.height();
    checked_pixel_count(width, height)?;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height)
        .ok_or_else(|| "failed to allocate SVG render target".to_owned())?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::identity(),
        &mut pixmap.as_mut(),
    );
    Ok(DecodedImage::still(width, height, pixmap.take()))
}

fn decode_image_crate_as(format: ImageFormat, bytes: &[u8]) -> Result<DecodedImage, String> {
    let image = image::load_from_memory_with_format(bytes, format)
        .map_err(|error| error.to_string())?
        .into_rgba8();
    let (width, height) = image.dimensions();
    Ok(DecodedImage::still(width, height, image.into_raw()))
}

fn from_backend(decoded: decoder_backend::DecodedRgba) -> DecodedImage {
    DecodedImage::still(decoded.width, decoded.height, decoded.pixels)
}

fn decode_zune_jpeg(bytes: &[u8]) -> Result<DecodedImage, String> {
    decoder_backend::decode_zune_jpeg(bytes).map(from_backend)
}

fn decode_jpeg_decoder(bytes: &[u8]) -> Result<DecodedImage, String> {
    let mut decoder = JpegDecoder::new(Cursor::new(bytes));
    let pixels = decoder.decode().map_err(|error| error.to_string())?;
    let info = decoder
        .info()
        .ok_or_else(|| "jpeg-decoder did not report dimensions".to_owned())?;
    rgba_from_jpeg_decoder_pixels(
        pixels,
        info.pixel_format,
        u32::from(info.width),
        u32::from(info.height),
    )
}

#[cfg(feature = "bench-native-jpeg-turbo")]
fn decode_turbojpeg(bytes: &[u8]) -> Result<DecodedImage, String> {
    let mut decompressor = turbojpeg::Decompressor::new().map_err(|error| error.to_string())?;
    let header = decompressor
        .read_header(bytes)
        .map_err(|error| error.to_string())?;
    let width = u32::try_from(header.width).map_err(|_| "TurboJPEG width exceeds u32")?;
    let height = u32::try_from(header.height).map_err(|_| "TurboJPEG height exceeds u32")?;
    let pixel_count = checked_pixel_count(width, height)?;
    let mut image = turbojpeg::Image {
        pixels: vec![0u8; pixel_count * 4],
        width: header.width,
        pitch: header
            .width
            .checked_mul(4)
            .ok_or_else(|| "TurboJPEG pitch overflow".to_owned())?,
        height: header.height,
        format: turbojpeg::PixelFormat::RGBA,
    };
    decompressor
        .decompress(bytes, image.as_deref_mut())
        .map_err(|error| error.to_string())?;
    checked_rgba(image.pixels, width, height)
}

fn decode_png_crate(bytes: &[u8]) -> Result<DecodedImage, String> {
    decoder_backend::decode_png_crate(bytes).map(from_backend)
}

fn decode_zune_png(bytes: &[u8]) -> Result<DecodedImage, String> {
    decoder_backend::decode_zune_png(bytes).map(from_backend)
}

fn decode_image_webp(bytes: &[u8]) -> Result<DecodedImage, String> {
    decoder_backend::decode_image_webp(bytes).map(from_backend)
}

#[cfg(feature = "bench-native-webp")]
fn decode_libwebp(bytes: &[u8]) -> Result<DecodedImage, String> {
    decoder_backend::decode_libwebp(bytes).map(from_backend)
}

#[cfg(feature = "bench-libavif-native")]
fn decode_libavif(bytes: &[u8]) -> Result<DecodedImage, String> {
    decoder_backend::decode_libavif(bytes).map(from_backend)
}

fn decode_gif_first_frame(bytes: &[u8]) -> Result<DecodedImage, String> {
    decoder_backend::decode_gif_first_frame(bytes).map(from_backend)
}

fn decode_bmp(bytes: &[u8]) -> Result<DecodedImage, String> {
    decoder_backend::decode_bmp(bytes).map(from_backend)
}

fn decode_ico(bytes: &[u8]) -> Result<DecodedImage, String> {
    decoder_backend::decode_ico(bytes).map(from_backend)
}

fn rgba_from_jpeg_decoder_pixels(
    pixels: Vec<u8>,
    format: JpegPixelFormat,
    width: u32,
    height: u32,
) -> Result<DecodedImage, String> {
    match format {
        JpegPixelFormat::L8 => rgba_from_luma(pixels, width, height),
        JpegPixelFormat::RGB24 => rgba_from_rgb(pixels, width, height),
        JpegPixelFormat::CMYK32 => Err("CMYK JPEG is not supported by this candidate".to_owned()),
        JpegPixelFormat::L16 => {
            Err("16-bit luma JPEG is not supported by this candidate".to_owned())
        }
    }
}

fn rgba_from_rgb(pixels: Vec<u8>, width: u32, height: u32) -> Result<DecodedImage, String> {
    let pixel_count = checked_pixel_count(width, height)?;
    expect_len(pixels.len(), pixel_count * 3, "RGB")?;
    let mut rgba = Vec::with_capacity(pixel_count * 4);
    for rgb in pixels.chunks_exact(3) {
        rgba.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
    }
    Ok(DecodedImage::still(width, height, rgba))
}

fn rgba_from_luma(pixels: Vec<u8>, width: u32, height: u32) -> Result<DecodedImage, String> {
    let pixel_count = checked_pixel_count(width, height)?;
    expect_len(pixels.len(), pixel_count, "luma")?;
    let mut rgba = Vec::with_capacity(pixel_count * 4);
    for gray in pixels {
        rgba.extend_from_slice(&[gray, gray, gray, 255]);
    }
    Ok(DecodedImage::still(width, height, rgba))
}

#[cfg(feature = "bench-native-jpeg-turbo")]
fn checked_rgba(pixels: Vec<u8>, width: u32, height: u32) -> Result<DecodedImage, String> {
    expect_len(
        pixels.len(),
        checked_pixel_count(width, height)? * 4,
        "RGBA",
    )?;
    Ok(DecodedImage::still(width, height, pixels))
}

pub(super) fn checked_pixel_count(width: u32, height: u32) -> Result<usize, String> {
    let width = usize::try_from(width).map_err(|_| "width exceeds platform limits".to_owned())?;
    let height =
        usize::try_from(height).map_err(|_| "height exceeds platform limits".to_owned())?;
    if width > MAX_BENCH_DIMENSION || height > MAX_BENCH_DIMENSION {
        return Err(format!(
            "image dimensions exceed bench limit: {width}x{height}"
        ));
    }
    width
        .checked_mul(height)
        .ok_or_else(|| "image dimensions overflow memory limits".to_owned())
}

pub(super) fn checked_rgba_len(width: u32, height: u32) -> Result<usize, String> {
    checked_pixel_count(width, height)?
        .checked_mul(4)
        .ok_or_else(|| "RGBA buffer length overflows memory limits".to_owned())
}

pub(super) fn checked_animation_rgba_len(
    width: u32,
    height: u32,
    frames_decoded: u32,
) -> Result<usize, String> {
    if frames_decoded == 0 {
        return Err("animation decoder produced no frames".to_owned());
    }
    let frame_len = checked_rgba_len(width, height)?;
    let total_len = frame_len
        .checked_mul(frames_decoded as usize)
        .ok_or_else(|| "animation frame count overflows memory limits".to_owned())?;
    if total_len > MAX_ANIMATION_RGBA_BYTES {
        return Err(format!(
            "animation RGBA output exceeds bench limit: {total_len} bytes"
        ));
    }
    Ok(total_len)
}

pub(super) fn reserve_animation_frame(
    out: &mut Vec<u8>,
    width: u32,
    height: u32,
) -> Result<(), String> {
    let frame_len = checked_rgba_len(width, height)?;
    let total_len = out
        .len()
        .checked_add(frame_len)
        .ok_or_else(|| "animation frame count overflows memory limits".to_owned())?;
    if total_len > MAX_ANIMATION_RGBA_BYTES {
        return Err(format!(
            "animation RGBA output exceeds bench limit: {total_len} bytes"
        ));
    }
    out.reserve(frame_len);
    Ok(())
}

pub(super) fn expect_len(actual: usize, expected: usize, label: &str) -> Result<(), String> {
    if actual == expected {
        Ok(())
    } else {
        Err(format!(
            "{label} buffer length mismatch: expected {expected}, got {actual}"
        ))
    }
}

fn is_avif_signature(bytes: &[u8]) -> bool {
    if bytes.len() < 12 || &bytes[4..8] != b"ftyp" {
        return false;
    }
    let brands = &bytes[8..bytes.len().min(64)];
    brands
        .chunks(4)
        .any(|brand| matches!(brand, b"avif" | b"avis"))
}

fn is_svg_signature(bytes: &[u8]) -> bool {
    let prefix = &bytes[..bytes.len().min(512)];
    let text = String::from_utf8_lossy(prefix).to_ascii_lowercase();
    let text = text.trim_start_matches('\u{feff}').trim_start();
    text.starts_with("<svg") || text.contains("<svg")
}

#[cfg(test)]
mod tests {
    use super::{detect_format, BenchFormat};

    #[test]
    fn detects_avif_container_signature() {
        let mut bytes = b"\0\0\0\x20ftypavif".to_vec();
        bytes.extend_from_slice(&[0; 24]);
        assert_eq!(detect_format(&bytes), Some(BenchFormat::Avif));
    }

    #[test]
    fn detects_svg_xml_signature() {
        let bytes = br#"<?xml version="1.0"?><svg xmlns="http://www.w3.org/2000/svg"/>"#;
        assert_eq!(detect_format(bytes), Some(BenchFormat::Svg));
    }
}
