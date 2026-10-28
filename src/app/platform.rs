use super::ui;
use arboard::{Clipboard, ImageData as ClipboardImageData};
use egui::epaint::ColorImage;
use egui::{self, FontData, FontDefinitions, FontFamily};
use std::borrow::Cow;
#[cfg(target_os = "windows")]
use std::ffi::OsString;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
const RESTART_BYPASS_SINGLE_INSTANCE_ENV: &str = "SUISUIVIEW_RESTART_BYPASS_SINGLE_INSTANCE";
const GPU_DEMOTION_GLOW_RESTART_ENV: &str = "SUISUIVIEW_GPU_DEMOTION_GLOW_RESTART";

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

pub(in crate::app) fn apply_window_level(ctx: &egui::Context, always_on_top: bool) {
    let level = if always_on_top {
        egui::WindowLevel::AlwaysOnTop
    } else {
        egui::WindowLevel::Normal
    };
    ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(level));
}

pub(in crate::app) fn copy_text_to_clipboard(text: &str) -> Result<(), String> {
    Clipboard::new()
        .map_err(|error| error.to_string())?
        .set_text(text.to_owned())
        .map_err(|error| error.to_string())
}

pub(in crate::app) fn copy_color_image_to_clipboard(image: &ColorImage) -> Result<(), String> {
    let bytes = clipboard_rgba_bytes(image);
    Clipboard::new()
        .map_err(|error| error.to_string())?
        .set_image(ClipboardImageData {
            width: image.size[0],
            height: image.size[1],
            bytes: Cow::Owned(bytes),
        })
        .map_err(|error| error.to_string())
}

pub(in crate::app) fn reveal_in_file_manager(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        let args = windows_explorer_select_arguments(path);
        Command::new("explorer.exe")
            .args(args)
            .spawn()
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    #[cfg(target_os = "macos")]
    {
        Command::new("open")
            .arg("-R")
            .arg(path)
            .spawn()
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    {
        let target = path.parent().unwrap_or(path);
        Command::new("xdg-open")
            .arg(target)
            .spawn()
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

pub(in crate::app) fn restart_current_process() -> Result<(), String> {
    restart_current_process_with_env(false)
}

pub(in crate::app) fn restart_current_process_into_glow() -> Result<(), String> {
    restart_current_process_with_env(true)
}

fn restart_current_process_with_env(force_glow_once: bool) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|error| error.to_string())?;
    let args = std::env::args_os().skip(1);
    let mut command = Command::new(exe);
    command
        .args(args)
        .env(RESTART_BYPASS_SINGLE_INSTANCE_ENV, "1");
    if force_glow_once {
        command.env(GPU_DEMOTION_GLOW_RESTART_ENV, "1");
    } else {
        command.env_remove(GPU_DEMOTION_GLOW_RESTART_ENV);
    }
    command
        .spawn()
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn clipboard_rgba_bytes(image: &ColorImage) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(image.pixels.len() * 4);
    for pixel in &image.pixels {
        bytes.extend_from_slice(&pixel.to_srgba_unmultiplied());
    }
    bytes
}

#[cfg(test)]
mod clipboard_tests {
    #[test]
    fn clipboard_preserves_straight_alpha_colors() {
        let rgba = [
            255, 0, 0, 255, 255, 0, 0, 128, 0, 255, 0, 128, 0, 0, 255, 64,
        ];
        let image = egui::ColorImage::from_rgba_unmultiplied([4, 1], &rgba);
        assert_eq!(super::clipboard_rgba_bytes(&image), rgba);
    }
}

#[cfg(target_os = "windows")]
pub(in crate::app) fn windows_explorer_select_arguments(path: &Path) -> [OsString; 2] {
    [OsString::from("/select,"), path.as_os_str().to_os_string()]
}

#[cfg(target_os = "windows")]
mod associations;
#[cfg(target_os = "windows")]
pub(in crate::app) use associations::{
    open_windows_default_apps_for_suisuiview, register_file_associations,
    unregister_file_associations,
};
