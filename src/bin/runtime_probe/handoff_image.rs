use std::num::NonZeroU32;
use std::path::PathBuf;
use std::sync::{mpsc, Arc};
use std::time::Instant;

use egui::ViewportId;
use egui_wgpu::winit::Painter;
use egui_wgpu::{WgpuConfiguration, WgpuSetupExisting};
use egui_winit::winit;

use super::glow_window::{create_display, GlutinWindowContext};
use super::handoff_image_report::{print_image_handoff_summary, ImageHandoffMetrics};
use super::handoff_image_ui::draw_image_ui;
use super::handoff_prewarm::{run_wgpu_prewarm, PrewarmReport, PrewarmedWgpu};
use super::image_first_page::{spawn_first_page_prepare, PreparedImageReport};
use super::wgpu_worker::elapsed_ms;

const AUTO_CLOSE_AFTER_WGPU_IMAGE_FRAMES: u32 = 4;

pub(crate) fn run_image_handoff_probe(
    started_at: Instant,
    auto_close_after_report: bool,
    prewarm_wgpu: bool,
    input_path: PathBuf,
    target_long_edge: u32,
) -> Result<(), String> {
    let event_loop = winit::event_loop::EventLoop::<()>::new()
        .map_err(|error| format!("failed to build winit event loop: {error}"))?;
    let mut app = ImageHandoffApp::new(
        started_at,
        auto_close_after_report,
        prewarm_wgpu,
        input_path,
        target_long_edge,
    );
    event_loop
        .run_app(&mut app)
        .map_err(|error| format!("failed to run image handoff probe: {error}"))
}

enum Stage {
    Glow {
        gl_window: GlutinWindowContext,
        gl: Arc<glow::Context>,
        egui_glow: egui_glow::EguiGlow,
        frames_after_image: u32,
    },
    Wgpu {
        window: winit::window::Window,
        egui_state: egui_winit::State,
        painter: Painter,
        frames: u32,
    },
}

struct ImageHandoffApp {
    started_at: Instant,
    auto_close_after_report: bool,
    prewarm_wgpu: bool,
    stage: Option<Stage>,
    image_rx: Option<mpsc::Receiver<PreparedImageReport>>,
    image_report: Option<PreparedImageReport>,
    prewarm_rx: Option<mpsc::Receiver<PrewarmReport>>,
    prewarmed_wgpu: Option<PrewarmedWgpu>,
    glow_texture: Option<egui::TextureHandle>,
    wgpu_texture: Option<egui::TextureHandle>,
    metrics: ImageHandoffMetrics,
    summary_printed: bool,
}

impl ImageHandoffApp {
    fn new(
        started_at: Instant,
        auto_close_after_report: bool,
        prewarm_wgpu: bool,
        input_path: PathBuf,
        target_long_edge: u32,
    ) -> Self {
        let (sender, receiver) = mpsc::channel();
        spawn_first_page_prepare(started_at, input_path, target_long_edge, move |report| {
            let _ = sender.send(report);
        });
        Self {
            started_at,
            auto_close_after_report,
            prewarm_wgpu,
            stage: None,
            image_rx: Some(receiver),
            image_report: None,
            prewarm_rx: None,
            prewarmed_wgpu: None,
            glow_texture: None,
            wgpu_texture: None,
            metrics: ImageHandoffMetrics::default(),
            summary_printed: false,
        }
    }

