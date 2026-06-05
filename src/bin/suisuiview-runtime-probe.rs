#![allow(unsafe_code)]

mod runtime_probe;

use egui_winit::winit;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use runtime_probe::args::{
    image_worker_mode_from_args, input_path_from_args, target_long_edge_from_args, ImageWorkerMode,
};
use runtime_probe::glow_window::{create_display, GlutinWindowContext};
use runtime_probe::handoff_cli::try_run_handoff_mode;
use runtime_probe::headless::run_headless_worker;
use runtime_probe::image_first_page::{
    spawn_first_page_prepare, PreparedImageReport, DEFAULT_TARGET_LONG_EDGE,
};
use runtime_probe::wgpu_worker::{elapsed_ms, spawn_wgpu_probe, WgpuProbeInput, WgpuProbeReport};

#[derive(Debug)]
enum UserEvent {
    Redraw(Duration),
    ImagePrepared(PreparedImageReport),
    WgpuProbeFinished(WgpuProbeReport),
}

struct GlowProbeApp {
    started_at: Instant,
    proxy: winit::event_loop::EventLoopProxy<UserEvent>,
    auto_close_after_report: bool,
    input_path: Option<PathBuf>,
    gl_window: Option<GlutinWindowContext>,
    gl: Option<Arc<glow::Context>>,
    egui_glow: Option<egui_glow::EguiGlow>,
    repaint_delay: Duration,
    first_visible_ms: Option<f64>,
    last_frame_ms: Option<f64>,
    base_texture_register_ms: Option<f64>,
    texture_register_ms: Option<f64>,
    base_texture: Option<egui::TextureHandle>,
    result_texture: Option<egui::TextureHandle>,
    image_report: Option<PreparedImageReport>,
    wgpu_report: Option<WgpuProbeReport>,
    summary_printed: bool,
}

impl GlowProbeApp {
    fn new(
        started_at: Instant,
        proxy: winit::event_loop::EventLoopProxy<UserEvent>,
        auto_close_after_report: bool,
        input_path: Option<PathBuf>,
        target_long_edge: u32,
        image_worker_mode: ImageWorkerMode,
    ) -> Self {
        if let Some(path) = input_path.clone() {
            let image_proxy = proxy.clone();
            let worker_proxy = proxy.clone();
            spawn_first_page_prepare(started_at, path, target_long_edge, move |report| {
                if let (Some(image_size), Some(rgba)) = (report.display_size, report.rgba.clone()) {
                    let input = image_worker_mode.input(image_size, rgba);
                    spawn_wgpu_probe(started_at, input, move |report| {
                        let _ = worker_proxy.send_event(UserEvent::WgpuProbeFinished(report));
                    });
                }
                let _ = image_proxy.send_event(UserEvent::ImagePrepared(report));
            });
        } else {
            let worker_proxy = proxy.clone();
            spawn_wgpu_probe(started_at, WgpuProbeInput::Synthetic, move |report| {
                let _ = worker_proxy.send_event(UserEvent::WgpuProbeFinished(report));
            });
        }
        Self {
            started_at,
            proxy,
            auto_close_after_report,
            input_path,
            gl_window: None,
            gl: None,
            egui_glow: None,
            repaint_delay: Duration::MAX,
            first_visible_ms: None,
            last_frame_ms: None,
            base_texture_register_ms: None,
            texture_register_ms: None,
            base_texture: None,
            result_texture: None,
            image_report: None,
            wgpu_report: None,
            summary_printed: false,
        }
    }

