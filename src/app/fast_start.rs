use super::platform;
use super::ui::theme;
use super::SuiSuiViewApp;
use crate::core::i18n::I18n;
use crate::core::source::{classify_path, SourceKind};
use crate::core::state::FastStartFailureNotice;
use crate::core::state::{RendererMode, StateStore};
use egui::{self, RichText};
use serde::Serialize;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app) enum FastStartReportAction {
    CopyReport,
    OpenDiagnostics,
}

pub(crate) fn disable_gpu_after_handoff_failure(
    mut store: StateStore,
    failure: &super::handoff_preview::HandoffFailure,
    startup_open_path: Option<&Path>,
) -> StateStore {
    let timestamp = UtcTimestamp::now();
    let diagnostic_path = write_diagnostic(&store, failure, startup_open_path, &timestamp)
        .map_err(|error| {
            eprintln!("SuiSuiView fast-start diagnostic write failed: {error}");
            error
        })
        .ok();
    let notice = FastStartFailureNotice {
        generated_at: timestamp.display,
        stage: failure.stage.key().to_owned(),
        error: failure.error.clone(),
        gpu_name: failure.metrics.prewarm_adapter_name.clone(),
        backend: failure.metrics.prewarm_backend.clone(),
        device_type: failure.metrics.prewarm_device_type.clone(),
        diagnostic_path: diagnostic_path.map(|path| path.display().to_string()),
        shown: false,
    };

    let mut settings = store.settings().clone();
    settings.renderer_mode = RendererMode::LowMemoryGlow;
    store.update_settings(settings);
    store.record_fast_start_failure(notice);
    store
}

pub(in crate::app) fn show_settings_status(
    ui: &mut egui::Ui,
    notice: Option<&FastStartFailureNotice>,
    action: &mut Option<FastStartReportAction>,
    i18n: I18n,
) {
    ui.add_space(6.0);
    match notice {
        Some(notice) => {
            ui.label(
                RichText::new(i18n.text("settings.rendering.fast_start.failed"))
                    .size(12.0)
                    .color(egui::Color32::from_rgb(245, 181, 84)),
            );
            ui.label(
                RichText::new(stage_user_reason(&notice.stage, i18n))
                    .size(12.0)
                    .color(theme::TEXT_MUTED),
            );
            ui.horizontal_wrapped(|ui| {
                if ui
                    .button(i18n.text("fast_start.copy_report"))
                    .on_hover_text(i18n.text("fast_start.copy_report.help"))
                    .clicked()
                {
                    *action = Some(FastStartReportAction::CopyReport);
                }
                if ui
                    .button(i18n.text("fast_start.open_diagnostics"))
                    .on_hover_text(i18n.text("fast_start.open_diagnostics.help"))
                    .clicked()
                {
                    *action = Some(FastStartReportAction::OpenDiagnostics);
                }
            });
        }
        None => {
            ui.label(
                RichText::new(i18n.text("settings.rendering.fast_start.normal"))
                    .size(12.0)
                    .color(theme::TEXT_MUTED),
            );
        }
    }
}

