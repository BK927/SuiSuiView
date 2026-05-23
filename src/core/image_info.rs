use image::metadata::Orientation;
use image::{ExtendedColorType, ImageDecoder, ImageFormat, ImageReader};
use lcms2::{InfoType, Locale, Profile};
use std::io::{BufReader, Cursor};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageInfo {
    pub summary: ImageSummary,
    pub exif: ExifInfo,
    pub color: ColorProfileInfo,
    pub exif_tags: Vec<ImageExifTag>,
    pub gps_tags: Vec<ImageExifTag>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageSummary {
    pub format: Option<String>,
    pub width: u32,
    pub height: u32,
    pub file_bytes: usize,
    pub color_type: String,
    pub channel_count: u8,
    pub bits_per_pixel: u16,
    pub bit_depth: Option<u16>,
    pub has_alpha: bool,
    pub animation: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ExifInfo {
    pub orientation: Option<String>,
    pub captured_at: Option<String>,
    pub camera_make: Option<String>,
    pub camera_model: Option<String>,
    pub lens_model: Option<String>,
    pub iso: Option<String>,
    pub exposure_time: Option<String>,
    pub f_number: Option<String>,
    pub focal_length: Option<String>,
    pub exposure_bias: Option<String>,
    pub flash: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ColorProfileInfo {
    pub icc_profile_bytes: Option<usize>,
    pub icc_profile_name: Option<String>,
    pub icc_profile_error: Option<String>,
    pub png_color_type: Option<String>,
    pub png_bit_depth: Option<u8>,
    pub png_srgb: Option<String>,
    pub png_gamma: Option<String>,
    pub png_chromaticities: Option<String>,
    pub png_density: Option<String>,
    pub png_animation_frames: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageExifTag {
    pub ifd: String,
    pub tag: String,
    pub value: String,
}

pub fn analyze_image_info(bytes: &[u8]) -> Result<ImageInfo, String> {
    let reader = ImageReader::new(BufReader::new(Cursor::new(bytes)))
        .with_guessed_format()
        .map_err(|error| error.to_string())?;
    let format = reader.format();
    let mut decoder = reader.into_decoder().map_err(|error| error.to_string())?;
    let (width, height) = decoder.dimensions();
    let original_color = decoder.original_color_type();
    let orientation = decoder
        .orientation()
        .ok()
        .filter(|orientation| *orientation != Orientation::NoTransforms);
    let (icc_profile, icc_profile_error) = match decoder.icc_profile() {
        Ok(profile) => (profile, None),
        Err(error) => (None, Some(error.to_string())),
    };

    let exif_bundle = read_exif_info(bytes, orientation);
    let png_info = if format == Some(ImageFormat::Png) {
        read_png_color_info(bytes)
    } else {
        None
    }
    .unwrap_or_default();

    let mut color = ColorProfileInfo {
        icc_profile_bytes: icc_profile
            .as_ref()
            .map(Vec::len)
            .or(png_info.icc_profile_bytes),
        icc_profile_name: icc_profile.as_deref().and_then(icc_profile_name),
        icc_profile_error,
        ..png_info
    };
    if icc_profile.is_some()
        && color.icc_profile_name.is_none()
        && color.icc_profile_error.is_none()
    {
        color.icc_profile_error = Some("프로파일 이름을 읽을 수 없습니다.".to_owned());
    }

    Ok(ImageInfo {
        summary: ImageSummary {
            format: format.map(format_label),
            width,
            height,
            file_bytes: bytes.len(),
            color_type: color_type_label(original_color).to_owned(),
            channel_count: original_color.channel_count(),
            bits_per_pixel: original_color.bits_per_pixel(),
            bit_depth: bit_depth_per_channel(original_color),
            has_alpha: has_alpha(original_color),
            animation: animation_label(format, &color),
        },
        exif: exif_bundle.info,
        color,
        exif_tags: exif_bundle.tags,
        gps_tags: exif_bundle.gps_tags,
    })
}

fn read_exif_info(bytes: &[u8], orientation: Option<Orientation>) -> ExifBundle {
    let mut reader = BufReader::new(Cursor::new(bytes));
    let Ok(exif) = exif::Reader::new().read_from_container(&mut reader) else {
        return ExifBundle {
            info: ExifInfo {
                orientation: orientation.map(orientation_label),
                ..Default::default()
            },
            tags: Vec::new(),
            gps_tags: Vec::new(),
        };
    };

    let mut tags = Vec::new();
    let mut gps_tags = Vec::new();
    for field in exif.fields() {
        let item = ImageExifTag {
            ifd: field.ifd_num.to_string(),
            tag: field.tag.to_string(),
            value: field.display_value().with_unit(&exif).to_string(),
        };
        if field.tag.context() == exif::Context::Gps {
            gps_tags.push(item);
        } else {
            tags.push(item);
        }
    }

    ExifBundle {
        info: ExifInfo {
            orientation: exif_text(&exif, exif::Tag::Orientation)
                .or_else(|| orientation.map(orientation_label)),
            captured_at: exif_text(&exif, exif::Tag::DateTimeOriginal),
            camera_make: exif_text(&exif, exif::Tag::Make),
            camera_model: exif_text(&exif, exif::Tag::Model),
            lens_model: exif_text(&exif, exif::Tag::LensModel),
            iso: exif_text(&exif, exif::Tag::PhotographicSensitivity),
            exposure_time: exif_text(&exif, exif::Tag::ExposureTime),
            f_number: exif_text(&exif, exif::Tag::FNumber),
            focal_length: exif_text(&exif, exif::Tag::FocalLength),
            exposure_bias: exif_text(&exif, exif::Tag::ExposureBiasValue),
            flash: exif_text(&exif, exif::Tag::Flash),
        },
        tags,
        gps_tags,
    }
}

struct ExifBundle {
    info: ExifInfo,
    tags: Vec<ImageExifTag>,
    gps_tags: Vec<ImageExifTag>,
}

fn exif_text(exif: &exif::Exif, tag: exif::Tag) -> Option<String> {
    exif.fields()
        .find(|field| field.tag == tag)
        .map(|field| field.display_value().with_unit(exif).to_string())
        .filter(|value| !value.trim().is_empty())
}

fn read_png_color_info(bytes: &[u8]) -> Option<ColorProfileInfo> {
    let decoder = png::Decoder::new(Cursor::new(bytes));
    let reader = decoder.read_info().ok()?;
    let info = reader.info();
    let png_chromaticities = info.chromaticities().map(|chromaticities| {
        format!(
            "white {:.4}/{:.4}, red {:.4}/{:.4}, green {:.4}/{:.4}, blue {:.4}/{:.4}",
            chromaticities.white.0.into_value(),
            chromaticities.white.1.into_value(),
            chromaticities.red.0.into_value(),
            chromaticities.red.1.into_value(),
            chromaticities.green.0.into_value(),
            chromaticities.green.1.into_value(),
            chromaticities.blue.0.into_value(),
            chromaticities.blue.1.into_value()
        )
    });
    Some(ColorProfileInfo {
        icc_profile_bytes: info.icc_profile.as_ref().map(|profile| profile.len()),
        icc_profile_name: None,
        icc_profile_error: None,
        png_color_type: Some(png_color_type_label(info.color_type).to_owned()),
        png_bit_depth: Some(info.bit_depth as u8),
        png_srgb: info.srgb.map(|intent| format!("{intent:?}")),
        png_gamma: info
            .gamma()
            .map(|gamma| format!("{:.5}", gamma.into_value())),
        png_chromaticities,
        png_density: info.pixel_dims.map(|dims| match dims.unit {
            png::Unit::Meter => format!("{} x {} px/m", dims.xppu, dims.yppu),
            png::Unit::Unspecified => format!("{} x {} px/unit", dims.xppu, dims.yppu),
        }),
        png_animation_frames: info.animation_control.map(|control| control.num_frames),
    })
}

fn icc_profile_name(profile_bytes: &[u8]) -> Option<String> {
    Profile::new_icc(profile_bytes)
        .ok()
        .and_then(|profile| profile.info(InfoType::Description, Locale::new("en_US")))
        .or_else(|| {
            Profile::new_icc(profile_bytes)
                .ok()
                .and_then(|profile| profile.info(InfoType::Description, Locale::none()))
        })
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
}

fn format_label(format: ImageFormat) -> String {
    match format {
        ImageFormat::Png => "PNG",
        ImageFormat::Jpeg => "JPEG",
        ImageFormat::Gif => "GIF",
        ImageFormat::WebP => "WebP",
        ImageFormat::Bmp => "BMP",
        ImageFormat::Ico => "ICO",
        ImageFormat::Tiff => "TIFF",
        ImageFormat::Tga => "TGA",
        ImageFormat::Dds => "DDS",
        ImageFormat::OpenExr => "OpenEXR",
        ImageFormat::Hdr => "HDR",
        ImageFormat::Pnm => "PNM",
        ImageFormat::Qoi => "QOI",
        _ => return format!("{format:?}"),
    }
    .to_owned()
}

fn color_type_label(color: ExtendedColorType) -> &'static str {
    match color {
        ExtendedColorType::A8 => "Alpha 8-bit",
        ExtendedColorType::L1
        | ExtendedColorType::L2
        | ExtendedColorType::L4
        | ExtendedColorType::L8
        | ExtendedColorType::L16 => "Grayscale",
        ExtendedColorType::La1
        | ExtendedColorType::La2
        | ExtendedColorType::La4
        | ExtendedColorType::La8
        | ExtendedColorType::La16 => "Grayscale + Alpha",
        ExtendedColorType::Rgb1
        | ExtendedColorType::Rgb2
        | ExtendedColorType::Rgb4
        | ExtendedColorType::Rgb5x1
        | ExtendedColorType::Rgb8
        | ExtendedColorType::Rgb16
        | ExtendedColorType::Rgb32F => "RGB",
        ExtendedColorType::Rgba1
        | ExtendedColorType::Rgba2
        | ExtendedColorType::Rgba4
        | ExtendedColorType::Rgba8
        | ExtendedColorType::Rgba16
        | ExtendedColorType::Rgba32F => "RGBA",
        ExtendedColorType::Bgr8 => "BGR",
        ExtendedColorType::Bgra8 => "BGRA",
        ExtendedColorType::Cmyk8 | ExtendedColorType::Cmyk16 => "CMYK",
        ExtendedColorType::Unknown(_) => "Unknown",
        _ => "Unknown",
    }
}

fn bit_depth_per_channel(color: ExtendedColorType) -> Option<u16> {
    let channels = color.channel_count();
    if channels == 0 {
        return None;
    }
    Some(color.bits_per_pixel() / u16::from(channels))
}

fn has_alpha(color: ExtendedColorType) -> bool {
    matches!(
        color,
        ExtendedColorType::A8
            | ExtendedColorType::La1
            | ExtendedColorType::La2
            | ExtendedColorType::La4
            | ExtendedColorType::La8
            | ExtendedColorType::La16
            | ExtendedColorType::Rgba1
            | ExtendedColorType::Rgba2
            | ExtendedColorType::Rgba4
            | ExtendedColorType::Rgba8
            | ExtendedColorType::Rgba16
            | ExtendedColorType::Rgba32F
            | ExtendedColorType::Bgra8
    )
}

fn animation_label(format: Option<ImageFormat>, color: &ColorProfileInfo) -> Option<String> {
    match format {
        Some(ImageFormat::Gif) => Some("GIF 애니메이션 가능".to_owned()),
        Some(ImageFormat::Png) => color
            .png_animation_frames
            .map(|frames| format!("APNG {frames} frames")),
        _ => None,
    }
}

fn png_color_type_label(color: png::ColorType) -> &'static str {
    match color {
        png::ColorType::Grayscale => "Grayscale",
        png::ColorType::Rgb => "RGB",
        png::ColorType::Indexed => "Indexed",
        png::ColorType::GrayscaleAlpha => "Grayscale + Alpha",
        png::ColorType::Rgba => "RGBA",
    }
}

fn orientation_label(orientation: Orientation) -> String {
    match orientation {
        Orientation::NoTransforms => "1 (No transform)",
        Orientation::FlipHorizontal => "2 (Flip horizontal)",
        Orientation::Rotate180 => "3 (Rotate 180)",
        Orientation::FlipVertical => "4 (Flip vertical)",
        Orientation::Rotate90FlipH => "5 (Rotate 90 + flip horizontal)",
        Orientation::Rotate90 => "6 (Rotate 90 CW)",
        Orientation::Rotate270FlipH => "7 (Rotate 270 + flip horizontal)",
        Orientation::Rotate270 => "8 (Rotate 270 CW)",
    }
    .to_owned()
}

#[cfg(test)]
mod tests {
    use super::analyze_image_info;
    use image::{codecs::jpeg::JpegEncoder, ExtendedColorType};
    use png::{BitDepth, ColorType, Encoder};

    #[test]
    fn png_rgb_metadata_tracks_color_channels_and_depth() {
        let bytes = png_bytes(ColorType::Rgb, &[10, 20, 30]);
        let info = analyze_image_info(&bytes).unwrap();

        assert_eq!(info.summary.format.as_deref(), Some("PNG"));
        assert_eq!((info.summary.width, info.summary.height), (1, 1));
        assert_eq!(info.summary.color_type, "RGB");
        assert_eq!(info.summary.channel_count, 3);
        assert_eq!(info.summary.bit_depth, Some(8));
        assert!(!info.summary.has_alpha);
        assert_eq!(info.color.png_color_type.as_deref(), Some("RGB"));
    }

    #[test]
    fn png_rgba_metadata_reports_alpha() {
        let bytes = png_bytes(ColorType::Rgba, &[10, 20, 30, 128]);
        let info = analyze_image_info(&bytes).unwrap();

        assert_eq!(info.summary.color_type, "RGBA");
        assert_eq!(info.summary.channel_count, 4);
        assert!(info.summary.has_alpha);
    }

    #[test]
    fn image_without_exif_uses_empty_exif_fields() {
        let bytes = png_bytes(ColorType::Rgb, &[0, 0, 0]);
        let info = analyze_image_info(&bytes).unwrap();

        assert!(info.exif.captured_at.is_none());
        assert!(info.exif_tags.is_empty());
        assert!(info.gps_tags.is_empty());
    }

    #[test]
    fn jpeg_metadata_reads_exif_orientation() {
        let bytes = jpeg_with_exif_orientation();
        let info = analyze_image_info(&bytes).unwrap();

        assert_eq!(info.summary.format.as_deref(), Some("JPEG"));
        assert_eq!((info.summary.width, info.summary.height), (1, 1));
        assert!(info.exif.orientation.is_some());
        assert!(info.exif_tags.iter().any(|tag| tag.tag == "Orientation"));
    }

    fn png_bytes(color: ColorType, data: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut encoder = Encoder::new(&mut bytes, 1, 1);
        encoder.set_color(color);
        encoder.set_depth(BitDepth::Eight);
        {
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(data).unwrap();
        }
        bytes
    }

    fn jpeg_with_exif_orientation() -> Vec<u8> {
        let mut jpeg = Vec::new();
        JpegEncoder::new(&mut jpeg)
            .encode(&[255, 0, 0], 1, 1, ExtendedColorType::Rgb8)
            .unwrap();

        let mut exif_payload = Vec::new();
        exif_payload.extend_from_slice(b"Exif\0\0");
        exif_payload.extend_from_slice(b"II");
        exif_payload.extend_from_slice(&42u16.to_le_bytes());
        exif_payload.extend_from_slice(&8u32.to_le_bytes());
        exif_payload.extend_from_slice(&1u16.to_le_bytes());
        exif_payload.extend_from_slice(&0x0112u16.to_le_bytes());
        exif_payload.extend_from_slice(&3u16.to_le_bytes());
        exif_payload.extend_from_slice(&1u32.to_le_bytes());
        exif_payload.extend_from_slice(&6u16.to_le_bytes());
        exif_payload.extend_from_slice(&0u16.to_le_bytes());
        exif_payload.extend_from_slice(&0u32.to_le_bytes());

        let segment_len = u16::try_from(exif_payload.len() + 2).unwrap();
        let mut with_exif = Vec::new();
        with_exif.extend_from_slice(&jpeg[..2]);
        with_exif.extend_from_slice(&[0xFF, 0xE1]);
        with_exif.extend_from_slice(&segment_len.to_be_bytes());
        with_exif.extend_from_slice(&exif_payload);
        with_exif.extend_from_slice(&jpeg[2..]);
        with_exif
    }
}