    fn redraw(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        let frame_started = Instant::now();
        let Some(gl_window) = self.gl_window.as_mut() else {
            return;
        };
        let Some(gl) = self.gl.as_ref() else {
            return;
        };
        let Some(egui_glow) = self.egui_glow.as_mut() else {
            return;
        };

        let started_at = self.started_at;
        let first_visible_ms = self.first_visible_ms;
        let image_report = self.image_report.clone();
        let wgpu_report = self.wgpu_report.clone();
        let base_texture_register_ms = &mut self.base_texture_register_ms;
        let texture_register_ms = &mut self.texture_register_ms;
        let base_texture = &mut self.base_texture;
        let result_texture = &mut self.result_texture;
        let mut quit = false;

        egui_glow.run(gl_window.window(), |ctx| {
            if base_texture.is_none() {
                if let Some(report) = image_report
                    .as_ref()
                    .filter(|report| report.error.is_none())
                {
                    if let (Some(rgba), Some(image_size)) =
                        (report.rgba.as_ref(), report.display_size)
                    {
                        let upload_started = Instant::now();
                        let image =
                            egui::ColorImage::from_rgba_unmultiplied(image_size, rgba.as_slice());
                        *base_texture = Some(ctx.load_texture(
                            "prepared-first-page",
                            image,
                            egui::TextureOptions::LINEAR,
                        ));
                        *base_texture_register_ms = Some(elapsed_ms(upload_started.elapsed()));
                    }
                }
            }
            if result_texture.is_none() {
                if let Some(report) = wgpu_report.as_ref().filter(|report| report.error.is_none()) {
                    if let Some(rgba) = report.rgba.as_ref() {
                        let upload_started = Instant::now();
                        let image = egui::ColorImage::from_rgba_unmultiplied(
                            report.image_size,
                            rgba.as_slice(),
                        );
                        *result_texture = Some(ctx.load_texture(
                            "wgpu-worker-result",
                            image,
                            egui::TextureOptions::LINEAR,
                        ));
                        *texture_register_ms = Some(elapsed_ms(upload_started.elapsed()));
                    }
                }
            }
            quit = draw_probe_ui(
                ctx,
                started_at,
                first_visible_ms,
                image_report.as_ref(),
                wgpu_report.as_ref(),
                base_texture.as_ref(),
                result_texture.as_ref(),
                *base_texture_register_ms,
                *texture_register_ms,
            );
        });

        unsafe {
            use glow::HasContext as _;
            gl.clear_color(0.035, 0.038, 0.043, 1.0);
            gl.clear(glow::COLOR_BUFFER_BIT);
        }
        egui_glow.paint(gl_window.window());
        if let Err(error) = gl_window.swap_buffers() {
            eprintln!("runtime_probe error={error}");
            event_loop.exit();
            return;
        }

        self.last_frame_ms = Some(elapsed_ms(frame_started.elapsed()));
        if self.first_visible_ms.is_none() {
            self.first_visible_ms = Some(elapsed_ms(self.started_at.elapsed()));
            gl_window.window().set_visible(true);
        }

        if quit {
            self.print_summary();
            event_loop.exit();
            return;
        }
        if self.auto_close_after_report
            && self.first_visible_ms.is_some()
            && self.is_probe_complete_for_auto_close()
        {
            self.print_summary();
            event_loop.exit();
        }
    }

    fn is_probe_complete_for_auto_close(&self) -> bool {
        if self.input_path.is_some() {
            return self.image_report.is_some()
                && self.wgpu_report.is_some()
                && (self.result_texture.is_some()
                    || self
                        .wgpu_report
                        .as_ref()
                        .is_some_and(|report| report.error.is_some()));
        }
        self.wgpu_report.is_some()
            && (self.result_texture.is_some()
                || self
                    .wgpu_report
                    .as_ref()
                    .is_some_and(|report| report.error.is_some()))
    }

    fn print_summary(&mut self) {
        if self.summary_printed {
            return;
        }
        self.summary_printed = true;
        if self.input_path.is_some() {
            self.print_image_summary();
            return;
        }
        let Some(report) = self.wgpu_report.as_ref() else {
            println!(
                "runtime_probe first_visible_ms={:.3} no_wgpu_report=true",
                self.first_visible_ms.unwrap_or(-1.0)
            );
            return;
        };
        println!(
            "runtime_probe first_visible_ms={:.3} glow_frame_ms={:.3} wgpu_worker_started_ms={:.3} wgpu_init_ms={:.3} wgpu_compute_readback_ms={:.3} shader_module_ms={:.3} pipeline_ms={:.3} upload_ms={:.3} setup_ms={:.3} encode_submit_ms={:.3} readback_ms={:.3} glow_texture_register_ms={:.3} source={}x{} output={}x{} backend={} device_type={} checksum={} mode={} error={}",
            self.first_visible_ms.unwrap_or(-1.0),
            self.last_frame_ms.unwrap_or(-1.0),
            report.worker_started_ms,
            report.init_ms.unwrap_or(-1.0),
            report.compute_readback_ms.unwrap_or(-1.0),
            report.shader_module_ms.unwrap_or(-1.0),
            report.pipeline_ms.unwrap_or(-1.0),
            report.upload_ms.unwrap_or(-1.0),
            report.setup_ms.unwrap_or(-1.0),
            report.encode_submit_ms.unwrap_or(-1.0),
            report.readback_ms.unwrap_or(-1.0),
            self.texture_register_ms.unwrap_or(-1.0),
            report.source_size[0],
            report.source_size[1],
            report.image_size[0],
            report.image_size[1],
            report.backend.unwrap_or("unknown"),
            report.device_type.unwrap_or("unknown"),
            report.checksum.unwrap_or_default(),
            report.mode,
            report.error.as_deref().unwrap_or("none")
        );
    }

