#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
#[allow(dead_code)]
mod core;
mod single_instance;

use crate::core::source::{classify_path, SourceKind};
use crate::core::state::{AppSettings, RendererMode, StateStore, WindowPlacement};
use app::SuiSuiViewApp;
use std::path::PathBuf;
use std::sync::Arc;

const DEFAULT_WINDOW_SIZE: [f32; 2] = [1280.0, 820.0];
const MIN_WINDOW_SIZE: [f32; 2] = [860.0, 560.0];
const GUI_CLI_REDIRECT_MESSAGE: &str =
    "CLI 명령은 suisuiview-cli를 사용하세요.\n예: suisuiview-cli --perf-scan <path>";

fn main() -> eframe::Result<()> {
    if let Some(first_arg) = std::env::args_os().nth(1) {
        if is_gui_cli_redirect_arg(&first_arg) {
            show_cli_redirect_message();
            return Ok(());
        }
    }

    let store = StateStore::load();
    let startup_open_path = startup_open_path();
    let ipc_rx = if store.settings().single_instance {
        let pipe_name = single_instance::pipe_name_for_key(&store.path().display().to_string());
        if single_instance::send_open_request(&pipe_name, startup_open_path.as_deref()) {
            return Ok(());
        }
        Some(single_instance::start_listener(pipe_name))
    } else {
        None
    };
    let options = eframe::NativeOptions {
        viewport: initial_viewport(&store, window_icon()),
        renderer: renderer_for_settings(store.settings()),
        persist_window: false,
        ..Default::default()
    };

    eframe::run_native(
        "SuiSuiView",
        options,
        Box::new(|cc| {
            Ok(Box::new(SuiSuiViewApp::new(
                cc,
                store,
                ipc_rx,
                startup_open_path,
            )))
        }),
    )
}

fn show_cli_redirect_message() {
    eprintln!("{GUI_CLI_REDIRECT_MESSAGE}");

    #[cfg(target_os = "windows")]
    {
        let _ = rfd::MessageDialog::new()
            .set_title("SuiSuiView CLI")
            .set_description(GUI_CLI_REDIRECT_MESSAGE)
            .set_level(rfd::MessageLevel::Info)
            .set_buttons(rfd::MessageButtons::Ok)
            .show();
    }
}

fn is_gui_cli_redirect_arg(arg: &std::ffi::OsString) -> bool {
    arg == "--help"
        || arg == "-h"
        || arg == "help"
        || arg == "--perf-scan"
        || arg == "--quality-scan"
        || arg == "--effect-bench"
        || arg == "--upscale-bench"
        || arg == "--upscale-quality-scan"
        || arg == "--gpu-copy-bench"
}

fn startup_open_path() -> Option<PathBuf> {
    std::env::args_os().skip(1).map(PathBuf::from).find(|path| {
        matches!(
            classify_path(path),
            SourceKind::Folder | SourceKind::ZipCbz | SourceKind::SingleImage
        )
    })
}

fn initial_viewport(
    store: &StateStore,
    icon: eframe::egui::IconData,
) -> eframe::egui::ViewportBuilder {
    let placement = store.window_placement();
    let inner_size = valid_window_size(placement).unwrap_or(DEFAULT_WINDOW_SIZE);
    let mut viewport = eframe::egui::ViewportBuilder::default()
        .with_inner_size(inner_size)
        .with_min_inner_size(MIN_WINDOW_SIZE)
        .with_clamp_size_to_monitor_size(true)
        .with_icon(Arc::new(icon));

    if let Some(position) = valid_window_position(placement) {
        viewport = viewport.with_position(position);
    }
    if placement.maximized {
        viewport = viewport.with_maximized(true);
    }
    viewport
}

fn valid_window_size(placement: &WindowPlacement) -> Option<[f32; 2]> {
    let [width, height] = placement.inner_size?;
    (width.is_finite()
        && height.is_finite()
        && width >= MIN_WINDOW_SIZE[0]
        && height >= MIN_WINDOW_SIZE[1])
        .then_some([width, height])
}

fn valid_window_position(placement: &WindowPlacement) -> Option<[f32; 2]> {
    let [x, y] = placement.outer_position?;
    (x.is_finite() && y.is_finite()).then_some([x, y])
}

fn renderer_for_settings(settings: &AppSettings) -> eframe::Renderer {
    match settings.renderer_mode {
        RendererMode::Wgpu => eframe::Renderer::Wgpu,
        RendererMode::LowMemoryGlow => eframe::Renderer::Glow,
    }
}

fn window_icon() -> eframe::egui::IconData {
    let image = image::load_from_memory(include_bytes!("../assets/app-icon.ico"))
        .expect("embedded app icon should be a valid ICO")
        .into_rgba8();
    let width = image.width();
    let height = image.height();

    eframe::egui::IconData {
        rgba: image.into_raw(),
        width,
        height,
    }
}

#[cfg(test)]
mod tests {
    use super::window_icon;

    #[test]
    fn embedded_window_icon_loads_from_ico() {
        let icon = window_icon();

        assert!(icon.width > 0);
        assert!(icon.height > 0);
        assert!(icon.width <= 256);
        assert!(icon.height <= 256);
        assert_eq!(
            icon.rgba.len(),
            icon.width as usize * icon.height as usize * 4
        );
    }
}