impl SuiSuiViewApp {
    pub(in crate::app) fn show_fast_start_failure_dialog(&mut self, ctx: &egui::Context) {
        let Some(notice) = self
            .fast_start_failure_notice
            .as_ref()
            .filter(|notice| !notice.shown)
            .cloned()
        else {
            return;
        };

        let viewport_rect = ctx.screen_rect();
        let dialog_size = egui::vec2(460.0, 254.0);
        let i18n = self.i18n();
        let mut action = None;
        let mut close_clicked = false;

        egui::Area::new(egui::Id::new("fast_start_failure_dialog"))
            .fixed_pos(viewport_rect.min)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                let (overlay_rect, _) =
                    ui.allocate_exact_size(viewport_rect.size(), egui::Sense::click());
                ui.painter().rect_filled(
                    overlay_rect,
                    egui::CornerRadius::ZERO,
                    egui::Color32::from_rgba_unmultiplied(0, 0, 0, 128),
                );

                let dialog_rect = egui::Rect::from_center_size(overlay_rect.center(), dialog_size);
                ui.scope_builder(egui::UiBuilder::new().max_rect(dialog_rect), |ui| {
                    egui::Frame::new()
                        .fill(egui::Color32::from_rgb(23, 25, 29))
                        .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(76, 82, 92)))
                        .corner_radius(egui::CornerRadius::same(8))
                        .inner_margin(egui::Margin::symmetric(16, 14))
                        .show(ui, |ui| {
                            ui.set_min_size(dialog_size - egui::vec2(32.0, 28.0));
                            ui.label(
                                RichText::new(i18n.text("fast_start.failure.title"))
                                    .size(18.0)
                                    .strong()
                                    .color(theme::TEXT_PRIMARY),
                            );
                            ui.add_space(8.0);
                            ui.label(
                                RichText::new(i18n.text("fast_start.failure.continue_glow"))
                                    .size(14.0)
                                    .color(theme::TEXT_PRIMARY),
                            );
                            ui.label(
                                RichText::new(i18n.text("fast_start.failure.gpu_disabled"))
                                    .size(12.5)
                                    .color(theme::TEXT_MUTED),
                            );
                            ui.add_space(8.0);
                            ui.label(
                                RichText::new(i18n.with_vars(
                                    "fast_start.failure.reason",
                                    &[("reason", stage_user_reason(&notice.stage, i18n))],
                                ))
                                .size(12.5)
                                .color(egui::Color32::from_rgb(245, 181, 84)),
                            );
                            ui.label(
                                RichText::new(i18n.text("fast_start.failure.report_hint"))
                                    .size(12.0)
                                    .color(theme::TEXT_MUTED),
                            );
                            ui.add_space(14.0);
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui
                                        .add_sized(
                                            [72.0, 34.0],
                                            egui::Button::new(i18n.text("common.ok")),
                                        )
                                        .clicked()
                                    {
                                        close_clicked = true;
                                    }
                                    if ui
                                        .add_sized(
                                            [132.0, 34.0],
                                            egui::Button::new(
                                                i18n.text("fast_start.open_diagnostics"),
                                            ),
                                        )
                                        .clicked()
                                    {
                                        action = Some(FastStartReportAction::OpenDiagnostics);
                                    }
                                    if ui
                                        .add_sized(
                                            [120.0, 34.0],
                                            egui::Button::new(i18n.text("fast_start.copy_report")),
                                        )
                                        .clicked()
                                    {
                                        action = Some(FastStartReportAction::CopyReport);
                                    }
                                },
                            );
                        });
                });
            });

        if let Some(action) = action {
            self.handle_fast_start_report_action(&notice, action);
        }
        if close_clicked {
            self.dismiss_fast_start_failure_notice();
        }
    }

    pub(in crate::app) fn handle_fast_start_report_action(
        &mut self,
        notice: &FastStartFailureNotice,
        action: FastStartReportAction,
    ) {
        let i18n = self.i18n();
        match action {
            FastStartReportAction::CopyReport => {
                match platform::copy_text_to_clipboard(&report_text(notice, i18n)) {
                    Ok(()) => self.notify(i18n.text("status.fast_start_report_copied")),
                    Err(error) => self.notify(
                        i18n.with_vars("status.fast_start_report_copy_failed", &[("error", error)]),
                    ),
                }
            }
            FastStartReportAction::OpenDiagnostics => {
                if let Err(error) = open_diagnostics_path(notice) {
                    self.notify(i18n.with_vars(
                        "status.fast_start_diagnostics_open_failed",
                        &[("error", error)],
                    ));
                }
            }
        }
    }

    fn dismiss_fast_start_failure_notice(&mut self) {
        if let Some(notice) = self.fast_start_failure_notice.as_mut() {
            notice.shown = true;
        }
        self.store.mark_fast_start_failure_notice_shown();
    }
}

pub(in crate::app) fn report_text(notice: &FastStartFailureNotice, i18n: I18n) -> String {
    let diagnostic_path = notice
        .diagnostic_path
        .as_deref()
        .unwrap_or("diagnostic file was not created");
    format!(
        "SuiSuiView WGPU fast-start failure report\n\
         App version: {}\n\
         OS: {} {}\n\
         GPU: {}\n\
         Backend: {}\n\
         Device type: {}\n\
         User reason: {}\n\
         Failure stage: {}\n\
         Error: {}\n\
         Diagnostic file: {}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        notice.gpu_name.as_deref().unwrap_or("unknown"),
        notice.backend.as_deref().unwrap_or("unknown"),
        notice.device_type.as_deref().unwrap_or("unknown"),
        stage_user_reason(&notice.stage, i18n),
        notice.stage,
        notice.error,
        diagnostic_path
    )
}

pub(in crate::app) fn stage_user_reason(stage: &str, i18n: I18n) -> String {
    i18n.text(match stage {
        "gl_create" | "gl_swap" => "fast_start.reason.gl_create",
        "wgpu_prewarm" => "fast_start.reason.wgpu_prewarm",
        "gl_destroy" => "fast_start.reason.gl_destroy",
        "wgpu_surface_attach" => "fast_start.reason.wgpu_surface_attach",
        "wgpu_render_state" | "first_wgpu_frame" => "fast_start.reason.first_wgpu_frame",
        _ => "fast_start.reason.unknown",
    })
}

fn open_diagnostics_path(notice: &FastStartFailureNotice) -> Result<(), String> {
    let Some(path) = notice.diagnostic_path.as_deref() else {
        return Err("diagnostic path is missing".to_owned());
    };
    platform::reveal_in_file_manager(&PathBuf::from(path))
}