    fn print_image_summary(&self) {
        let Some(image) = self.image_report.as_ref() else {
            println!(
                "runtime_probe_image first_visible_ms={:.3} no_image_report=true",
                self.first_visible_ms.unwrap_or(-1.0)
            );
            return;
        };
        let wgpu = self.wgpu_report.as_ref();
        println!(
            "runtime_probe_image first_visible_ms={:.3} glow_frame_ms={:.3} image_worker_started_ms={:.3} open_source_ms={:.3} read_page_ms={:.3} prepare_ms={:.3} base_texture_register_ms={:.3} wgpu_worker_started_ms={:.3} wgpu_init_ms={:.3} wgpu_work_ms={:.3} shader_module_ms={:.3} pipeline_ms={:.3} upload_ms={:.3} setup_ms={:.3} encode_submit_ms={:.3} readback_ms={:.3} wgpu_texture_register_ms={:.3} page_index={} page_count={} original={}x{} display={}x{} source={}x{} output={}x{} target_long_edge={} decode_backend={} backend={} device_type={} checksum={} mode={} image_error={} wgpu_error={}",
            self.first_visible_ms.unwrap_or(-1.0),
            self.last_frame_ms.unwrap_or(-1.0),
            image.worker_started_ms,
            image.open_source_ms.unwrap_or(-1.0),
            image.read_page_ms.unwrap_or(-1.0),
            image.prepare_ms.unwrap_or(-1.0),
            self.base_texture_register_ms.unwrap_or(-1.0),
            wgpu.map_or(-1.0, |report| report.worker_started_ms),
            wgpu.and_then(|report| report.init_ms).unwrap_or(-1.0),
            wgpu.and_then(|report| report.compute_readback_ms).unwrap_or(-1.0),
            wgpu.and_then(|report| report.shader_module_ms).unwrap_or(-1.0),
            wgpu.and_then(|report| report.pipeline_ms).unwrap_or(-1.0),
            wgpu.and_then(|report| report.upload_ms).unwrap_or(-1.0),
            wgpu.and_then(|report| report.setup_ms).unwrap_or(-1.0),
            wgpu.and_then(|report| report.encode_submit_ms).unwrap_or(-1.0),
            wgpu.and_then(|report| report.readback_ms).unwrap_or(-1.0),
            self.texture_register_ms.unwrap_or(-1.0),
            image.page_index.unwrap_or_default(),
            image.page_count.unwrap_or_default(),
            image.original_size.map_or(0, |size| size[0]),
            image.original_size.map_or(0, |size| size[1]),
            image.display_size.map_or(0, |size| size[0]),
            image.display_size.map_or(0, |size| size[1]),
            wgpu.map_or(0, |report| report.source_size[0]),
            wgpu.map_or(0, |report| report.source_size[1]),
            wgpu.map_or(0, |report| report.image_size[0]),
            wgpu.map_or(0, |report| report.image_size[1]),
            image.target_long_edge,
            image.decode_backend.unwrap_or("unknown"),
            wgpu.and_then(|report| report.backend).unwrap_or("unknown"),
            wgpu.and_then(|report| report.device_type).unwrap_or("unknown"),
            wgpu.and_then(|report| report.checksum).unwrap_or_default(),
            wgpu.map_or("unknown", |report| report.mode),
            image.error.as_deref().unwrap_or("none"),
            wgpu.and_then(|report| report.error.as_deref()).unwrap_or("none")
        );
    }
}

