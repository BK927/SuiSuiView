#![allow(unsafe_code)]

use super::runtime::{AppRuntime, StartupReveal};
use super::{StartupOpen, SuiSuiViewApp};
use crate::core::state::{AppSettings, RendererMode, StateStore};
use crossbeam_channel::Receiver;
use egui::{self, ViewportId};
use egui_wgpu::winit::Painter;
use egui_wgpu::{wgpu, WgpuConfiguration, WgpuSetupExisting};
use egui_winit::winit;
use serde::Serialize;
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

mod glow_window;
mod prewarm;

use glow_window::{create_gl_display, create_plain_window, GlutinWindowContext};
use prewarm::{run_wgpu_prewarm, PrewarmReport, PrewarmedWgpu};

const REQUEST_ARG: &str = "--experimental-app-handoff";
const REQUEST_ENV: &str = "SUISUIVIEW_EXPERIMENT_APP_HANDOFF";
const FAIL_STAGE_ENV: &str = "SUISUIVIEW_HANDOFF_FAIL_STAGE";
#[cfg(target_os = "windows")]
static TITLE_SYNC_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub(crate) struct HandoffPreviewOptions {
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

pub(crate) fn enabled_for_settings(settings: &AppSettings) -> bool {
    matches!(settings.renderer_mode, RendererMode::Wgpu)
}

pub(crate) fn run(options: HandoffPreviewOptions, wgpu_direct: bool) -> Result<(), HandoffFailure> {
    let event_loop = winit::event_loop::EventLoop::<()>::new()
        .map_err(|error| HandoffFailure::new(HandoffFailureStage::Unknown, error.to_string()))?;
    let wake_proxy = event_loop.create_proxy();
    let mut app = HandoffPreviewApp::new(options, wgpu_direct, wake_proxy);
    let result = event_loop.run_app(&mut app);
    if let Some(failure) = app.failure.take() {
        return Err(failure);
    }
    result.map_err(|error| {
        HandoffFailure::with_metrics(
            HandoffFailureStage::Unknown,
            format!("failed to run app handoff preview: {error}"),
            app.metrics.clone(),
        )
    })
}

#[derive(Debug, Clone)]
pub(crate) struct HandoffFailure {
    pub(crate) stage: HandoffFailureStage,
    pub(crate) error: String,
    pub(crate) metrics: HandoffPreviewMetrics,
}

impl HandoffFailure {
    fn new(stage: HandoffFailureStage, error: String) -> Self {
        Self {
            stage,
            error: error.clone(),
            metrics: HandoffPreviewMetrics {
                error: Some(error),
                ..HandoffPreviewMetrics::default()
            },
        }
    }

    fn with_metrics(
        stage: HandoffFailureStage,
        error: String,
        mut metrics: HandoffPreviewMetrics,
    ) -> Self {
        metrics.error = Some(error.clone());
        Self {
            stage,
            error,
            metrics,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) enum HandoffFailureStage {
    GlCreate,
    GlSwap,
    WgpuPrewarm,
    FirstWgpuFrame,
    Unknown,
}

impl HandoffFailureStage {
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

struct HandoffPreviewApp {
    options: Option<HandoffPreviewOptions>,
    // true: build the window and attach a WGPU surface directly (renderer_mode
    // Wgpu). false: run the Glow-only host (LowMemoryGlow) for the whole session.
    wgpu_direct: bool,
    started_at: Instant,
    stage: Option<Stage>,
    prewarm_rx: Option<mpsc::Receiver<PrewarmReport>>,
    prewarmed_wgpu: Option<PrewarmedWgpu>,
    metrics: HandoffPreviewMetrics,
    failure: Option<HandoffFailure>,
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
pub(crate) struct HandoffPreviewMetrics {
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

impl HandoffPreviewApp {
    fn new(
        options: HandoffPreviewOptions,
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
            metrics: HandoffPreviewMetrics::default(),
            failure: None,
            summary_printed: false,
            redraw_deadline: Arc::new(Mutex::new(None)),
            wake_proxy,
        }
    }

    fn start_glow(
        &mut self,
        event_loop: &winit::event_loop::ActiveEventLoop,
    ) -> Result<(), String> {
        if let Some(error) = injected_failure(HandoffFailureStage::GlCreate) {
            return Err(error);
        }
        let options = self
            .options
            .take()
            .ok_or_else(|| "handoff preview options were already consumed".to_owned())?;
        let (gl_window, gl) = create_gl_display(
            event_loop,
            &options.store,
            options.icon,
            options.default_window_size,
            options.min_window_size,
        )?;
        let gl = Arc::new(gl);
        let egui_glow = egui_glow::EguiGlow::new(
            event_loop,
            gl.clone(),
            None,
            Some(gl_window.window().scale_factor() as f32),
            true,
        );
        self.install_repaint_callback(&egui_glow.egui_ctx);
        let app = SuiSuiViewApp::new(
            AppRuntime::new(egui_glow.egui_ctx.clone(), StartupReveal::HostManaged),
            options.store,
            options.ipc_rx,
            options.startup_open_path,
            options.startup_open,
        );
        gl_window.window().request_redraw();
        self.stage = Some(Stage::Glow {
            app,
            gl_window,
            gl,
            egui_glow,
        });
        Ok(())
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

    fn redraw_glow(
        &mut self,
        event_loop: &winit::event_loop::ActiveEventLoop,
        mut app: SuiSuiViewApp,
        gl_window: GlutinWindowContext,
        gl: Arc<glow::Context>,
        mut egui_glow: egui_glow::EguiGlow,
    ) {
        self.poll_prewarm();
        if let Some(error) = self.metrics.prewarm_error.clone() {
            self.fail(event_loop, HandoffFailureStage::WgpuPrewarm, error);
            return;
        }
        // Cleared before the frame so the app's repaint requests during
        // `update_frame` (via the callback) set a fresh deadline for the loop.
        *self.redraw_deadline.lock().unwrap() = None;
        egui_glow.run(gl_window.window(), |ctx| app.update_frame(ctx));
        if app.close_requested {
            self.print_summary();
            event_loop.exit();
            return;
        }

        unsafe {
            use glow::HasContext as _;
            gl.clear_color(0.015, 0.016, 0.020, 1.0);
            gl.clear(glow::COLOR_BUFFER_BIT);
        }
        egui_glow.paint(gl_window.window());
        if let Some(error) = injected_failure(HandoffFailureStage::GlSwap) {
            self.fail(event_loop, HandoffFailureStage::GlSwap, error);
            return;
        }
        if let Err(error) = gl_window.swap_buffers() {
            self.fail(event_loop, HandoffFailureStage::GlSwap, error);
            return;
        }

        let now_ms = elapsed_ms(self.started_at.elapsed());
        self.metrics.last_glow_present_ms = Some(now_ms);
        if self.metrics.first_glow_present_ms.is_none() {
            self.metrics.first_glow_present_ms = Some(now_ms);
            gl_window.reveal_after_first_frame();
            schedule_process_visible_window_title("SuiSuiView".to_owned());
        }
        sync_visible_window_title(gl_window.window());

        // The Glow host is reactive: the next redraw is scheduled by egui's
        // repaint deadline (see `about_to_wait`) or by input in `window_event`.
        self.stage = Some(Stage::Glow {
            app,
            gl_window,
            gl,
            egui_glow,
        });
    }

    fn redraw_wgpu(
        &mut self,
        event_loop: &winit::event_loop::ActiveEventLoop,
        mut app: SuiSuiViewApp,
        window: winit::window::Window,
        mut egui_state: egui_winit::State,
        mut painter: Painter,
        mut viewport_info: egui::ViewportInfo,
    ) {
        // Cleared before the frame so this frame's repaint requests (via the
        // shared egui repaint callback) set a fresh deadline for `about_to_wait`.
        *self.redraw_deadline.lock().unwrap() = None;
        painter.handle_screenshots(&mut egui_state.egui_input_mut().events);
        let raw_input = egui_state.take_egui_input(&window);
        let egui_ctx = egui_state.egui_ctx().clone();
        let full_output = egui_ctx.run(raw_input, |ctx| app.update_frame(ctx));
        let requested_title = process_wgpu_viewport_output(
            &egui_ctx,
            &window,
            &mut viewport_info,
            full_output.viewport_output,
        );
        if let Some(title) = requested_title {
            window.set_title(&title);
            schedule_process_visible_window_title(title);
        }
        sync_visible_window_title(&window);
        egui_state.handle_platform_output(&window, full_output.platform_output);
        let clipped_primitives =
            egui_ctx.tessellate(full_output.shapes, full_output.pixels_per_point);
        if let Some(error) = injected_failure(HandoffFailureStage::FirstWgpuFrame) {
            self.fail(event_loop, HandoffFailureStage::FirstWgpuFrame, error);
            return;
        }
        painter.paint_and_update_textures(
            ViewportId::ROOT,
            full_output.pixels_per_point,
            [0.015, 0.016, 0.020, 1.0],
            &clipped_primitives,
            &full_output.textures_delta,
            Vec::new(),
        );

        if self.metrics.first_wgpu_present_ms.is_none() {
            let now_ms = elapsed_ms(self.started_at.elapsed());
            self.metrics.first_wgpu_present_ms = Some(now_ms);
            // The flash guard (MaskMainUntilRevealed) has kept the main window
            // masked (layered alpha 0) since it was shown inside create_window;
            // now that a frame is painted, release the mask so it appears already
            // rendered. set_visible is idempotent here (the window is already
            // WS_VISIBLE) but keeps the intent explicit.
            crate::startup_window::reveal_main_windows();
            window.set_visible(true);
        }

        if app_requested_close(&viewport_info) {
            self.stage = Some(Stage::Wgpu {
                app,
                window,
                egui_state,
                painter,
                viewport_info,
            });
            self.print_summary();
            event_loop.exit();
            return;
        }

        // Reactive: the next WGPU frame is scheduled by egui's repaint deadline
        // (see `about_to_wait`) or by input in `window_event`, not every frame.
        self.stage = Some(Stage::Wgpu {
            app,
            window,
            egui_state,
            painter,
            viewport_info,
        });
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

    /// WGPU-direct startup: skip the Glow stage and the handoff entirely —
    /// create a plain window and attach WGPU straight away. The WGPU device is
    /// prewarmed concurrently with window creation; the window stays hidden
    /// until the first WGPU frame reveals it (see `redraw_wgpu`).
    fn start_wgpu_direct(
        &mut self,
        event_loop: &winit::event_loop::ActiveEventLoop,
    ) -> Result<(), String> {
        let options = self
            .options
            .take()
            .ok_or_else(|| "handoff preview options were already consumed".to_owned())?;

        // Build the WGPU device on a worker thread while we create the window.
        self.start_prewarm();
        let window = create_plain_window(
            event_loop,
            &options.store,
            options.icon,
            options.default_window_size,
            options.min_window_size,
        )?;
        self.wait_for_prewarm();

        let egui_ctx = egui::Context::default();
        self.install_repaint_callback(&egui_ctx);

        let mut config = tuned_wgpu_configuration();
        if let Some(prewarmed) = self.prewarmed_wgpu.take() {
            config.wgpu_setup = WgpuSetupExisting {
                instance: prewarmed.instance,
                adapter: prewarmed.adapter,
                device: prewarmed.device,
                queue: prewarmed.queue,
            }
            .into();
            self.metrics.used_prewarmed_wgpu = true;
        }

        let painter_started = Instant::now();
        let mut painter =
            pollster::block_on(Painter::new(egui_ctx.clone(), config, 1, None, false, true));
        self.metrics.wgpu_painter_new_ms = Some(elapsed_ms(painter_started.elapsed()));

        let set_window_started = Instant::now();
        let set_window_result = unsafe {
            pollster::block_on(painter.set_window_unsafe(ViewportId::ROOT, Some(&window)))
        };
        if let Err(error) = set_window_result {
            return Err(format!("failed to attach WGPU surface: {error}"));
        }
        self.metrics.wgpu_set_window_ms = Some(elapsed_ms(set_window_started.elapsed()));

        let target_format = painter
            .render_state()
            .map(|state| state.target_format)
            .ok_or_else(|| "WGPU render state was not initialized".to_owned())?;

        let mut egui_state = egui_winit::State::new(
            egui_ctx.clone(),
            ViewportId::ROOT,
            event_loop,
            Some(window.scale_factor() as f32),
            window.theme(),
            painter.max_texture_side(),
        );
        let focused = window.has_focus();
        egui_state.egui_input_mut().focused = focused;
        egui_state
            .egui_input_mut()
            .events
            .push(egui::Event::WindowFocused(focused));

        let mut app = SuiSuiViewApp::new(
            AppRuntime::new(egui_ctx, StartupReveal::HostManaged),
            options.store,
            options.ipc_rx,
            options.startup_open_path,
            options.startup_open,
        );
        app.gpu_effects_available = true;
        app.gpu_target_format = Some(target_format);

        window.request_redraw();
        self.stage = Some(Stage::Wgpu {
            app,
            window,
            egui_state,
            painter,
            viewport_info: egui::ViewportInfo::default(),
        });
        Ok(())
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
                if let Some(error) = injected_failure(HandoffFailureStage::WgpuPrewarm) {
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
        stage: HandoffFailureStage,
        error: String,
    ) {
        self.metrics.error = Some(error.clone());
        self.failure = Some(HandoffFailure::with_metrics(
            stage,
            error,
            self.metrics.clone(),
        ));
        self.print_summary();
        event_loop.exit();
    }

    fn resize_wgpu_surface(painter: &mut Painter, size: winit::dpi::PhysicalSize<u32>) {
        let Some(width) = NonZeroU32::new(size.width) else {
            return;
        };
        let Some(height) = NonZeroU32::new(size.height) else {
            return;
        };
        painter.on_window_resized(ViewportId::ROOT, width, height);
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

impl winit::application::ApplicationHandler<()> for HandoffPreviewApp {
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
            self.fail(event_loop, HandoffFailureStage::GlCreate, error);
        }
    }

    fn window_event(
        &mut self,
        event_loop: &winit::event_loop::ActiveEventLoop,
        _window_id: winit::window::WindowId,
        event: winit::event::WindowEvent,
    ) {
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
                } => {
                    if let winit::event::WindowEvent::Resized(size) = event {
                        gl_window.resize(size);
                        gl_window.window().request_redraw();
                        return;
                    }
                    let response = egui_glow.on_window_event(gl_window.window(), &event);
                    if response.repaint {
                        gl_window.window().request_redraw();
                    }
                }
                Stage::Wgpu {
                    window,
                    egui_state,
                    painter,
                    ..
                } => {
                    if let winit::event::WindowEvent::Resized(size) = event {
                        Self::resize_wgpu_surface(painter, size);
                        window.request_redraw();
                        return;
                    }
                    let response = egui_state.on_window_event(window, &event);
                    if response.repaint {
                        window.request_redraw();
                    }
                }
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

fn process_wgpu_viewport_output(
    egui_ctx: &egui::Context,
    window: &winit::window::Window,
    viewport_info: &mut egui::ViewportInfo,
    viewport_output: egui::ViewportIdMap<egui::ViewportOutput>,
) -> Option<String> {
    let mut requested_title = None;
    for (_, output) in viewport_output {
        for command in &output.commands {
            if let egui::ViewportCommand::Title(title) = command {
                requested_title = Some(title.clone());
            }
        }
        let mut actions_requested = Default::default();
        egui_winit::process_viewport_commands(
            egui_ctx,
            viewport_info,
            output.commands,
            window,
            &mut actions_requested,
        );
    }
    requested_title
}

#[cfg(target_os = "windows")]
fn sync_visible_window_title(window: &winit::window::Window) {
    let title = window.title();
    let title = if title.is_empty() {
        "SuiSuiView"
    } else {
        title.as_str()
    };
    set_process_visible_window_title(title);
}

#[cfg(not(target_os = "windows"))]
fn sync_visible_window_title(_window: &winit::window::Window) {}

#[cfg(target_os = "windows")]
fn schedule_process_visible_window_title(title: String) {
    let generation = next_title_sync_generation();
    set_process_visible_window_title(&title);
    std::thread::Builder::new()
        .name("suisuiview-handoff-title-sync".to_owned())
        .spawn(move || {
            for delay in [
                Duration::from_millis(80),
                Duration::from_millis(320),
                Duration::from_millis(900),
            ] {
                std::thread::sleep(delay);
                if !title_sync_generation_matches(generation) {
                    break;
                }
                set_process_visible_window_title(&title);
            }
        })
        .ok();
}

#[cfg(not(target_os = "windows"))]
fn schedule_process_visible_window_title(_title: String) {}

#[cfg(target_os = "windows")]
fn next_title_sync_generation() -> u64 {
    use std::sync::atomic::Ordering;

    TITLE_SYNC_GENERATION.fetch_add(1, Ordering::Relaxed) + 1
}

#[cfg(target_os = "windows")]
fn title_sync_generation_matches(generation: u64) -> bool {
    use std::sync::atomic::Ordering;

    TITLE_SYNC_GENERATION.load(Ordering::Relaxed) == generation
}

#[cfg(target_os = "windows")]
fn set_process_visible_window_title(title: &str) {
    use windows_sys::Win32::Foundation::{HWND, LPARAM};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowThreadProcessId, IsWindowVisible, SetWindowTextW,
    };

    struct TitleUpdate {
        process_id: u32,
        title: *const u16,
    }

    unsafe extern "system" fn enum_window(hwnd: HWND, lparam: LPARAM) -> i32 {
        let update = &*(lparam as *const TitleUpdate);
        let mut window_process_id = 0;
        GetWindowThreadProcessId(hwnd, &mut window_process_id);
        if window_process_id == update.process_id && IsWindowVisible(hwnd) != 0 {
            SetWindowTextW(hwnd, update.title);
        }
        1
    }

    let title: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
    let update = TitleUpdate {
        process_id: std::process::id(),
        title: title.as_ptr(),
    };
    unsafe {
        EnumWindows(Some(enum_window), &update as *const TitleUpdate as isize);
    }
}

#[cfg(not(target_os = "windows"))]
fn set_process_visible_window_title(_title: &str) {}

fn app_requested_close(viewport_info: &egui::ViewportInfo) -> bool {
    viewport_info
        .events
        .iter()
        .any(|event| matches!(event, egui::ViewportEvent::Close))
}

fn tuned_wgpu_configuration() -> WgpuConfiguration {
    let mut config = WgpuConfiguration::default();
    if let egui_wgpu::WgpuSetup::CreateNew(create_new) = &mut config.wgpu_setup {
        create_new.instance_descriptor.backends =
            wgpu::Backends::from_env().unwrap_or(wgpu::Backends::PRIMARY);
        create_new.instance_descriptor.flags = wgpu::InstanceFlags::empty().with_env();
    }
    config
}

fn elapsed_ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

fn injected_failure(stage: HandoffFailureStage) -> Option<String> {
    let requested = std::env::var(FAIL_STAGE_ENV).ok()?;
    (requested == stage.key()).then(|| format!("injected handoff failure at {}", stage.key()))
}
