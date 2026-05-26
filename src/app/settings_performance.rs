use super::cache_budget_summary;
use super::settings::{checkbox_with_help, grid_label_with_help, info_icon, setting_group};
use super::ui::theme;
use crate::core::state::{AppSettings, CacheMemoryMode, DecodeMode, RendererMode};
use eframe::egui::{self, RichText};

pub(in crate::app) fn show_performance_settings(
    ui: &mut egui::Ui,
    draft: &mut AppSettings,
    target_long_edge: u32,
    visible_pages: usize,
    changed: &mut bool,
) {
    setting_group(
        ui,
        "이미지 읽기",
        "페이지 파일을 읽고 표시용 이미지로 준비하는 방식입니다.",
        |ui| {
            egui::Grid::new("settings_performance_grid")
                .num_columns(2)
                .spacing([14.0, 8.0])
                .show(ui, |ui| {
                    grid_label_with_help(
                        ui,
                        "디코딩 모드",
                        "이미지를 읽는 내부 방식입니다. 자동 모드는 가능한 경우 빠른 경로를 사용합니다.",
                    );
                    egui::ComboBox::from_id_salt("decode_mode")
                        .selected_text(draft.decode_mode.label())
                        .show_ui(ui, |ui| {
                            for mode in DecodeMode::ALL {
                                *changed |= ui
                                    .selectable_value(&mut draft.decode_mode, mode, mode.label())
                                    .changed();
                            }
                        });
                    ui.end_row();
                });
        },
    );

    ui.add_space(8.0);
    setting_group(
        ui,
        "렌더러",
        "앱 창을 그리는 백엔드입니다. 변경 사항은 앱을 다시 시작하면 적용됩니다.",
        |ui| {
            egui::Grid::new("settings_renderer_grid")
                .num_columns(2)
                .spacing([14.0, 8.0])
                .show(ui, |ui| {
                    grid_label_with_help(
                        ui,
                        "표시 백엔드",
                        "기본값은 메모리와 페이지 넘김 응답성을 우선하는 OpenGL입니다. WGPU는 표시 업스케일러와 GPU shader 효과가 필요할 때 선택합니다.",
                    );
                    egui::ComboBox::from_id_salt("renderer_mode")
                        .selected_text(draft.renderer_mode.label())
                        .show_ui(ui, |ui| {
                            for mode in RendererMode::ALL {
                                *changed |= ui
                                    .selectable_value(&mut draft.renderer_mode, mode, mode.label())
                                    .changed();
                            }
                        });
                    ui.end_row();
                });
            ui.add_space(4.0);
            ui.label(
                RichText::new("WGPU로 바꾸면 다음 실행부터 GPU 표시 업스케일러를 사용할 수 있지만, 기본 메모리 사용량이 크게 늘 수 있습니다.")
                    .size(12.0)
                    .color(theme::TEXT_MUTED),
            );
        },
    );

    ui.add_space(8.0);
    setting_group(
        ui,
        "미리 불러오기",
        "페이지 넘김을 부드럽게 하기 위해 주변 페이지를 미리 준비합니다.",
        |ui| {
            *changed |= checkbox_with_help(
                ui,
                &mut draft.prefetch_enabled,
                "다음 페이지 미리 캐시",
                "현재 페이지 주변의 다음 페이지를 미리 읽어 페이지 넘김 대기 시간을 줄입니다.",
            );
            *changed |= checkbox_with_help(
                ui,
                &mut draft.progressive_preview_enabled,
                "저해상도 먼저 표시",
                "큰 이미지를 먼저 낮은 해상도로 보여준 뒤 선명한 이미지로 교체합니다.",
            );
        },
    );

    ui.add_space(8.0);
    setting_group(
        ui,
        "메모리",
        "준비된 페이지 이미지를 얼마나 오래 보관할지 정합니다.",
        |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label("페이지 캐시 메모리");
                info_icon(
                    ui,
                    "캐시가 클수록 다시 보는 페이지가 빨리 뜨지만 메모리를 더 사용합니다.",
                );
                *changed |= ui
                    .radio_value(&mut draft.cache_memory_mode, CacheMemoryMode::Auto, "자동")
                    .changed();
                *changed |= ui
                    .radio_value(
                        &mut draft.cache_memory_mode,
                        CacheMemoryMode::Manual,
                        "수동",
                    )
                    .changed();
                ui.add_enabled_ui(draft.cache_memory_mode == CacheMemoryMode::Manual, |ui| {
                    *changed |= ui
                        .add(
                            egui::DragValue::new(&mut draft.manual_cache_mb)
                                .range(64..=2048)
                                .speed(16)
                                .suffix(" MB"),
                        )
                        .changed();
                });
            });

            let summary = cache_budget_summary(draft, target_long_edge, visible_pages);
            ui.add_space(4.0);
            ui.label(
                RichText::new(format!(
                    "CPU 페이지 캐시 {:.0} MB · worker 미리읽기 {:.0} MB · AI 결과 {:.0} MB",
                    mib(summary.cpu_prepared_bytes),
                    mib(summary.worker_prefetch_bytes),
                    mib(summary.upscaled_bytes)
                ))
                .size(12.0)
                .color(theme::TEXT_MUTED),
            );
            ui.label(
                RichText::new(format!(
                    "GPU 표시 캐시 source {:.0} MB · intermediate {:.0} MB · 현재 target 기준 페이지당 최대 약 {:.0} MB, CPU 약 {}장 / worker 약 {}장",
                    mib(summary.gpu_source_texture_bytes),
                    mib(summary.gpu_intermediate_texture_bytes),
                    mib(summary.estimated_page_bytes),
                    summary.estimated_cpu_pages,
                    summary.estimated_worker_pages
                ))
                .size(12.0)
                .color(theme::TEXT_MUTED),
            );
        },
    );
}

fn mib(bytes: usize) -> f32 {
    bytes as f32 / (1024.0 * 1024.0)
}