impl winit::application::ApplicationHandler<UserEvent> for GlowProbeApp {
    fn resumed(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        let (gl_window, gl) = match create_display(event_loop) {
            Ok(value) => value,
            Err(error) => {
                eprintln!("runtime_probe error={error}");
                event_loop.exit();
                return;
            }
        };
        let gl = Arc::new(gl);
        let egui_glow = egui_glow::EguiGlow::new(event_loop, gl.clone(), None, None, true);
        let proxy = egui::mutex::Mutex::new(self.proxy.clone());
        egui_glow
            .egui_ctx
            .set_request_repaint_callback(move |info| {
                let _ = proxy.lock().send_event(UserEvent::Redraw(info.delay));
            });

        gl_window.window().set_visible(true);
        gl_window.window().request_redraw();
        self.gl_window = Some(gl_window);
        self.gl = Some(gl);
        self.egui_glow = Some(egui_glow);
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
        if let winit::event::WindowEvent::Resized(size) = event {
            if let Some(gl_window) = self.gl_window.as_ref() {
                gl_window.resize(size);
            }
            return;
        }
        let Some(gl_window) = self.gl_window.as_ref() else {
            return;
        };
        let Some(egui_glow) = self.egui_glow.as_mut() else {
            return;
        };
        let response = egui_glow.on_window_event(gl_window.window(), &event);
        if response.repaint {
            gl_window.window().request_redraw();
        }
    }

    fn user_event(&mut self, event_loop: &winit::event_loop::ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Redraw(delay) => {
                self.repaint_delay = delay;
                if let Some(gl_window) = self.gl_window.as_ref() {
                    if delay.is_zero() {
                        gl_window.window().request_redraw();
                        event_loop.set_control_flow(winit::event_loop::ControlFlow::Poll);
                    } else if let Some(redraw_at) = Instant::now().checked_add(delay) {
                        event_loop
                            .set_control_flow(winit::event_loop::ControlFlow::WaitUntil(redraw_at));
                    } else {
                        event_loop.set_control_flow(winit::event_loop::ControlFlow::Wait);
                    }
                }
            }
            UserEvent::ImagePrepared(report) => {
                self.image_report = Some(report);
                if let Some(gl_window) = self.gl_window.as_ref() {
                    gl_window.window().request_redraw();
                }
            }
            UserEvent::WgpuProbeFinished(report) => {
                self.wgpu_report = Some(report);
                if let Some(gl_window) = self.gl_window.as_ref() {
                    gl_window.window().request_redraw();
                }
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        let Some(gl_window) = self.gl_window.as_ref() else {
            return;
        };
        let needs_probe_redraw = self.first_visible_ms.is_none()
            || (self.auto_close_after_report
                && (self.wgpu_report.is_some() || self.image_report.is_some()));
        if needs_probe_redraw {
            gl_window.window().request_redraw();
            event_loop.set_control_flow(winit::event_loop::ControlFlow::Poll);
        }
    }

    fn new_events(
        &mut self,
        _event_loop: &winit::event_loop::ActiveEventLoop,
        cause: winit::event::StartCause,
    ) {
        if matches!(cause, winit::event::StartCause::ResumeTimeReached { .. }) {
            if let Some(gl_window) = self.gl_window.as_ref() {
                gl_window.window().request_redraw();
            }
        }
    }

    fn exiting(&mut self, _event_loop: &winit::event_loop::ActiveEventLoop) {
        if let Some(egui_glow) = self.egui_glow.as_mut() {
            egui_glow.destroy();
        }
    }
}

fn draw_probe_ui(
    ctx: &egui::Context,
    started_at: Instant,
    first_visible_ms: Option<f64>,
    image_report: Option<&PreparedImageReport>,
    report: Option<&WgpuProbeReport>,
    base_texture: Option<&egui::TextureHandle>,
    texture: Option<&egui::TextureHandle>,
    base_texture_register_ms: Option<f64>,
    texture_register_ms: Option<f64>,
) -> bool {
    let mut quit = false;
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.fill(egui::Color32::from_rgb(9, 10, 12)))
        .show(ctx, |ui| {
            ui.add_space(14.0);
            ui.heading("SuiSuiView runtime probe");
            ui.label(format!(
                "Glow UI alive at {:.1} ms",
                elapsed_ms(started_at.elapsed())
            ));
            ui.label(format!("first visible: {}", format_ms(first_visible_ms)));
            ui.separator();
            if let Some(image_report) = image_report {
                ui.label(format!(
                    "first-page open/read/prepare: {} / {} / {}",
                    format_ms(image_report.open_source_ms),
                    format_ms(image_report.read_page_ms),
                    format_ms(image_report.prepare_ms)
                ));
                ui.label(format!(
                    "base texture register: {}",
                    format_ms(base_texture_register_ms)
                ));
                ui.label(format!(
                    "page: {} / {}",
                    image_report
                        .page_index
                        .map(|page| page + 1)
                        .unwrap_or_default(),
                    image_report.page_count.unwrap_or_default()
                ));
                if let Some(title) = image_report.title.as_deref() {
                    ui.label(format!("title: {title}"));
                }
                if let Some(page_name) = image_report.page_name.as_deref() {
                    ui.label(format!("page name: {page_name}"));
                }
                if let Some(error) = image_report.error.as_ref() {
                    ui.colored_label(egui::Color32::LIGHT_RED, error);
                }
            }
            match report {
                Some(report) => {
                    ui.label(format!("WGPU init: {}", format_ms(report.init_ms)));
                    ui.label(format!(
                        "WGPU work: {}",
                        format_ms(report.compute_readback_ms)
                    ));
                    if report.shader_module_ms.is_some() || report.pipeline_ms.is_some() {
                        ui.label(format!(
                            "shader/pipeline: {} / {}",
                            format_ms(report.shader_module_ms),
                            format_ms(report.pipeline_ms)
                        ));
                        ui.label(format!(
                            "upload/setup/submit/readback: {} / {} / {} / {}",
                            format_ms(report.upload_ms),
                            format_ms(report.setup_ms),
                            format_ms(report.encode_submit_ms),
                            format_ms(report.readback_ms)
                        ));
                    }
                    ui.label(format!(
                        "Glow texture register: {}",
                        format_ms(texture_register_ms)
                    ));
                    ui.label(format!(
                        "source/output: {}x{} -> {}x{}",
                        report.source_size[0],
                        report.source_size[1],
                        report.image_size[0],
                        report.image_size[1]
                    ));
                    ui.label(format!(
                        "adapter: {} / {}",
                        report.backend.unwrap_or("unknown"),
                        report.device_type.unwrap_or("unknown")
                    ));
                    ui.label(format!("mode: {}", report.mode));
                    if let Some(error) = report.error.as_ref() {
                        ui.colored_label(egui::Color32::LIGHT_RED, error);
                    }
                    if let Some(texture) = texture.or(base_texture) {
                        ui.add(
                            egui::Image::new(texture)
                                .max_width(128.0)
                                .corner_radius(egui::CornerRadius::same(4)),
                        );
                    }
                }
                None => {
                    ui.label("WGPU worker: running...");
                    if let Some(texture) = base_texture {
                        ui.add(
                            egui::Image::new(texture)
                                .max_width(128.0)
                                .corner_radius(egui::CornerRadius::same(4)),
                        );
                    }
                }
            }
            ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                quit = ui.button("Quit").clicked();
            });
        });
    quit
}

