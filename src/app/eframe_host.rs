use super::runtime::{AppRuntime, ScreenRenderer};
use super::{perf, StartupOpen, SuiSuiViewApp};
use crate::core::state::StateStore;
use crossbeam_channel::Receiver;
use eframe::egui;
use std::path::PathBuf;

impl SuiSuiViewApp {
    pub(crate) fn from_eframe(
        cc: &eframe::CreationContext<'_>,
        store: StateStore,
        ipc_rx: Option<Receiver<Option<PathBuf>>>,
        startup_open_path: Option<PathBuf>,
        startup_open: Option<StartupOpen>,
    ) -> Self {
        let screen_renderer =
            cc.wgpu_render_state
                .as_ref()
                .map_or(ScreenRenderer::Glow, |render_state| {
                    perf::record_wgpu_render_state(render_state);
                    ScreenRenderer::Wgpu {
                        target_format: render_state.target_format,
                    }
                });
        Self::new(
            AppRuntime::new(cc.egui_ctx.clone(), screen_renderer),
            store,
            ipc_rx,
            startup_open_path,
            startup_open,
        )
    }
}

impl eframe::App for SuiSuiViewApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.update_frame(ctx);
    }
}
