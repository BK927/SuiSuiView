use std::num::NonZeroU32;
use std::sync::{mpsc, Arc};
use std::time::Instant;

use egui::ViewportId;
use egui_wgpu::winit::Painter;
use egui_wgpu::{WgpuConfiguration, WgpuSetupExisting};
use egui_winit::winit;

use super::glow_window::{create_display, GlutinWindowContext};
use super::handoff_prewarm::{run_wgpu_prewarm, PrewarmReport, PrewarmedWgpu};
use super::handoff_ui::draw_handoff_ui;
use super::wgpu_worker::elapsed_ms;

const HANDOFF_AFTER_GLOW_FRAMES: u32 = 3;
const AUTO_CLOSE_AFTER_WGPU_FRAMES: u32 = 4;

pub(crate) fn run_handoff_probe(
    started_at: Instant,
    auto_close_after_report: bool,
    prewarm_wgpu: bool,
) -> Result<(), String> {
    let event_loop = winit::event_loop::EventLoop::<()>::new()
        .map_err(|error| format!("failed to build winit event loop: {error}"))?;
    let mut app = HandoffProbeApp::new(started_at, auto_close_after_report, prewarm_wgpu);
    event_loop
        .run_app(&mut app)
        .map_err(|error| format!("failed to run handoff probe: {error}"))
}

enum Stage {
    Glow {
        gl_window: GlutinWindowContext,
        gl: Arc<glow::Context>,
        egui_glow: egui_glow::EguiGlow,
        frames: u32,
    },
    Wgpu {
        window: winit::window::Window,
        egui_state: egui_winit::State,
        painter: Painter,
        frames: u32,
    },
}

#[derive(Default)]
pub(super) struct HandoffMetrics {
    pub(super) first_glow_visible_ms: Option<f64>,
    pub(super) last_glow_present_ms: Option<f64>,
    pub(super) glow_frame_ms: Option<f64>,
    pub(super) handoff_started_ms: Option<f64>,
    pub(super) glow_destroy_ms: Option<f64>,
    pub(super) context_destroy_ms: Option<f64>,
    pub(super) painter_new_ms: Option<f64>,
    pub(super) set_window_ms: Option<f64>,
    pub(super) first_wgpu_present_ms: Option<f64>,
    pub(super) first_wgpu_frame_ms: Option<f64>,
    pub(super) wgpu_vsync_wait_ms: Option<f64>,
    pub(super) handoff_gap_ms: Option<f64>,
    glow_focus_before_handoff: Option<bool>,
    wgpu_focus_after_handoff: Option<bool>,
    glow_input_focused_before_handoff: Option<bool>,
    wgpu_input_focused_after_handoff: Option<bool>,
    text_preserved_after_handoff: Option<bool>,
    window_focused_at_handoff: Option<bool>,
    prewarm_started_ms: Option<f64>,
    prewarm_ready_ms: Option<f64>,
    prewarm_init_ms: Option<f64>,
    prewarm_backend: Option<String>,
    prewarm_device_type: Option<String>,
    used_prewarmed_wgpu: bool,
    prewarm_error: Option<String>,
    error: Option<String>,
}

struct HandoffProbeApp {
    started_at: Instant,
    auto_close_after_report: bool,
    prewarm_wgpu: bool,
    stage: Option<Stage>,
    prewarm_rx: Option<mpsc::Receiver<PrewarmReport>>,
    prewarmed_wgpu: Option<PrewarmedWgpu>,
    metrics: HandoffMetrics,
    probe_text: String,
    focus_id: egui::Id,
    focus_requested: bool,
    summary_printed: bool,
}

impl HandoffProbeApp {
    fn new(started_at: Instant, auto_close_after_report: bool, prewarm_wgpu: bool) -> Self {
        Self {
            started_at,
            auto_close_after_report,
            prewarm_wgpu,
            stage: None,
            prewarm_rx: None,
            prewarmed_wgpu: None,
            metrics: HandoffMetrics::default(),
            probe_text: "state-before-handoff".to_owned(),
            focus_id: egui::Id::new("handoff-focus-field"),
            focus_requested: false,
            summary_printed: false,
        }
    }

    fn redraw(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        let Some(stage) = self.stage.take() else {
            return;
        };
        match stage {
            Stage::Glow {
                gl_window,
                gl,
                egui_glow,
                frames,
            } => self.redraw_glow(event_loop, gl_window, gl, egui_glow, frames),
            Stage::Wgpu {
                window,
                egui_state,
                painter,
                frames,
            } => self.redraw_wgpu(event_loop, window, egui_state, painter, frames),
        }
    }