fn write_diagnostic(
    store: &StateStore,
    failure: &super::handoff_preview::HandoffFailure,
    startup_open_path: Option<&Path>,
    timestamp: &UtcTimestamp,
) -> Result<PathBuf, String> {
    let diagnostics_dir = diagnostics_dir(store);
    fs::create_dir_all(&diagnostics_dir).map_err(|error| error.to_string())?;
    let path = diagnostics_dir.join(format!("fast-start-{}.json", timestamp.filename));
    let diagnostic = FastStartDiagnostic {
        diagnostic_version: 1,
        generated_at_utc: timestamp.display.as_str(),
        app_version: env!("CARGO_PKG_VERSION"),
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        stage: failure.stage.key(),
        user_reason_key: user_reason_key_for_stage(failure.stage.key()),
        error: failure.error.as_str(),
        gpu: GpuDiagnostic {
            name: failure.metrics.prewarm_adapter_name.as_deref(),
            backend: failure.metrics.prewarm_backend.as_deref(),
            device_type: failure.metrics.prewarm_device_type.as_deref(),
        },
        metrics: &failure.metrics,
        fallback: "gpu_acceleration_disabled_glow_fallback",
        startup_source: startup_open_path.map(source_diagnostic),
    };
    let text = serde_json::to_string_pretty(&diagnostic).map_err(|error| error.to_string())?;
    fs::write(&path, text).map_err(|error| error.to_string())?;
    Ok(path)
}

#[derive(Serialize)]
struct FastStartDiagnostic<'a> {
    diagnostic_version: u32,
    generated_at_utc: &'a str,
    app_version: &'a str,
    os: &'a str,
    arch: &'a str,
    stage: &'a str,
    user_reason_key: &'a str,
    error: &'a str,
    gpu: GpuDiagnostic<'a>,
    metrics: &'a super::handoff_preview::HandoffPreviewMetrics,
    fallback: &'a str,
    startup_source: Option<StartupSourceDiagnostic>,
}

#[derive(Serialize)]
struct GpuDiagnostic<'a> {
    name: Option<&'a str>,
    backend: Option<&'a str>,
    device_type: Option<&'a str>,
}

#[derive(Serialize)]
struct StartupSourceDiagnostic {
    source_kind: &'static str,
    file_name: Option<String>,
    extension: Option<String>,
    size_bytes: Option<u64>,
}

fn source_diagnostic(path: &Path) -> StartupSourceDiagnostic {
    StartupSourceDiagnostic {
        source_kind: source_kind_label(classify_path(path)),
        file_name: path
            .file_name()
            .and_then(|name| name.to_str())
            .map(ToOwned::to_owned),
        extension: path
            .extension()
            .and_then(|extension| extension.to_str())
            .map(ToOwned::to_owned),
        size_bytes: fs::metadata(path).ok().map(|metadata| metadata.len()),
    }
}

fn source_kind_label(kind: SourceKind) -> &'static str {
    match kind {
        SourceKind::Folder => "folder",
        SourceKind::ZipCbz => "zip_cbz",
        SourceKind::SingleImage => "single_image",
        SourceKind::UnsupportedRar => "unsupported_rar",
        SourceKind::Unsupported => "unsupported",
    }
}

fn user_reason_key_for_stage(stage: &str) -> &'static str {
    match stage {
        "gl_create" | "gl_swap" => "fast_start.reason.gl_create",
        "wgpu_prewarm" => "fast_start.reason.wgpu_prewarm",
        "gl_destroy" => "fast_start.reason.gl_destroy",
        "wgpu_surface_attach" => "fast_start.reason.wgpu_surface_attach",
        "wgpu_render_state" | "first_wgpu_frame" => "fast_start.reason.first_wgpu_frame",
        _ => "fast_start.reason.unknown",
    }
}

fn diagnostics_dir(store: &StateStore) -> PathBuf {
    let state_parent = store.path().parent().unwrap_or_else(|| Path::new("."));
    if state_parent.file_name().and_then(|name| name.to_str()) == Some("data") {
        return state_parent
            .parent()
            .unwrap_or(state_parent)
            .join("diagnostics");
    }
    state_parent.join("diagnostics")
}

struct UtcTimestamp {
    display: String,
    filename: String,
}

impl UtcTimestamp {
    fn now() -> Self {
        let seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs() as i64)
            .unwrap_or_default();
        let (year, month, day, hour, minute, second) = utc_parts(seconds);
        Self {
            display: format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z"),
            filename: format!("{year:04}{month:02}{day:02}-{hour:02}{minute:02}{second:02}"),
        }
    }
}

fn utc_parts(seconds: i64) -> (i32, u32, u32, u32, u32, u32) {
    let days = seconds.div_euclid(86_400);
    let seconds_of_day = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = (seconds_of_day / 3_600) as u32;
    let minute = ((seconds_of_day % 3_600) / 60) as u32;
    let second = (seconds_of_day % 60) as u32;
    (year, month, day, hour, minute, second)
}

fn civil_from_days(days_since_unix_epoch: i64) -> (i32, u32, u32) {
    let z = days_since_unix_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    let year = year + i64::from(month <= 2);
    (year as i32, month as u32, day as u32)
}
