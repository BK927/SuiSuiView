use super::{
    prepare_image, prepare_image_with_options, DecodeBackend, DecodeOptions, DecodeStrategy,
};

#[test]
fn psd_preview_decodes_with_zune_psd_backend() {
    let bytes = encoded_test_psd_rgb();
    let page = prepare_image(&bytes, 1024).unwrap();

    assert_eq!(page.decode_backend, DecodeBackend::ZunePsd);
    assert_eq!(page.original_width, 2);
    assert_eq!(page.original_height, 1);
    assert_eq!(page.display_width, 2);
    assert_eq!(page.display_height, 1);
    assert!(!page.pixels.is_luma());
    assert_eq!(
        &page.pixels.as_slice()[..8],
        &[255, 0, 0, 255, 0, 255, 0, 255]
    );
}

#[test]
fn psd_oversized_header_fails_before_decode() {
    let bytes = encoded_test_psd_header(12_000, 12_000, 3, 8);
    let error = prepare_image(&bytes, 1024).err().unwrap();

    assert!(error.contains("too large"));
}

#[test]
fn psd_unsupported_color_mode_fails_before_decode() {
    let bytes = encoded_test_psd_header_with_color_mode(2, 1, 4, 8, 4);
    let error = prepare_image(&bytes, 1024).err().unwrap();

    assert!(error.contains("unsupported PSD color mode"));
}

#[test]
fn psd_skips_image_metadata_probe() {
    let bytes = encoded_test_psd_rgb();
    let page = prepare_image_with_options(
        &bytes,
        1024,
        DecodeOptions {
            strategy: DecodeStrategy::ImageCrate,
            apply_embedded_icc: true,
            apply_exif_orientation: true,
            ..DecodeOptions::default()
        },
    )
    .unwrap();

    assert_eq!(page.decode_backend, DecodeBackend::ZunePsd);
    assert!(page.notice.is_none());
}

#[test]
fn postscript_ai_requires_pdf_compatible_content() {
    let error = prepare_image(b"%!PS-Adobe-3.0\n%%Creator: Adobe Illustrator", 1024)
        .err()
        .unwrap();

    assert!(error.contains("PDF-compatible"));
}

#[cfg(feature = "native-ai")]
#[test]
fn pdf_compatible_ai_renders_with_pdfium_when_available() {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|parent| parent.to_path_buf()))
        .unwrap();
    let pdfium_path =
        pdfium_render::prelude::Pdfium::pdfium_platform_library_name_at_path(&exe_dir);
    if !pdfium_path.exists() {
        eprintln!(
            "skipping PDF-compatible AI render test; missing {}",
            pdfium_path.display()
        );
        return;
    }

    let page = prepare_image(&encoded_test_pdf_compatible_ai(), 128).unwrap();

    assert_eq!(page.decode_backend, DecodeBackend::PdfiumAi);
    assert_eq!(page.original_width, 128);
    assert_eq!(page.original_height, 128);
    assert_eq!(page.display_width, 128);
    assert_eq!(page.display_height, 128);
    assert_eq!(page.pixels.byte_len(), 128 * 128 * 4);
}

fn encoded_test_psd_rgb() -> Vec<u8> {
    let mut bytes = encoded_test_psd_header(2, 1, 3, 8);
    bytes.extend_from_slice(&0u32.to_be_bytes());
    bytes.extend_from_slice(&0u32.to_be_bytes());
    bytes.extend_from_slice(&0u32.to_be_bytes());
    bytes.extend_from_slice(&1u16.to_be_bytes());
    for _ in 0..3 {
        bytes.extend_from_slice(&3u16.to_be_bytes());
    }
    bytes.extend_from_slice(&[1, 255, 0]);
    bytes.extend_from_slice(&[1, 0, 255]);
    bytes.extend_from_slice(&[1, 0, 0]);
    bytes
}

fn encoded_test_psd_header(width: u32, height: u32, channels: u16, depth: u16) -> Vec<u8> {
    encoded_test_psd_header_with_color_mode(width, height, channels, depth, 3)
}

fn encoded_test_psd_header_with_color_mode(
    width: u32,
    height: u32,
    channels: u16,
    depth: u16,
    color_mode: u16,
) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"8BPS");
    bytes.extend_from_slice(&1u16.to_be_bytes());
    bytes.extend_from_slice(&[0; 6]);
    bytes.extend_from_slice(&channels.to_be_bytes());
    bytes.extend_from_slice(&height.to_be_bytes());
    bytes.extend_from_slice(&width.to_be_bytes());
    bytes.extend_from_slice(&depth.to_be_bytes());
    bytes.extend_from_slice(&color_mode.to_be_bytes());
    bytes
}

#[cfg(feature = "native-ai")]
fn encoded_test_pdf_compatible_ai() -> Vec<u8> {
    let content = b"q\n1 0 0 rg\n0 0 64 64 re\nf\nQ\n";
    let objects = [
        b"<< /Type /Catalog /Pages 2 0 R >>".as_slice(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".as_slice(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 64 64] /Resources << /ProcSet [/PDF] >> /Contents 4 0 R >>".as_slice(),
    ];
    let mut bytes = b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offsets = vec![0usize];
    for (index, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
        bytes.extend_from_slice(object);
        bytes.extend_from_slice(b"\nendobj\n");
    }
    offsets.push(bytes.len());
    bytes.extend_from_slice(b"4 0 obj\n<< /Length ");
    bytes.extend_from_slice(content.len().to_string().as_bytes());
    bytes.extend_from_slice(b" >>\nstream\n");
    bytes.extend_from_slice(content);
    bytes.extend_from_slice(b"endstream\nendobj\n");

    let xref_offset = bytes.len();
    bytes.extend_from_slice(b"xref\n0 5\n0000000000 65535 f \n");
    for offset in offsets.into_iter().skip(1) {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(b"trailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n");
    bytes.extend_from_slice(xref_offset.to_string().as_bytes());
    bytes.extend_from_slice(b"\n%%EOF\n");
    bytes
}
