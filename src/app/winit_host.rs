#![allow(unsafe_code)]

use super::{StartupOpen, SuiSuiViewApp};
use crate::core::state::{AppSettings, RendererMode, StateStore};
use crossbeam_channel::Receiver;
use egui::{self};
use egui_wgpu::winit::Painter;
use egui_winit::winit;
use serde::Serialize;
use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

mod dpi_guard;
mod glow_stage;
mod glow_window;
mod prewarm;
mod title_sync;
mod wgpu_stage;

use dpi_guard::DpiSizeGuard;
use glow_window::GlutinWindowContext;
use prewarm::{run_wgpu_prewarm, PrewarmReport, PrewarmedWgpu};

// The "handoff" naming in these external-facing strings (CLI arg + env vars) is
// kept for external compatibility even though the module was renamed to winit_host.
const REQUEST_ARG: &str = "--experimental-app-handoff";
const REQUEST_ENV: &str = "SUISUIVIEW_EXPERIMENT_APP_HANDOFF";
/// Forces the Glow-only host regardless of the persisted renderer setting.
/// Testing hook: lets a harness exercise the Glow path without touching the
/// user's state.json (mirror of `REQUEST_ENV`, which forces WGPU).
const FORCE_GLOW_ENV: &str = "SUISUIVIEW_FORCE_GLOW";
const FAIL_STAGE_ENV: &str = "SUISUIVIEW_HANDOFF_FAIL_STAGE";

pub(crate) struct WinitHostOptions {
    pub(crate) store: StateStore,
    pub(crate) ipc_rx: Option<Receiver<Option<PathBuf>>>,
    pub(crate) startup_open_path: Option<PathBuf>,
    pub(crate) startup_open: Option<StartupOpen>,
    pub(crate) icon: egui::IconData,
    pub(crate) default_window_size: [f32; 2],
    pub(crate) min_window_size: [f32; 2],
}

pub(crate) fn requested() -> bool {
    std::env::var_os(REQUEST_ENV).is_some() || std::env::args_os().any(|arg| arg == REQUEST_ARG)
}

pub(crate) fn glow_forced() -> bool {
    std::env::var_os(FORCE_GLOW_ENV).is_some()
}

pub(crate) fn enabled_for_settings(settings: &AppSettings) -> bool {
    matches!(settings.renderer_mode, RendererMode::Wgpu)
}

// HostFailure is intentionally rich diagnostic data; boxing it would only obscure the error path.
#[allow(clippy::result_large_err)]
pub(crate) fn run(options: WinitHostOptions, wgpu_direct: bool) -> Result<(), HostFailure> {
    let event_loop = winit::event_loop::EventLoop::<()>::new()
        .map_err(|error| HostFailure::new(HostFailureStage::Unknown, error.to_string()))?;
    let wake_proxy = event_loop.create_proxy();
    let mut app = WinitHostApp::new(options, wgpu_direct, wake_proxy);
    let result = event_loop.run_app(&mut app);
    if let Some(failure) = app.failure.take() {
        return Err(failure);
    }
    result.map_err(|error| {
        HostFailure::with_metrics(
            HostFailureStage::Unknown,
            format!("failed to run app handoff preview: {error}"),
            app.metrics.clone(),
        )
    })
}

#[derive(Debug, Clone)]
pub(crate) struct HostFailure {
    pub(crate) stage: HostFailureStage,
    pub(crate) error: String,
    pub(crate) metrics: WinitHostMetrics,
}

impl HostFailure {
    fn new(stage: HostFailureStage, error: String) -> Self {
        Self {
            stage,
            error: error.clone(),
            metrics: WinitHostMetrics {
                error: Some(error),
                ..WinitHostMetrics::default()
            },
        }
    }

    fn with_metrics(stage: HostFailureStage, error: String, mut metrics: WinitHostMetrics) -> Self {
        metrics.error = Some(error.clone());
        Self {
            stage,
            error,
            metrics,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) enum HostFailureStage {
    GlCreate,
    GlSwap,
    WgpuPrewarm,
    FirstWgpuFrame,
    Unknown,
}

impl HostFailureStage {
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::GlCreate => "gl_create",
            Self::GlSwap => "gl_swap",
            Self::WgpuPrewarm => "wgpu_prewarm",
            Self::FirstWgpuFrame => "first_wgpu_frame",
            Self::Unknown => "unknown",
        }
    }
}