    fn redraw_glow(
        &mut self,
        event_loop: &winit::event_loop::ActiveEventLoop,
        gl_window: GlutinWindowContext,
        gl: Arc<glow::Context>,
        mut egui_glow: egui_glow::EguiGlow,
        frames: u32,
    ) {
        let frame_started = Instant::now();
        self.poll_prewarm();
        let mut quit = false;
        let mut focus_has = false;
        let mut input_focused = false;
        let phase = if frames + 1 >= HANDOFF_AFTER_GLOW_FRAMES {
            "Glow: final frame before WGPU"
        } else {
            "Glow: visible startup frame"
        };

        egui_glow.run(gl_window.window(), |ctx| {
            quit = draw_handoff_ui(
                ctx,
                self.started_at,
                phase,
                &mut self.probe_text,
                self.focus_id,
                &mut self.focus_requested,
                &self.metrics,
            );
            focus_has = ctx.memory(|memory| memory.has_focus(self.focus_id));
            input_focused = ctx.input(|input| input.focused);
        });

        unsafe {
            use glow::HasContext as _;
            gl.clear_color(0.025, 0.080, 0.060, 1.0);
            gl.clear(glow::COLOR_BUFFER_BIT);
        }
        egui_glow.paint(gl_window.window());
        if let Err(error) = gl_window.swap_buffers() {
            self.metrics.error = Some(error);
            self.print_summary();
            event_loop.exit();
            return;
        }

        let now_ms = elapsed_ms(self.started_at.elapsed());
        self.metrics.last_glow_present_ms = Some(now_ms);
        self.metrics.glow_frame_ms = Some(elapsed_ms(frame_started.elapsed()));
        if self.metrics.first_glow_visible_ms.is_none() {
            self.metrics.first_glow_visible_ms = Some(now_ms);
            gl_window.window().set_visible(true);
        }

        let frames = frames + 1;
        if quit {
            self.stage = Some(Stage::Glow {
                gl_window,
                gl,
                egui_glow,
                frames,
            });
            self.print_summary();
            event_loop.exit();
            return;
        }
        let prewarm_ready = !self.prewarm_wgpu
            || self.prewarmed_wgpu.is_some()
            || self.metrics.prewarm_error.is_some();
        if frames >= HANDOFF_AFTER_GLOW_FRAMES && prewarm_ready {
            self.metrics.glow_focus_before_handoff = Some(focus_has);
            self.metrics.glow_input_focused_before_handoff = Some(input_focused);
            self.metrics.handoff_started_ms = Some(elapsed_ms(self.started_at.elapsed()));
            self.begin_handoff(event_loop, gl_window, gl, egui_glow);
            return;
        }

        gl_window.window().request_redraw();
        self.stage = Some(Stage::Glow {
            gl_window,
            gl,
            egui_glow,
            frames,
        });
    }

