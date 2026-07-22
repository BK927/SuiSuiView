#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
#[allow(dead_code)]
mod core;
mod single_instance;
mod startup_window;

use crate::core::source::{classify_path, SourceKind};
use crate::core::state::{RendererMode, StateStore, WindowPlacement};
use crossbeam_channel::Receiver;
use std::path::PathBuf;

const DEFAULT_WINDOW_SIZE: [f32; 2] = [1280.0, 820.0];
const MIN_WINDOW_SIZE: [f32; 2] = [860.0, 560.0];
const GUI_CLI_REDIRECT_MESSAGE: &str =
    "CLI 명령은 suisuiview-cli를 사용하세요.\n예: suisuiview-cli --perf-scan <path>";
const RESTART_BYPASS_SINGLE_INSTANCE_ENV: &str = "SUISUIVIEW_RESTART_BYPASS_SINGLE_INSTANCE";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if let Some(first_arg) = std::env::args_os().nth(1) {
        if is_gui_cli_redirect_arg(&first_arg) {
            show_cli_redirect_message();
            return Ok(());
        }
    }

    let store = StateStore::load();
    let _startup_flash_guard = startup_window::start_flash_guard(startup_window_guard_mode(&store));
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

    // The custom winit host is the sole runtime. renderer_mode Wgpu takes the
    // WGPU fast-start handoff; LowMemoryGlow runs the Glow-only host; a handoff
    // failure persists the demotion and relaunches into the Glow-only host
    // (winit forbids a second event loop in one process).
    let wants_wgpu = !app::winit_host::glow_forced()
        && (app::winit_host::requested()
            || app::winit_host::enabled_for_settings(store.settings()));
    if wants_wgpu {
        let mut handoff_store = store.clone();
        handoff_store.clear_fast_start_failure_notice();
        match run_host(
            handoff_store,
            true,
            ipc_rx.clone(),
            startup_open_path.clone(),
        ) {
            Ok(()) => Ok(()),
            Err(failure) => {
                eprintln!(
                    "SuiSuiView WGPU fast start failed at {}: {}",
                    failure.stage.key(),
                    failure.error
                );
                // Persist LowMemoryGlow + the failure notice (to disk), then
                // relaunch: a fresh process reads the demoted setting and runs
                // the Glow-only host.
                let _ = app::fast_start::disable_gpu_after_handoff_failure(
                    StateStore::load(),
                    &failure,
                    startup_open_path.as_deref(),
                );
                if let Err(error) = app::restart_current_process() {
                    eprintln!("SuiSuiView restart into Glow host failed: {error}");
                }
                Ok(())
            }
        }
    } else {
        // renderer_mode = LowMemoryGlow: the Glow-only host is the runtime.
        if let Err(failure) = run_host(store, false, ipc_rx, startup_open_path) {
            eprintln!(
                "SuiSuiView Glow host failed at {}: {}",
                failure.stage.key(),
                failure.error
            );
        }
        Ok(())
    }
}

#[allow(clippy::result_large_err)] // mirrors winit_host::run's return type
fn run_host(
    store: StateStore,
    handoff_enabled: bool,
    ipc_rx: Option<Receiver<Option<PathBuf>>>,
    startup_open_path: Option<PathBuf>,
) -> Result<(), app::winit_host::HostFailure> {
    let startup_open = startup_open_path
        .as_ref()
        .and_then(|path| app::start_startup_open_loader(path.clone(), &store));
    app::winit_host::run(
        app::winit_host::WinitHostOptions {
            store,
            ipc_rx,
            startup_open_path,
            startup_open,
            icon: window_icon(),
            default_window_size: DEFAULT_WINDOW_SIZE,
            min_window_size: MIN_WINDOW_SIZE,
        },
        handoff_enabled,
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

fn startup_window_guard_mode(store: &StateStore) -> startup_window::StartupWindowGuardMode {
    startup_window_guard_mode_for(store.settings().renderer_mode, store.window_placement())
}

fn startup_window_guard_mode_for(
    _renderer_mode: RendererMode,
    _placement: &WindowPlacement,
) -> startup_window::StartupWindowGuardMode {
    use startup_window::StartupWindowGuardMode::{AuxiliaryOnly, MaskMainUntilRevealed};
    if !cfg!(target_os = "windows") {
        return AuxiliaryOnly;
    }
    // winit shows the window *inside* create_window (before any post-create Rust
    // code can mask it), so the main-window flash can only be caught by the
    // guard's WH_CALLWNDPROC hook. Every renderer/placement combination holds
    // that mask until the host reveals it on the first rendered frame via
    // `reveal_main_windows()`. The retired MaskMainUntilStable mode instead
    // revealed the Glow-maximized window as soon as it reported visible, but
    // that state is winit's transient pre-paint show inside create_window, so
    // the reveal exposed an unpainted frame that winit then hid again — a
    // visible ~26ms blink at startup.
    MaskMainUntilRevealed
}

fn window_icon() -> egui::IconData {
    let image = image::load_from_memory(include_bytes!("../assets/app-icon.ico"))
        .expect("embedded app icon should be a valid ICO")
        .into_rgba8();
    let width = image.width();
    let height = image.height();

    egui::IconData {
        rgba: image.into_raw(),
        width,
        height,
    }
}

#[cfg(test)]
mod tests {
    use super::{startup_window_guard_mode_for, window_icon};
    use crate::core::state::{RendererMode, WindowPlacement};
    use crate::startup_window::StartupWindowGuardMode;

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
    fn startup_window_guard_mode_per_renderer_and_placement() {
        let maximized = WindowPlacement {
            inner_size: Some([1280.0, 820.0]),
            outer_position: None,
            outer_position_px: None,
            normal_rect_px: None,
            maximized: true,
        };
        let windowed = WindowPlacement {
            maximized: false,
            ..maximized.clone()
        };

        // Every renderer/placement combination masks the main window via the
        // guard hook (the flash happens inside create_window) and holds the mask
        // until the host reveals it on the first rendered frame.
        let expected = if cfg!(target_os = "windows") {
            StartupWindowGuardMode::MaskMainUntilRevealed
        } else {
            StartupWindowGuardMode::AuxiliaryOnly
        };
        for placement in [&maximized, &windowed] {
            for renderer in [RendererMode::LowMemoryGlow, RendererMode::Wgpu] {
                assert_eq!(startup_window_guard_mode_for(renderer, placement), expected);
            }
        }
    }
}