struct WinitHostApp {
    options: Option<WinitHostOptions>,
    // true: build the window and attach a WGPU surface directly (renderer_mode
    // Wgpu). false: run the Glow-only host (LowMemoryGlow) for the whole session.
    wgpu_direct: bool,
    started_at: Instant,
    stage: Option<Stage>,
    prewarm_rx: Option<mpsc::Receiver<PrewarmReport>>,
    prewarmed_wgpu: Option<PrewarmedWgpu>,
    metrics: WinitHostMetrics,
    failure: Option<HostFailure>,
    summary_printed: bool,
    // Reactive control flow for the steady-state host (Glow-only, or the WGPU
    // stage after handoff): egui's repaint callback records the next requested
    // redraw time here so `about_to_wait` can `WaitUntil` it instead of
    // busy-polling. `None` means idle (wait for input).
    redraw_deadline: Arc<Mutex<Option<Instant>>>,
    // Wakes the (possibly sleeping) event loop when a repaint is requested from
    // another thread, so background work (image loads, thumbnails) is shown
    // without waiting for the next input event.
    wake_proxy: winit::event_loop::EventLoopProxy<()>,
    /// Authoritative logical inner size, defended against winit 0.30's mixed-DPI
    /// drag storms (see DpiSizeGuard docs).
    dpi_size_guard: DpiSizeGuard,
    // Last window title the Glow stage broadcast. A change must go through
    // `schedule_process_visible_window_title` so its generation bump kills any
    // stale timed re-assert that would otherwise restore the old title.
    glow_synced_title: Option<String>,
}

enum Stage {
    Glow {
        app: SuiSuiViewApp,
        gl_window: GlutinWindowContext,
        gl: Arc<glow::Context>,
        egui_glow: egui_glow::EguiGlow,
    },
    Wgpu {
        app: SuiSuiViewApp,
        window: winit::window::Window,
        egui_state: egui_winit::State,
        painter: Painter,
        viewport_info: egui::ViewportInfo,
    },
}

#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct WinitHostMetrics {
    pub(crate) first_glow_present_ms: Option<f64>,
    pub(crate) last_glow_present_ms: Option<f64>,
    pub(crate) handoff_started_ms: Option<f64>,
    pub(crate) glow_destroy_ms: Option<f64>,
    pub(crate) gl_context_destroy_ms: Option<f64>,
    pub(crate) wgpu_painter_new_ms: Option<f64>,
    pub(crate) wgpu_set_window_ms: Option<f64>,
    pub(crate) first_wgpu_present_ms: Option<f64>,
    pub(crate) handoff_gap_ms: Option<f64>,
    pub(crate) prewarm_started_ms: Option<f64>,
    pub(crate) prewarm_ready_ms: Option<f64>,
    pub(crate) prewarm_init_ms: Option<f64>,
    pub(crate) prewarm_adapter_name: Option<String>,
    pub(crate) prewarm_backend: Option<String>,
    pub(crate) prewarm_device_type: Option<String>,
    pub(crate) used_prewarmed_wgpu: bool,
    pub(crate) prewarm_error: Option<String>,
    pub(crate) error: Option<String>,
}

impl WinitHostApp {
    fn new(
        options: WinitHostOptions,
        wgpu_direct: bool,
        wake_proxy: winit::event_loop::EventLoopProxy<()>,
    ) -> Self {
        Self {
            options: Some(options),
            wgpu_direct,
            started_at: Instant::now(),
            stage: None,
            prewarm_rx: None,
            prewarmed_wgpu: None,
            metrics: WinitHostMetrics::default(),
            failure: None,
            summary_printed: false,
            redraw_deadline: Arc::new(Mutex::new(None)),
            wake_proxy,
            dpi_size_guard: DpiSizeGuard::new(),
            glow_synced_title: None,
        }
    }

    fn redraw(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        let Some(stage) = self.stage.take() else {
            return;
        };
        match stage {
            Stage::Glow {
                app,
                gl_window,
                gl,
                egui_glow,
            } => self.redraw_glow(event_loop, app, gl_window, gl, egui_glow),
            Stage::Wgpu {
                app,
                window,
                egui_state,
                painter,
                viewport_info,
            } => self.redraw_wgpu(event_loop, app, window, egui_state, painter, viewport_info),
        }
    }

    /// Register egui's repaint callback (records the next redraw deadline and
    /// wakes the loop). Shared by the Glow and WGPU-direct start paths.
    fn install_repaint_callback(&self, egui_ctx: &egui::Context) {
        let deadline = self.redraw_deadline.clone();
        let wake_proxy = self.wake_proxy.clone();
        egui_ctx.set_request_repaint_callback(move |info| {
            let at = Instant::now() + info.delay;
            {
                let mut slot = deadline.lock().unwrap();
                *slot = Some(slot.map_or(at, |current| current.min(at)));
            }
            // Wake the event loop so it re-evaluates the deadline; needed when
            // the request comes from a non-UI thread while idle.
            let _ = wake_proxy.send_event(());
        });
    }