    fn begin_handoff(
        &mut self,
        event_loop: &winit::event_loop::ActiveEventLoop,
        gl_window: GlutinWindowContext,
        gl: Arc<glow::Context>,
        mut egui_glow: egui_glow::EguiGlow,
    ) {
        let egui_ctx = egui_glow.egui_ctx.clone();

        let destroy_started = Instant::now();
        egui_glow.destroy();
        drop(egui_glow);
        drop(gl);
        self.metrics.glow_destroy_ms = Some(elapsed_ms(destroy_started.elapsed()));

        let context_destroy_started = Instant::now();
        let window = match gl_window.into_window_after_context_destroy() {
            Ok(window) => window,
            Err(error) => {
                self.metrics.error = Some(error);
                self.print_summary();
                event_loop.exit();
                return;
            }
        };
        self.metrics.context_destroy_ms = Some(elapsed_ms(context_destroy_started.elapsed()));
        self.metrics.window_focused_at_handoff = Some(window.has_focus());
        window.set_title("SuiSuiView renderer handoff probe - WGPU");

        let mut config = WgpuConfiguration::default();
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
        self.metrics.painter_new_ms = Some(elapsed_ms(painter_started.elapsed()));

        let set_window_started = Instant::now();
        let set_window_result = unsafe {
            pollster::block_on(painter.set_window_unsafe(ViewportId::ROOT, Some(&window)))
        };
        if let Err(error) = set_window_result {
            self.metrics.error = Some(format!("failed to attach WGPU surface: {error}"));
            self.print_summary();
            event_loop.exit();
            return;
        }
        self.metrics.set_window_ms = Some(elapsed_ms(set_window_started.elapsed()));

        let mut egui_state = egui_winit::State::new(
            egui_ctx,
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

        window.request_redraw();
        self.stage = Some(Stage::Wgpu {
            window,
            egui_state,
            painter,
            frames: 0,
        });
    }

    fn redraw_wgpu(
        &mut self,
        event_loop: &winit::event_loop::ActiveEventLoop,
        window: winit::window::Window,
        mut egui_state: egui_winit::State,
        mut painter: Painter,
        frames: u32,
    ) {
        let frame_started = Instant::now();
        painter.handle_screenshots(&mut egui_state.egui_input_mut().events);
        let raw_input = egui_state.take_egui_input(&window);
        let egui_ctx = egui_state.egui_ctx().clone();
        let mut quit = false;
        let mut focus_has = false;
        let mut input_focused = false;
        let full_output = egui_ctx.run(raw_input, |ctx| {
            quit = draw_handoff_ui(
                ctx,
                self.started_at,
                "WGPU: same window after handoff",
                &mut self.probe_text,
                self.focus_id,
                &mut self.focus_requested,
                &self.metrics,
            );
            focus_has = ctx.memory(|memory| memory.has_focus(self.focus_id));
            input_focused = ctx.input(|input| input.focused);
        });
        egui_state.handle_platform_output(&window, full_output.platform_output);
        let clipped_primitives =
            egui_ctx.tessellate(full_output.shapes, full_output.pixels_per_point);
        let vsync_seconds = painter.paint_and_update_textures(
            ViewportId::ROOT,
            full_output.pixels_per_point,
            [0.030, 0.045, 0.090, 1.0],
            &clipped_primitives,
            &full_output.textures_delta,
            Vec::new(),
        );

        let now_ms = elapsed_ms(self.started_at.elapsed());
        if self.metrics.first_wgpu_present_ms.is_none() {
            self.metrics.first_wgpu_present_ms = Some(now_ms);
            self.metrics.first_wgpu_frame_ms = Some(elapsed_ms(frame_started.elapsed()));
            self.metrics.wgpu_vsync_wait_ms = Some(f64::from(vsync_seconds) * 1000.0);
            self.metrics.wgpu_focus_after_handoff = Some(focus_has);
            self.metrics.wgpu_input_focused_after_handoff = Some(input_focused);
            self.metrics.text_preserved_after_handoff =
                Some(self.probe_text == "state-before-handoff");
            if let Some(last_glow_ms) = self.metrics.last_glow_present_ms {
                self.metrics.handoff_gap_ms = Some(now_ms - last_glow_ms);
            }
        }

        let frames = frames + 1;
        if quit || (self.auto_close_after_report && frames >= AUTO_CLOSE_AFTER_WGPU_FRAMES) {
            self.stage = Some(Stage::Wgpu {
                window,
                egui_state,
                painter,
                frames,
            });
            self.print_summary();
            event_loop.exit();
            return;
        }

        window.request_redraw();
        self.stage = Some(Stage::Wgpu {
            window,
            egui_state,
            painter,
            frames,
        });
    }

    fn start_prewarm(&mut self) {
        if !self.prewarm_wgpu || self.prewarm_rx.is_some() || self.prewarmed_wgpu.is_some() {
            return;
        }
        self.metrics.prewarm_started_ms = Some(elapsed_ms(self.started_at.elapsed()));
        let started_at = self.started_at;
        let (sender, receiver) = mpsc::channel();
        std::thread::Builder::new()
            .name("suisuiview-handoff-prewarm-wgpu".to_owned())
            .spawn(move || {
                let _ = sender.send(run_wgpu_prewarm(started_at));
            })
            .expect("failed to spawn WGPU prewarm thread");
        self.prewarm_rx = Some(receiver);
    }

    fn poll_prewarm(&mut self) {
        let Some(receiver) = self.prewarm_rx.take() else {
            return;
        };
        match receiver.try_recv() {
            Ok(report) => {
                self.metrics.prewarm_ready_ms = Some(report.ready_ms);
                self.metrics.prewarm_init_ms = Some(report.init_ms);
                self.metrics.prewarm_backend = report.backend;
                self.metrics.prewarm_device_type = report.device_type;
                match report.result {
                    Ok(prewarmed) => {
                        self.prewarmed_wgpu = Some(prewarmed);
                    }
                    Err(error) => {
                        self.metrics.prewarm_error = Some(error);
                    }
                }
            }
            Err(mpsc::TryRecvError::Empty) => {
                self.prewarm_rx = Some(receiver);
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.metrics.prewarm_error = Some("prewarm thread disconnected".to_owned());
            }
        }
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
            "runtime_probe_handoff glow_first_visible_ms={:.3} glow_frame_ms={:.3} last_glow_present_ms={:.3} handoff_started_ms={:.3} glow_destroy_ms={:.3} gl_context_destroy_ms={:.3} wgpu_painter_new_ms={:.3} wgpu_set_window_ms={:.3} first_wgpu_present_ms={:.3} first_wgpu_frame_ms={:.3} handoff_gap_ms={:.3} wgpu_vsync_wait_ms={:.3} glow_focus_before={} wgpu_focus_after={} glow_input_focused={} wgpu_input_focused={} text_preserved={} window_focused_at_handoff={} error={}",
            self.metrics.first_glow_visible_ms.unwrap_or(-1.0),
            self.metrics.glow_frame_ms.unwrap_or(-1.0),
            self.metrics.last_glow_present_ms.unwrap_or(-1.0),
            self.metrics.handoff_started_ms.unwrap_or(-1.0),
            self.metrics.glow_destroy_ms.unwrap_or(-1.0),
            self.metrics.context_destroy_ms.unwrap_or(-1.0),
            self.metrics.painter_new_ms.unwrap_or(-1.0),
            self.metrics.set_window_ms.unwrap_or(-1.0),
            self.metrics.first_wgpu_present_ms.unwrap_or(-1.0),
            self.metrics.first_wgpu_frame_ms.unwrap_or(-1.0),
            self.metrics.handoff_gap_ms.unwrap_or(-1.0),
            self.metrics.wgpu_vsync_wait_ms.unwrap_or(-1.0),
            self.metrics.glow_focus_before_handoff.unwrap_or(false),
            self.metrics.wgpu_focus_after_handoff.unwrap_or(false),
            self.metrics
                .glow_input_focused_before_handoff
                .unwrap_or(false),
            self.metrics
                .wgpu_input_focused_after_handoff
                .unwrap_or(false),
            self.metrics.text_preserved_after_handoff.unwrap_or(false),
            self.metrics.window_focused_at_handoff.unwrap_or(false),
            self.metrics.error.as_deref().unwrap_or("none")
        );
        println!(
            "runtime_probe_handoff_prewarm enabled={} started_ms={:.3} ready_ms={:.3} init_ms={:.3} backend={} device_type={} used={} error={}",
            self.prewarm_wgpu,
            self.metrics.prewarm_started_ms.unwrap_or(-1.0),
            self.metrics.prewarm_ready_ms.unwrap_or(-1.0),
            self.metrics.prewarm_init_ms.unwrap_or(-1.0),
            self.metrics.prewarm_backend.as_deref().unwrap_or("unknown"),
            self.metrics.prewarm_device_type.as_deref().unwrap_or("unknown"),
            self.metrics.used_prewarmed_wgpu,
            self.metrics.prewarm_error.as_deref().unwrap_or("none")
        );
    }
}

