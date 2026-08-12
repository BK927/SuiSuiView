use super::ui;
use arboard::{Clipboard, ImageData as ClipboardImageData};
use egui::epaint::ColorImage;
use egui::{self, FontData, FontDefinitions, FontFamily};
use std::borrow::Cow;
#[cfg(target_os = "windows")]
use std::ffi::OsStr;
#[cfg(target_os = "windows")]
use std::ffi::OsString;
use std::fs;
#[cfg(target_os = "windows")]
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
#[cfg(target_os = "windows")]
use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
#[cfg(target_os = "windows")]
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegDeleteValueW, RegSetValueExW, HKEY,
    HKEY_CURRENT_USER, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ,
};
#[cfg(target_os = "windows")]
use windows_sys::Win32::UI::Shell::{SHChangeNotify, SHCNE_ASSOCCHANGED, SHCNF_IDLIST};

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
    let mut bytes = Vec::with_capacity(image.pixels.len() * 4);
    for pixel in &image.pixels {
        bytes.extend_from_slice(&[pixel.r(), pixel.g(), pixel.b(), pixel.a()]);
    }
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

#[cfg(target_os = "windows")]
pub(in crate::app) fn windows_explorer_select_arguments(path: &Path) -> [OsString; 2] {
    [OsString::from("/select,"), path.as_os_str().to_os_string()]
}

#[cfg(target_os = "windows")]
pub(in crate::app) fn register_file_associations(
    selected_extensions: &[&str],
    known_extensions: &[&str],
) -> Result<usize, String> {
    let exe = std::env::current_exe().map_err(|error| error.to_string())?;
    let exe_path = exe.to_string_lossy();
    let icon_value = format!("\"{exe_path}\",0");
    let command_value = format!("\"{exe_path}\" \"%1\"");

    delete_key_tree(CAPABILITIES_ROOT)?;
    for extension in known_extensions {
        delete_key_tree(&classes_prog_id_path(extension))?;
    }

    let capabilities = create_current_user_key(CAPABILITIES_ROOT)?;
    capabilities.set_string(Some("ApplicationName"), APP_NAME)?;
    capabilities.set_string(Some("ApplicationDescription"), APP_DESCRIPTION)?;
    drop(capabilities);

    let associations = create_current_user_key(FILE_ASSOCIATIONS_ROOT)?;
    for extension in selected_extensions {
        let normalized = normalized_extension(extension)?;
        let prog_id = prog_id_for_extension(&normalized)?;
        associations.set_string(Some(&normalized), &prog_id)?;

        let class_root = create_current_user_key(&classes_prog_id_path(&normalized))?;
        class_root.set_string(
            None,
            &format!("{APP_NAME} {} file", normalized.to_uppercase()),
        )?;
        class_root.set_string(
            Some("FriendlyTypeName"),
            &format!("{APP_NAME} {} file", normalized.to_uppercase()),
        )?;
        drop(class_root);

        let icon_key = create_current_user_key(&format!(
            "{}\\DefaultIcon",
            classes_prog_id_path(&normalized)
        ))?;
        icon_key.set_string(None, &icon_value)?;
        drop(icon_key);

        let command_key = create_current_user_key(&format!(
            "{}\\shell\\open\\command",
            classes_prog_id_path(&normalized)
        ))?;
        command_key.set_string(None, &command_value)?;
    }
    drop(associations);

    let registered = create_current_user_key(REGISTERED_APPLICATIONS_ROOT)?;
    registered.set_string(Some(APP_NAME), CAPABILITIES_ROOT)?;
    notify_shell_associations_changed();

    Ok(selected_extensions.len())
}

#[cfg(target_os = "windows")]
pub(in crate::app) fn unregister_file_associations(
    known_extensions: &[&str],
) -> Result<(), String> {
    delete_registered_application_value(APP_NAME)?;
    delete_key_tree(CAPABILITIES_OWNER_ROOT)?;
    for extension in known_extensions {
        delete_key_tree(&classes_prog_id_path(extension))?;
    }
    notify_shell_associations_changed();
    Ok(())
}

#[cfg(target_os = "windows")]
pub(in crate::app) fn open_windows_default_apps_for_suisuiview() -> Result<(), String> {
    Command::new("explorer.exe")
        .arg("ms-settings:defaultapps?registeredAppUser=SuiSuiView")
        .spawn()
        .map(|_| ())
        .map_err(|error| error.to_string())
}