    fn redraw(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        self.poll_image();
        self.poll_prewarm();
        let Some(stage) = self.stage.take() else {
            return;
        };
        match stage {
            Stage::Glow {
                gl_window,
                gl,
                egui_glow,
                frames_after_image,
            } => self.redraw_glow(event_loop, gl_window, gl, egui_glow, frames_after_image),
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
        frames_after_image: u32,
    ) {
        let mut quit = false;
        let mut loaded_image_this_frame = false;
        egui_glow.run(gl_window.window(), |ctx| {
            if self.glow_texture.is_none() {
                if let Some((image_size, rgba)) = self.image_rgba() {
                    let register_started = Instant::now();
                    let image = egui::ColorImage::from_rgba_unmultiplied(image_size, rgba);
                    self.glow_texture = Some(ctx.load_texture(
                        "handoff-image-glow",
                        image,
                        egui::TextureOptions::LINEAR,
                    ));
                    self.metrics.glow_image_register_ms =
                        Some(elapsed_ms(register_started.elapsed()));
                    loaded_image_this_frame = true;
                }
            }
            quit = draw_image_ui(
                ctx,
                self.started_at,
                "Glow first image",
                self.image_report.as_ref(),
                self.glow_texture.as_ref(),
                &self.metrics,
            );
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
        if self.metrics.first_glow_visible_ms.is_none() {
            self.metrics.first_glow_visible_ms = Some(now_ms);
            gl_window.window().set_visible(true);
        }
        let mut frames_after_image = frames_after_image;
        if self.glow_texture.is_some() {
            frames_after_image = frames_after_image.saturating_add(1);
            self.metrics.last_glow_image_present_ms = Some(now_ms);
            if self.metrics.glow_image_visible_ms.is_none() || loaded_image_this_frame {
                self.metrics.glow_image_visible_ms.get_or_insert(now_ms);
            }
        }

        if quit || self.image_error().is_some() {
            self.stage = Some(Stage::Glow {
                gl_window,
                gl,
                egui_glow,
                frames_after_image,
            });
            self.print_summary();
            event_loop.exit();
            return;
        }

        if frames_after_image >= 2 && self.prewarm_ready() {
            self.metrics.handoff_started_ms = Some(elapsed_ms(self.started_at.elapsed()));
            self.begin_handoff(event_loop, gl_window, gl, egui_glow);
            return;
        }

        gl_window.window().request_redraw();
        self.stage = Some(Stage::Glow {
            gl_window,
            gl,
            egui_glow,
            frames_after_image,
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
        self.glow_texture = None;

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
        window.set_title("SuiSuiView image handoff probe - WGPU");

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
        let image_report = self.image_report.clone();
        let image_rgba = self.image_rgba_owned();
        let wgpu_texture = &mut self.wgpu_texture;
        let metrics = &mut self.metrics;
        let full_output = egui_ctx.run(raw_input, |ctx| {
            if wgpu_texture.is_none() {
                if let Some((image_size, rgba)) = image_rgba.as_ref() {
                    let register_started = Instant::now();
                    let image =
                        egui::ColorImage::from_rgba_unmultiplied(*image_size, rgba.as_slice());
                    *wgpu_texture = Some(ctx.load_texture(
                        "handoff-image-wgpu",
                        image,
                        egui::TextureOptions::LINEAR,
                    ));
                    metrics.wgpu_image_register_ms = Some(elapsed_ms(register_started.elapsed()));
                }
            }
            quit = draw_image_ui(
                ctx,
                self.started_at,
                "WGPU same window image",
                image_report.as_ref(),
                wgpu_texture.as_ref(),
                metrics,
            );
        });
        egui_state.handle_platform_output(&window, full_output.platform_output);
        let clipped_primitives =
            egui_ctx.tessellate(full_output.shapes, full_output.pixels_per_point);
        painter.paint_and_update_textures(
            ViewportId::ROOT,
            full_output.pixels_per_point,
            [0.030, 0.045, 0.090, 1.0],
            &clipped_primitives,
            &full_output.textures_delta,
            Vec::new(),
        );

        let now_ms = elapsed_ms(self.started_at.elapsed());
        if self.metrics.first_wgpu_image_present_ms.is_none() && self.wgpu_texture.is_some() {
            self.metrics.first_wgpu_image_present_ms = Some(now_ms);
            self.metrics.first_wgpu_frame_ms = Some(elapsed_ms(frame_started.elapsed()));
            if let Some(last_glow_ms) = self.metrics.last_glow_image_present_ms {
                self.metrics.handoff_gap_ms = Some(now_ms - last_glow_ms);
            }
        }

        let frames = frames + 1;
        if quit || (self.auto_close_after_report && frames >= AUTO_CLOSE_AFTER_WGPU_IMAGE_FRAMES) {
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

    fn poll_image(&mut self) {
        let Some(receiver) = self.image_rx.take() else {
            return;
        };
        match receiver.try_recv() {
            Ok(report) => {
                self.metrics.image_worker_started_ms = Some(report.worker_started_ms);
                self.metrics.open_source_ms = report.open_source_ms;
                self.metrics.read_page_ms = report.read_page_ms;
                self.metrics.prepare_ms = report.prepare_ms;
                self.metrics.error = report.error.clone();
                self.image_report = Some(report);
            }
            Err(mpsc::TryRecvError::Empty) => {
                self.image_rx = Some(receiver);
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.metrics.error = Some("image worker disconnected".to_owned());
            }
        }
    }

    fn start_prewarm(&mut self) {
        if !self.prewarm_wgpu || self.prewarm_rx.is_some() || self.prewarmed_wgpu.is_some() {
            return;
        }
        self.metrics.prewarm_started_ms = Some(elapsed_ms(self.started_at.elapsed()));
        let started_at = self.started_at;
        let (sender, receiver) = mpsc::channel();
        std::thread::Builder::new()
            .name("suisuiview-image-handoff-prewarm-wgpu".to_owned())
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
                    Ok(prewarmed) => self.prewarmed_wgpu = Some(prewarmed),
                    Err(error) => self.metrics.error = Some(error),
                }
            }
            Err(mpsc::TryRecvError::Empty) => self.prewarm_rx = Some(receiver),
            Err(mpsc::TryRecvError::Disconnected) => {
                self.metrics.error = Some("prewarm worker disconnected".to_owned());
            }
        }
    }

    fn prewarm_ready(&self) -> bool {
        !self.prewarm_wgpu || self.prewarmed_wgpu.is_some() || self.metrics.error.is_some()
    }

    fn image_rgba(&self) -> Option<([usize; 2], &[u8])> {
        let report = self.image_report.as_ref()?;
        Some((report.display_size?, report.rgba.as_ref()?.as_slice()))
    }

    fn image_rgba_owned(&self) -> Option<([usize; 2], Vec<u8>)> {
        let report = self.image_report.as_ref()?;
        Some((report.display_size?, report.rgba.as_ref()?.clone()))
    }

    fn image_error(&self) -> Option<&str> {
        self.image_report
            .as_ref()
            .and_then(|report| report.error.as_deref())
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
        print_image_handoff_summary(&self.metrics, self.image_report.as_ref());
    }
}

impl winit::application::ApplicationHandler<()> for ImageHandoffApp {
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
            frames_after_image: 0,
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
        self.poll_image();
        self.poll_prewarm();
        match self.stage.as_ref() {
            Some(Stage::Glow { gl_window, .. }) => {
                gl_window.window().request_redraw();
                event_loop.set_control_flow(winit::event_loop::ControlFlow::Poll);
            }
            Some(Stage::Wgpu { window, frames, .. })
                if *frames < AUTO_CLOSE_AFTER_WGPU_IMAGE_FRAMES =>
            {
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
