#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
use super::perf;
use super::SuiSuiViewApp;
use eframe::egui;
use std::time::Duration;
#[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
use std::time::Instant;

impl SuiSuiViewApp {
    pub(in crate::app) fn update_frame(&mut self, ctx: &egui::Context) {
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        let update_started = Instant::now();
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        let mut phase_started = update_started;
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        macro_rules! record_update_phase {
            ($phase:literal) => {{
                perf::record_app_update_phase(phase_started, $phase);
                phase_started = Instant::now();
            }};
        }

        self.drain_ipc_open_requests(ctx);
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        record_update_phase!("drain_ipc_open_requests");
        self.drain_loader_events();
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        record_update_phase!("drain_loader_events");
        self.drain_worker_events();
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        record_update_phase!("drain_worker_events");
        self.drain_auto_kind_events();
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        record_update_phase!("drain_auto_kind_events");
        self.drain_debug_compare_events();
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        record_update_phase!("drain_debug_compare_events");
        self.run_pending_adjacent_seed_prefetch();
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        record_update_phase!("run_pending_adjacent_seed_prefetch");
        self.drain_adjacent_seed_events();
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        record_update_phase!("drain_adjacent_seed_events");
        self.drain_pending_original_inspection_cache_cleanup(ctx);
        if let Some(thumbnails) = self.bookmark_thumbnails.as_mut() {
            thumbnails.drain(ctx);
        }
        if self.loader_pending {
            ctx.request_repaint_after(Duration::from_millis(25));
        }
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        record_update_phase!("background_maintenance");
        self.handle_dropped_files(ctx);
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        record_update_phase!("handle_dropped_files");
        if !self.settings_is_capturing_keyboard() {
            self.handle_keyboard(ctx);
        }
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        record_update_phase!("handle_keyboard");
        self.drive_queued_sibling_book_turn(ctx);
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        record_update_phase!("drive_queued_sibling_book_before_viewer");
        self.maintain_native_window_state(ctx);
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        record_update_phase!("maintain_native_window_state");
        self.update_window_title(ctx);
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        record_update_phase!("update_window_title");
        self.flush_deferred_state_save_if_due();
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        record_update_phase!("flush_deferred_state_save");
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        self.drive_auto_page_turn_diagnostics(ctx);
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        record_update_phase!("drive_auto_page_turn_diagnostics");

        self.show_top_bar(ctx);
        self.show_status_surfaces(ctx);
        self.show_settings_window(ctx);
        self.show_about_window(ctx);
        self.show_fast_start_failure_dialog(ctx);
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        record_update_phase!("status_surfaces");

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ctx, |ui| self.show_viewer(ui, ctx));
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        record_update_phase!("show_viewer");

        self.show_bookmark_popover(ctx);
        self.show_edge_prompt(ctx);
        self.show_delete_confirmation_dialog(ctx);
        self.drive_queued_page_turn_after_paint(ctx);
        self.drive_queued_sibling_book_turn(ctx);
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        record_update_phase!("post_viewer");
        self.prewarm_neighbor_textures(ctx);
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        perf::record_app_update_phase(phase_started, "prewarm_neighbor_textures");

        if self.transition.is_some() {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
        #[cfg(any(feature = "perf-dev", feature = "perf-diagnostics"))]
        perf::record_ui_update(
            update_started,
            self.source.is_some(),
            self.transition.is_some(),
        );
    }
}