#[cfg(target_os = "windows")]
const APP_NAME: &str = "SuiSuiView";
#[cfg(target_os = "windows")]
const APP_DESCRIPTION: &str = "Fast native image and comic viewer";
#[cfg(target_os = "windows")]
const CAPABILITIES_OWNER_ROOT: &str = "Software\\SuiSuiView";
#[cfg(target_os = "windows")]
const CAPABILITIES_ROOT: &str = "Software\\SuiSuiView\\Capabilities";
#[cfg(target_os = "windows")]
const FILE_ASSOCIATIONS_ROOT: &str = "Software\\SuiSuiView\\Capabilities\\FileAssociations";
#[cfg(target_os = "windows")]
const REGISTERED_APPLICATIONS_ROOT: &str = "Software\\RegisteredApplications";

#[cfg(target_os = "windows")]
struct RegKey(HKEY);

#[cfg(target_os = "windows")]
impl RegKey {
    fn set_string(&self, value_name: Option<&str>, value: &str) -> Result<(), String> {
        let name = value_name.map(wide_null);
        let name_ptr = name.as_ref().map_or(std::ptr::null(), |name| name.as_ptr());
        let data = wide_null(value);
        let data_byte_len = (data.len() * std::mem::size_of::<u16>())
            .try_into()
            .map_err(|_| "registry value is too large".to_owned())?;
        let status = unsafe {
            RegSetValueExW(
                self.0,
                name_ptr,
                0,
                REG_SZ,
                data.as_ptr().cast(),
                data_byte_len,
            )
        };
        if status == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(format!("registry write failed with code {status}"))
        }
    }
}

#[cfg(target_os = "windows")]
impl Drop for RegKey {
    fn drop(&mut self) {
        unsafe {
            RegCloseKey(self.0);
        }
    }
}

#[cfg(target_os = "windows")]
fn create_current_user_key(path: &str) -> Result<RegKey, String> {
    let path = wide_null(path);
    let mut key = std::ptr::null_mut();
    let status = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            path.as_ptr(),
            0,
            std::ptr::null_mut(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            std::ptr::null(),
            &mut key,
            std::ptr::null_mut(),
        )
    };
    if status == ERROR_SUCCESS {
        Ok(RegKey(key))
    } else {
        Err(format!("registry key create failed with code {status}"))
    }
}

#[cfg(target_os = "windows")]
fn delete_key_tree(path: &str) -> Result<(), String> {
    let path = wide_null(path);
    let status = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, path.as_ptr()) };
    if status == ERROR_SUCCESS || status == ERROR_FILE_NOT_FOUND {
        Ok(())
    } else {
        Err(format!("registry key delete failed with code {status}"))
    }
}

#[cfg(target_os = "windows")]
fn delete_registered_application_value(name: &str) -> Result<(), String> {
    let registered = create_current_user_key(REGISTERED_APPLICATIONS_ROOT)?;
    let name = wide_null(name);
    let status = unsafe { RegDeleteValueW(registered.0, name.as_ptr()) };
    if status == ERROR_SUCCESS || status == ERROR_FILE_NOT_FOUND {
        Ok(())
    } else {
        Err(format!("registry value delete failed with code {status}"))
    }
}

#[cfg(target_os = "windows")]
fn normalized_extension(extension: &str) -> Result<String, String> {
    let extension = extension
        .trim()
        .trim_start_matches('.')
        .to_ascii_lowercase();
    if extension.is_empty()
        || extension
            .chars()
            .any(|ch| !(ch.is_ascii_alphanumeric() || ch == '_'))
    {
        return Err(format!("invalid extension: {extension}"));
    }
    Ok(format!(".{extension}"))
}

#[cfg(target_os = "windows")]
fn prog_id_for_extension(extension: &str) -> Result<String, String> {
    let extension = normalized_extension(extension)?;
    Ok(format!("{APP_NAME}.{}", extension.trim_start_matches('.')))
}

#[cfg(target_os = "windows")]
fn classes_prog_id_path(extension: &str) -> String {
    let extension = extension
        .trim()
        .trim_start_matches('.')
        .to_ascii_lowercase();
    format!("Software\\Classes\\{APP_NAME}.{extension}")
}

#[cfg(target_os = "windows")]
fn wide_null(value: &str) -> Vec<u16> {
    OsStr::new(value).encode_wide().chain([0]).collect()
}

#[cfg(target_os = "windows")]
fn notify_shell_associations_changed() {
    unsafe {
        SHChangeNotify(
            SHCNE_ASSOCCHANGED as i32,
            SHCNF_IDLIST,
            std::ptr::null(),
            std::ptr::null(),
        );
    }
}
