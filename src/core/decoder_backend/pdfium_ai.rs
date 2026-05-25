use super::{ensure_backend_rgba_budget, is_pdf_signature, DecodedRgba};
use pdfium_render::prelude::{PdfPage, PdfRenderConfig, Pdfium};
use std::sync::OnceLock;

const MAX_AI_RENDER_LONG_EDGE: u32 = 4096;

pub fn decode_pdfium_ai(bytes: &[u8], target_long_edge: u32) -> Result<DecodedRgba, String> {
    if !is_pdf_signature(bytes) {
        return Err("AI file must be saved with PDF-compatible content".to_owned());
    }

    let pdfium = app_local_pdfium()?;
    let document = pdfium
        .load_pdf_from_byte_slice(bytes, None)
        .map_err(|error| format!("failed to load PDF-compatible AI data: {error}"))?;
    if document.pages().is_empty() {
        return Err("PDF-compatible AI file did not contain a renderable page".to_owned());
    }
    let page = document
        .pages()
        .get(0)
        .map_err(|error| format!("failed to load first AI page: {error}"))?;
    let (target_width, target_height) = ai_render_dimensions(&page, target_long_edge)?;
    let render_config = PdfRenderConfig::new()
        .set_target_size(target_width, target_height)
        .set_reverse_byte_order(true);
    let image = page
        .render_with_config(&render_config)
        .map_err(|error| format!("failed to render AI preview with PDFium: {error}"))?
        .as_image()
        .map_err(|error| format!("failed to convert PDFium bitmap to image: {error}"))?
        .into_rgba8();
    DecodedRgba::new(image.width(), image.height(), image.into_raw())
}

fn app_local_pdfium() -> Result<Pdfium, String> {
    static PDFIUM: OnceLock<Result<Pdfium, String>> = OnceLock::new();
    PDFIUM
        .get_or_init(|| {
            let exe_path = std::env::current_exe()
                .map_err(|error| format!("failed to locate current executable: {error}"))?;
            let exe_dir = exe_path
                .parent()
                .ok_or_else(|| "current executable has no containing directory".to_owned())?;
            let library_path = Pdfium::pdfium_platform_library_name_at_path(exe_dir);
            let bindings = Pdfium::bind_to_library(library_path).map_err(|error| {
                format!("PDFium library was not found next to the executable: {error}")
            })?;
            Ok(Pdfium::new(bindings))
        })
        .clone()
}

fn ai_render_dimensions(page: &PdfPage<'_>, target_long_edge: u32) -> Result<(i32, i32), String> {
    let source_width = page.width().value.max(1.0);
    let source_height = page.height().value.max(1.0);
    let target_long_edge = target_long_edge.clamp(1, MAX_AI_RENDER_LONG_EDGE);
    let scale = target_long_edge as f32 / source_width.max(source_height);
    let width = (source_width * scale)
        .round()
        .clamp(1.0, target_long_edge as f32) as u32;
    let height = (source_height * scale)
        .round()
        .clamp(1.0, target_long_edge as f32) as u32;
    ensure_backend_rgba_budget(width, height, "AI preview")?;
    let width =
        i32::try_from(width).map_err(|_| "AI render width exceeds platform limits".to_owned())?;
    let height =
        i32::try_from(height).map_err(|_| "AI render height exceeds platform limits".to_owned())?;
    Ok((width, height))
}