fn format_ms(value: Option<f64>) -> String {
    value
        .map(|value| format!("{value:.1} ms"))
        .unwrap_or_else(|| "pending".to_owned())
}

fn main() {
    let started_at = Instant::now();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--headless-worker") {
        if let Err(error) = run_headless_worker(started_at) {
            eprintln!("runtime_probe error={error}");
            std::process::exit(1);
        }
        return;
    }

    let auto_close_after_report = args.iter().any(|arg| arg == "--auto-close-after-report");
    let target_long_edge = target_long_edge_from_args(&args).unwrap_or(DEFAULT_TARGET_LONG_EDGE);
    match try_run_handoff_mode(started_at, &args, auto_close_after_report, target_long_edge) {
        Ok(true) => return,
        Ok(false) => {}
        Err(error) => {
            eprintln!("runtime_probe error={error}");
            std::process::exit(1);
        }
    }

    let input_path = input_path_from_args(&args);
    let image_worker_mode = image_worker_mode_from_args(&args).unwrap_or(ImageWorkerMode::Copy);
    let event_loop = winit::event_loop::EventLoop::<UserEvent>::with_user_event()
        .build()
        .expect("failed to build winit event loop");
    let proxy = event_loop.create_proxy();
    let mut app = GlowProbeApp::new(
        started_at,
        proxy,
        auto_close_after_report,
        input_path,
        target_long_edge,
        image_worker_mode,
    );
    event_loop
        .run_app(&mut app)
        .expect("failed to run runtime probe");
}