    fn start_prewarm(&mut self) {
        if self.prewarm_rx.is_some() || self.prewarmed_wgpu.is_some() {
            return;
        }
        self.metrics.prewarm_started_ms = Some(elapsed_ms(self.started_at.elapsed()));
        let started_at = self.started_at;
        let (sender, receiver) = mpsc::channel();
        std::thread::Builder::new()
            .name("suisuiview-app-handoff-prewarm-wgpu".to_owned())
            .spawn(move || {
                let _ = sender.send(run_wgpu_prewarm(started_at));
            })
            .expect("failed to spawn WGPU prewarm thread");
        self.prewarm_rx = Some(receiver);
    }

    fn apply_prewarm_report(&mut self, report: PrewarmReport) {
        self.metrics.prewarm_ready_ms = Some(report.ready_ms);
        self.metrics.prewarm_init_ms = Some(report.init_ms);
        self.metrics.prewarm_adapter_name = report.adapter_name;
        self.metrics.prewarm_backend = report.backend;
        self.metrics.prewarm_device_type = report.device_type;
        match report.result {
            Ok(prewarmed) => {
                if let Some(error) = injected_failure(HostFailureStage::WgpuPrewarm) {
                    self.metrics.prewarm_error = Some(error);
                } else {
                    self.prewarmed_wgpu = Some(prewarmed);
                }
            }
            Err(error) => {
                self.metrics.prewarm_error = Some(error);
            }
        }
    }

    fn poll_prewarm(&mut self) {
        let Some(receiver) = self.prewarm_rx.take() else {
            return;
        };
        match receiver.try_recv() {
            Ok(report) => self.apply_prewarm_report(report),
            Err(mpsc::TryRecvError::Empty) => {
                self.prewarm_rx = Some(receiver);
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.metrics.prewarm_error = Some("prewarm thread disconnected".to_owned());
            }
        }
    }

    /// Block until the WGPU prewarm thread finishes (used by the WGPU-direct
    /// path, which spawns prewarm concurrently with window creation and then
    /// needs the prewarmed device to build the painter).
    fn wait_for_prewarm(&mut self) {
        let Some(receiver) = self.prewarm_rx.take() else {
            return;
        };
        match receiver.recv() {
            Ok(report) => self.apply_prewarm_report(report),
            Err(_) => {
                self.metrics.prewarm_error = Some("prewarm thread disconnected".to_owned());
            }
        }
    }

    fn fail(
        &mut self,
        event_loop: &winit::event_loop::ActiveEventLoop,
        stage: HostFailureStage,
        error: String,
    ) {
        self.metrics.error = Some(error.clone());
        self.failure = Some(HostFailure::with_metrics(
            stage,
            error,
            self.metrics.clone(),
        ));
        self.print_summary();
        event_loop.exit();
    }

    fn print_summary(&mut self) {
        if self.summary_printed {
            return;
        }
        self.summary_printed = true;
        println!(
            "app_handoff_preview glow_first_visible_ms={:.3} last_glow_present_ms={:.3} handoff_started_ms={:.3} glow_destroy_ms={:.3} gl_context_destroy_ms={:.3} wgpu_painter_new_ms={:.3} wgpu_set_window_ms={:.3} first_wgpu_present_ms={:.3} handoff_gap_ms={:.3} prewarm_started_ms={:.3} prewarm_ready_ms={:.3} prewarm_init_ms={:.3} prewarm_adapter={} prewarm_backend={} prewarm_device_type={} used_prewarmed_wgpu={} prewarm_error={} error={}",
            self.metrics.first_glow_present_ms.unwrap_or(-1.0),
            self.metrics.last_glow_present_ms.unwrap_or(-1.0),
            self.metrics.handoff_started_ms.unwrap_or(-1.0),
            self.metrics.glow_destroy_ms.unwrap_or(-1.0),
            self.metrics.gl_context_destroy_ms.unwrap_or(-1.0),
            self.metrics.wgpu_painter_new_ms.unwrap_or(-1.0),
            self.metrics.wgpu_set_window_ms.unwrap_or(-1.0),
            self.metrics.first_wgpu_present_ms.unwrap_or(-1.0),
            self.metrics.handoff_gap_ms.unwrap_or(-1.0),
            self.metrics.prewarm_started_ms.unwrap_or(-1.0),
            self.metrics.prewarm_ready_ms.unwrap_or(-1.0),
            self.metrics.prewarm_init_ms.unwrap_or(-1.0),
            self.metrics.prewarm_adapter_name.as_deref().unwrap_or("unknown"),
            self.metrics.prewarm_backend.as_deref().unwrap_or("unknown"),
            self.metrics.prewarm_device_type.as_deref().unwrap_or("unknown"),
            self.metrics.used_prewarmed_wgpu,
            self.metrics.prewarm_error.as_deref().unwrap_or("none"),
            self.metrics.error.as_deref().unwrap_or("none"),
        );
    }
}

