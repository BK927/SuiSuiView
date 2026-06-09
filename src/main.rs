#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
#[allow(dead_code)]
mod core;
mod single_instance;
mod startup_window;

use crate::core::source::{classify_path, SourceKind};
use crate::core::state::{AppSettings, RendererMode, StateStore, WindowPlacement};
use app::SuiSuiViewApp;
use crossbeam_channel::Receiver;
use std::path::PathBuf;
use std::sync::Arc;

const DEFAULT_WINDOW_SIZE: [f32; 2] = [1280.0, 820.0];
const MIN_WINDOW_SIZE: [f32; 2] = [860.0, 560.0];
const GUI_CLI_REDIRECT_MESSAGE: &str =
    "CLI 명령은 suisuiview-cli를 사용하세요.\n예: suisuiview-cli --perf-scan <path>";
const RESTART_BYPASS_SINGLE_INSTANCE_ENV: &str = "SUISUIVIEW_RESTART_BYPASS_SINGLE_INSTANCE";

fn main() -> eframe::Result<()> {
    if let Some(first_arg) = std::env::args_os().nth(1) {
        if is_gui_cli_redirect_arg(&first_arg) {
            show_cli_redirect_message();
            return Ok(());
        }
    }

    let _startup_flash_guard = startup_window::start_flash_guard();
    let store = StateStore::load();
    let startup_open_path = startup_open_path();

    let restart_bypasses_single_instance =
        std::env::var_os(RESTART_BYPASS_SINGLE_INSTANCE_ENV).is_some();
    let ipc_rx = if store.settings().single_instance && !restart_bypasses_single_instance {
        let pipe_name = single_instance::pipe_name_for_key(&store.path().display().to_string());
        if single_instance::send_open_request(&pipe_name, startup_open_path.as_deref()) {
            return Ok(());
        }
        Some(single_instance::start_listener(pipe_name))
    } else {
        None
    };

    #[cfg(feature = "wgpu-fast-start")]
    if app::handoff_preview::requested()
        || app::handoff_preview::enabled_for_settings(store.settings())
    {
        let mut handoff_store = store.clone();
        handoff_store.clear_fast_start_failure_notice();
        let startup_open = startup_open_path
            .as_ref()
            .and_then(|path| app::start_startup_open_loader(path.clone(), &handoff_store));
        match app::handoff_preview::run(app::handoff_preview::HandoffPreviewOptions {
            store: handoff_store,
            ipc_rx: ipc_rx.clone(),
            startup_open_path: startup_open_path.clone(),
            startup_open,
            icon: window_icon(),
            default_window_size: DEFAULT_WINDOW_SIZE,
            min_window_size: MIN_WINDOW_SIZE,
        }) {
            Ok(()) => return Ok(()),
            Err(failure) => {
                eprintln!(
                    "SuiSuiView WGPU fast start failed at {}: {}",
                    failure.stage.key(),
                    failure.error
                );
                let fallback_store = app::fast_start::disable_gpu_after_handoff_failure(
                    StateStore::load(),
                    &failure,
                    startup_open_path.as_deref(),
                );
                let result = run_eframe_app(fallback_store.clone(), ipc_rx, startup_open_path);
                if let Err(error) = &result {
                    show_fast_start_fallback_failed_message(&fallback_store, error);
                }
                return result;
            }
        }
    }

    run_eframe_app(store, ipc_rx, startup_open_path)
}

fn run_eframe_app(
    store: StateStore,
    ipc_rx: Option<Receiver<Option<PathBuf>>>,
    startup_open_path: Option<PathBuf>,
) -> eframe::Result<()> {
    let startup_open = startup_open_path
        .as_ref()
        .and_then(|path| app::start_startup_open_loader(path.clone(), &store));
    let options = eframe::NativeOptions {
        viewport: initial_viewport(&store, window_icon()),
        renderer: renderer_for_settings(store.settings()),
        wgpu_options: wgpu_options_for_settings(store.settings()),
        persist_window: false,
        ..Default::default()
    };

    eframe::run_native(
        "SuiSuiView",
        options,
        Box::new(|cc| {
            Ok(Box::new(SuiSuiViewApp::from_eframe(
                cc,
                store,
                ipc_rx,
                startup_open_path,
                startup_open,
            )))
        }),
    )
}

