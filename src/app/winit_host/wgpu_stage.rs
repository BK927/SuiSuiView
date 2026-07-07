use super::glow_window::create_plain_window;
use super::title_sync::{schedule_process_visible_window_title, sync_visible_window_title};
use super::SuiSuiViewApp;
use super::{elapsed_ms, injected_failure, HostFailureStage, Stage, WinitHostApp};
use crate::app::runtime::{AppRuntime, StartupReveal};
use egui::ViewportId;
use egui_wgpu::winit::Painter;
use egui_wgpu::{wgpu, WgpuConfiguration, WgpuSetupExisting};
use egui_winit::winit;
use std::num::NonZeroU32;
use std::time::Instant;

impl WinitHostApp {
    /// WGPU-direct startup: skip the Glow stage and the handoff entirely —
    /// create a plain window and attach WGPU straight away. The WGPU device is
    /// prewarmed concurrently with window creation; the window stays hidden
    /// until the first WGPU frame reveals it (see `redraw_wgpu`).
    pub(super) fn start_wgpu_direct(
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

        self.dpi_size_guard.seed_initial(&window);

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

    pub(super) fn redraw_wgpu(
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
        if let Some(error) = injected_failure(HostFailureStage::FirstWgpuFrame) {
            self.fail(event_loop, HostFailureStage::FirstWgpuFrame, error);
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

    pub(super) fn resize_wgpu_surface(painter: &mut Painter, size: winit::dpi::PhysicalSize<u32>) {
        let Some(width) = NonZeroU32::new(size.width) else {
            return;
        };
        let Some(height) = NonZeroU32::new(size.height) else {
            return;
        };
        painter.on_window_resized(ViewportId::ROOT, width, height);
    }

    pub(super) fn wgpu_window_event(
        event: winit::event::WindowEvent,
        window: &winit::window::Window,
        painter: &mut Painter,
        egui_state: &mut egui_winit::State,
    ) {
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