impl winit::application::ApplicationHandler<()> for WinitHostApp {
    fn resumed(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        if self.stage.is_some() {
            return;
        }
        // renderer_mode Wgpu builds the window + attaches a WGPU surface
        // directly; LowMemoryGlow runs the Glow-only host.
        let result = if self.wgpu_direct {
            self.start_wgpu_direct(event_loop)
        } else {
            self.start_glow(event_loop)
        };
        if let Err(error) = result {
            self.fail(event_loop, HostFailureStage::GlCreate, error);
        }
    }

    fn window_event(
        &mut self,
        event_loop: &winit::event_loop::ActiveEventLoop,
        _window_id: winit::window::WindowId,
        mut event: winit::event::WindowEvent,
    ) {
        // Non-consuming: the event still flows to egui below (see the method doc).
        if let Some((w, h)) = self.dpi_size_guard.defend_scale_change(&mut event) {
            // A stale DPI suggested rect was applied as a plain Resized with no
            // scale event to hang the correction on; re-request the tracked size.
            let window = match self.stage.as_ref() {
                Some(Stage::Glow { gl_window, .. }) => Some(gl_window.window()),
                Some(Stage::Wgpu { window, .. }) => Some(window),
                None => None,
            };
            if let Some(window) = window {
                let _ = window.request_inner_size(winit::dpi::PhysicalSize::new(w, h));
            }
        }
        if matches!(
            event,
            winit::event::WindowEvent::CloseRequested | winit::event::WindowEvent::Destroyed
        ) {
            self.print_summary();
            event_loop.exit();
            return;
        }
        if matches!(event, winit::event::WindowEvent::RedrawRequested) {
            self.redraw(event_loop);
            return;
        }

        if let Some(stage) = self.stage.as_mut() {
            match stage {
                Stage::Glow {
                    gl_window,
                    egui_glow,
                    ..
                } => Self::glow_window_event(event, gl_window, egui_glow),
                Stage::Wgpu {
                    window,
                    egui_state,
                    painter,
                    ..
                } => Self::wgpu_window_event(event, window, painter, egui_state),
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        use winit::event_loop::ControlFlow;

        // Bootstrap the first frame (Glow, or the first WGPU frame for
        // WGPU-direct) so we can reveal and measure it.
        let needs_first_frame = match self.stage {
            Some(Stage::Glow { .. }) => self.metrics.first_glow_present_ms.is_none(),
            Some(Stage::Wgpu { .. }) => self.metrics.first_wgpu_present_ms.is_none(),
            None => false,
        };
        if needs_first_frame {
            self.redraw(event_loop);
            event_loop.set_control_flow(ControlFlow::Poll);
            return;
        }

        // Reactive steady state (both the Glow-only host and the WGPU-direct
        // stage): sleep until egui's next requested repaint or an input event
        // instead of rendering every loop iteration.
        let deadline = *self.redraw_deadline.lock().unwrap();
        match deadline {
            Some(at) if at <= Instant::now() => {
                match self.stage.as_ref() {
                    Some(Stage::Glow { gl_window, .. }) => gl_window.window().request_redraw(),
                    Some(Stage::Wgpu { window, .. }) => window.request_redraw(),
                    None => {}
                }
                event_loop.set_control_flow(ControlFlow::Poll);
            }
            Some(at) => event_loop.set_control_flow(ControlFlow::WaitUntil(at)),
            None => event_loop.set_control_flow(ControlFlow::Wait),
        }
    }

    fn exiting(&mut self, _event_loop: &winit::event_loop::ActiveEventLoop) {
        if let Some(Stage::Glow { egui_glow, .. }) = self.stage.as_mut() {
            egui_glow.destroy();
        }
    }
}

fn elapsed_ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

fn injected_failure(stage: HostFailureStage) -> Option<String> {
    let requested = std::env::var(FAIL_STAGE_ENV).ok()?;
    (requested == stage.key()).then(|| format!("injected handoff failure at {}", stage.key()))
}