#[cfg(feature = "wgpu-fast-start")]
fn show_fast_start_fallback_failed_message(store: &StateStore, error: &eframe::Error) {
    let diagnostic_path = store
        .fast_start_failure_notice()
        .and_then(|notice| notice.diagnostic_path.as_deref())
        .unwrap_or("진단 파일 경로 없음");
    let message = format!(
        "WGPU 빠른 시작 실패 후 일반 모드 실행도 실패했습니다.\n\n오류: {error}\n진단 파일: {diagnostic_path}"
    );
    let _ = rfd::MessageDialog::new()
        .set_title("SuiSuiView 시작 실패")
        .set_description(message)
        .set_level(rfd::MessageLevel::Error)
        .set_buttons(rfd::MessageButtons::Ok)
        .show();
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
    initial_viewport_for_placement(store.window_placement(), icon)
}

fn initial_viewport_for_placement(
    placement: &WindowPlacement,
    icon: eframe::egui::IconData,
) -> eframe::egui::ViewportBuilder {
    let inner_size = valid_window_size(placement).unwrap_or(DEFAULT_WINDOW_SIZE);
    let mut viewport = eframe::egui::ViewportBuilder::default()
        .with_inner_size(inner_size)
        .with_min_inner_size(MIN_WINDOW_SIZE)
        .with_clamp_size_to_monitor_size(true)
        .with_icon(Arc::new(icon));
    #[cfg(target_os = "windows")]
    {
        viewport = viewport.with_visible(false);
    }

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

fn wgpu_options_for_settings(settings: &AppSettings) -> egui_wgpu::WgpuConfiguration {
    let mut options = egui_wgpu::WgpuConfiguration::default();
    if matches!(settings.renderer_mode, RendererMode::Wgpu) {
        tune_wgpu_startup_options(&mut options);
    }
    options
}

#[cfg(target_os = "windows")]
fn tune_wgpu_startup_options(options: &mut egui_wgpu::WgpuConfiguration) {
    if std::env::var_os("WGPU_BACKEND").is_some() {
        return;
    }
    if let egui_wgpu::WgpuSetup::CreateNew(create_new) = &mut options.wgpu_setup {
        // eframe includes the wgpu GL fallback by default; this app's WGPU mode
        // targets native WGSL backends and keeps Glow as the OpenGL path.
        create_new.instance_descriptor.backends = wgpu::Backends::PRIMARY;
        create_new.instance_descriptor.flags = wgpu::InstanceFlags::empty().with_env();
    }
}

#[cfg(not(target_os = "windows"))]
fn tune_wgpu_startup_options(_options: &mut egui_wgpu::WgpuConfiguration) {}

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
    use super::{initial_viewport_for_placement, window_icon};
    use crate::core::state::WindowPlacement;

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

    #[test]
    fn initial_viewport_visibility_matches_platform() {
        let viewport = initial_viewport_for_placement(
            &WindowPlacement {
                inner_size: Some([1280.0, 820.0]),
                outer_position: None,
                maximized: false,
            },
            window_icon(),
        );

        if cfg!(target_os = "windows") {
            assert_eq!(viewport.visible, Some(false));
        } else {
            assert_eq!(viewport.visible, None);
        }
    }

    #[test]
    fn initial_viewport_keeps_saved_maximize_state() {
        let viewport = initial_viewport_for_placement(
            &WindowPlacement {
                inner_size: Some([1280.0, 820.0]),
                outer_position: None,
                maximized: true,
            },
            window_icon(),
        );

        assert_eq!(viewport.maximized, Some(true));
    }
}
