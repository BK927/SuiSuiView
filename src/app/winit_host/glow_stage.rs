use super::glow_window::{create_gl_display, startup_maximized, GlutinWindowContext};
use super::title_sync::{schedule_process_visible_window_title, sync_visible_window_title};
use super::SuiSuiViewApp;
use super::{elapsed_ms, injected_failure, HostFailureStage, Stage, WinitHostApp};
use crate::app::runtime::{AppRuntime, StartupReveal};
use egui_winit::winit;
use std::sync::Arc;

impl WinitHostApp {
    pub(super) fn start_glow(
        &mut self,
        event_loop: &winit::event_loop::ActiveEventLoop,
    ) -> Result<(), String> {
        if let Some(error) = injected_failure(HostFailureStage::GlCreate) {
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
        self.startup_maximized = startup_maximized(&options.store);
        self.dpi_size_guard.seed_initial(gl_window.window());
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

    pub(super) fn redraw_glow(
        &mut self,
        event_loop: &winit::event_loop::ActiveEventLoop,
        mut app: SuiSuiViewApp,
        gl_window: GlutinWindowContext,
        gl: Arc<glow::Context>,
        mut egui_glow: egui_glow::EguiGlow,
    ) {
        self.poll_prewarm();
        if let Some(error) = self.metrics.prewarm_error.clone() {
            self.fail(event_loop, HostFailureStage::WgpuPrewarm, error);
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
        if let Some(error) = injected_failure(HostFailureStage::GlSwap) {
            self.fail(event_loop, HostFailureStage::GlSwap, error);
            return;
        }
        if let Err(error) = gl_window.swap_buffers() {
            self.fail(event_loop, HostFailureStage::GlSwap, error);
            return;
        }

        let now_ms = elapsed_ms(self.started_at.elapsed());
        self.metrics.last_glow_present_ms = Some(now_ms);
        if self.metrics.first_glow_present_ms.is_none() {
            self.metrics.first_glow_present_ms = Some(now_ms);
            gl_window.reveal_after_first_frame(self.startup_maximized);
        }
        // egui_glow applies Title viewport commands itself, so detect changes from
        // the winit side. A change re-schedules the timed re-assert with the new
        // title (killing the pending one, which would clobber it back — the book
        // title used to vanish ~900ms after a fresh open until the next input).
        let title = gl_window.window().title();
        let title = if title.is_empty() {
            "SuiSuiView".to_owned()
        } else {
            title
        };
        if self.glow_synced_title.as_ref() != Some(&title) {
            self.glow_synced_title = Some(title.clone());
            schedule_process_visible_window_title(title);
        } else {
            sync_visible_window_title(gl_window.window());
        }

        // The Glow host is reactive: the next redraw is scheduled by egui's
        // repaint deadline (see `about_to_wait`) or by input in `window_event`.
        self.stage = Some(Stage::Glow {
            app,
            gl_window,
            gl,
            egui_glow,
        });
    }

    pub(super) fn glow_window_event(
        event: winit::event::WindowEvent,
        gl_window: &GlutinWindowContext,
        egui_glow: &mut egui_glow::EguiGlow,
    ) {
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
}
