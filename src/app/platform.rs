use super::ui;
use eframe::egui::{self, FontData, FontDefinitions, FontFamily};
use std::fs;
use std::sync::Arc;

pub(in crate::app) fn install_app_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    if let Some((name, bytes)) = load_first_existing_font(korean_font_candidates()) {
        fonts
            .font_data
            .insert(name.clone(), Arc::new(FontData::from_owned(bytes)));
        for family in [FontFamily::Proportional, FontFamily::Monospace] {
            fonts.families.entry(family).or_default().push(name.clone());
        }
    }
    install_icon_font(
        &mut fonts,
        ui::icons::REGULAR_FONT,
        include_bytes!("../../assets/fonts/FluentSystemIcons-Regular.ttf"),
    );
    install_icon_font(
        &mut fonts,
        ui::icons::FILLED_FONT,
        include_bytes!("../../assets/fonts/FluentSystemIcons-Filled.ttf"),
    );
    ctx.set_fonts(fonts);
}

fn install_icon_font(fonts: &mut FontDefinitions, name: &str, bytes: &'static [u8]) {
    fonts
        .font_data
        .insert(name.to_owned(), Arc::new(FontData::from_static(bytes)));
    fonts
        .families
        .insert(FontFamily::Name(name.into()), vec![name.to_owned()]);
}

pub(in crate::app) fn load_first_existing_font(candidates: &[&str]) -> Option<(String, Vec<u8>)> {
    candidates.iter().find_map(|path| {
        fs::read(path).ok().map(|bytes| {
            (
                format!("suisuiview-cjk-{}", sanitize_font_name(path)),
                bytes,
            )
        })
    })
}

pub(in crate::app) fn korean_font_candidates() -> &'static [&'static str] {
    &[
        "C:\\Windows\\Fonts\\malgun.ttf",
        "C:\\Windows\\Fonts\\malgunbd.ttf",
        "/System/Library/Fonts/AppleSDGothicNeo.ttc",
        "/System/Library/Fonts/AppleGothic.ttf",
        "/usr/share/fonts/opentype/noto/NotoSansCJKkr-Regular.otf",
        "/usr/share/fonts/opentype/noto/NotoSansKR-Regular.otf",
        "/usr/share/fonts/truetype/noto/NotoSansKR-Regular.ttf",
    ]
}

pub(in crate::app) fn sanitize_font_name(path: &str) -> String {
    path.chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect()
}