impl winit::application::ApplicationHandler<()> for HandoffProbeApp {
    fn resumed(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        if self.stage.is_some() {
            return;
        }
        let (gl_window, gl) = match create_display(event_loop) {
            Ok(value) => value,
            Err(error) => {
                self.metrics.error = Some(error);
                self.print_summary();
                event_loop.exit();
                return;
            }
        };
        let gl = Arc::new(gl);
        let egui_glow = egui_glow::EguiGlow::new(event_loop, gl.clone(), None, None, true);
        self.start_prewarm();
        gl_window.window().set_visible(true);
        gl_window.window().request_redraw();
        self.stage = Some(Stage::Glow {
            gl_window,
            gl,
            egui_glow,
            frames: 0,
        });
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
        match self.stage.as_ref() {
            Some(Stage::Glow { gl_window, .. }) => {
                gl_window.window().request_redraw();
                event_loop.set_control_flow(winit::event_loop::ControlFlow::Poll);
            }
            Some(Stage::Wgpu { window, frames, .. }) if *frames < AUTO_CLOSE_AFTER_WGPU_FRAMES => {
                window.request_redraw();
                event_loop.set_control_flow(winit::event_loop::ControlFlow::Poll);
            }
            _ => {}
        }
    }

    fn exiting(&mut self, _event_loop: &winit::event_loop::ActiveEventLoop) {
        if let Some(Stage::Glow { egui_glow, .. }) = self.stage.as_mut() {
            egui_glow.destroy();
        }
    }
}
