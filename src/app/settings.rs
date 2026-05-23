use super::ui::{dialog, icons, theme};
use super::{apply_window_level, cache_budget_bytes, SuiSuiViewApp};
use crate::core::state::{
    AiUpscaleBackend, AiUpscalePrefetchMode, AppSettings, CacheMemoryMode, DecodeMode,
    DisplayUpscaler, EdgePageAction, GpuEffectMode, LargeImageAnchor, ResizeFilter, WheelMode,
};
use eframe::egui::{self, RichText};
use rfd::FileDialog;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum SettingsSection {
    #[default]
    General,
    ImageProcessing,
    Performance,
    Mouse,
}

impl SettingsSection {
    const ALL: [Self; 4] = [
        Self::General,
        Self::ImageProcessing,
        Self::Performance,
        Self::Mouse,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::General => "일반",
            Self::ImageProcessing => "영상 처리",
            Self::Performance => "성능",
            Self::Mouse => "마우스",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::General => "삭제 확인, 창 표시, 상태바 같은 기본 동작",
            Self::ImageProcessing => "보간, 실시간 업스케일, 색상 보정",
            Self::Performance => "디코딩, 미리보기, 프리로드, 캐시 정책",
            Self::Mouse => "더블클릭, 가운데 버튼, 휠 조작",
        }
    }

    fn icon(self) -> (char, icons::IconStyle) {
        match self {
            Self::General => (icons::SETTINGS, icons::IconStyle::Regular),
            Self::ImageProcessing => (icons::WAND, icons::IconStyle::Regular),
            Self::Performance => (icons::DOCUMENT, icons::IconStyle::Regular),
            Self::Mouse => (icons::PIN, icons::IconStyle::Regular),
        }
    }
}

impl SuiSuiViewApp {
    pub(super) fn show_settings_window(&mut self, ctx: &egui::Context) {
        if !self.settings_open {
            return;
        }

        let mut open = self.settings_open;
        let mut draft = self.settings.clone();
        let mut changed = false;
        if draft.gpu_effect_mode != GpuEffectMode::Auto {
            draft.gpu_effect_mode = GpuEffectMode::Auto;
            changed = true;
        }
        let mut active_section = self.settings_section;
        let dialog_size = dialog::bounded_dialog_size(
            ctx,
            dialog::SPLIT_DIALOG_SIZE,
            dialog::MIN_SPLIT_DIALOG_SIZE,
        );

        egui::Window::new("환경설정")
            .open(&mut open)
            .fixed_size(dialog_size)
            .collapsible(false)
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing = egui::vec2(10.0, 8.0);
                let body_size = ui.available_size();
                let spacing_x = ui.spacing().item_spacing.x;
                let nav_size = egui::vec2(dialog::NAV_WIDTH, body_size.y);
                let content_size = egui::vec2(
                    (body_size.x - dialog::NAV_WIDTH - spacing_x).max(0.0),
                    body_size.y,
                );

                ui.horizontal(|ui| {
                    dialog::show_sized_frame(ui, nav_size, dialog::rail_frame(), |ui| {
                        ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                            ui.label(
                                RichText::new("설정")
                                    .strong()
                                    .size(15.0)
                                    .color(theme::TEXT_PRIMARY),
                            );
                            ui.add_space(8.0);
                            for section in SettingsSection::ALL {
                                let (icon, icon_style) = section.icon();
                                if dialog::nav_button(
                                    ui,
                                    active_section == section,
                                    icon,
                                    icon_style,
                                    section.label(),
                                )
                                .clicked()
                                {
                                    active_section = section;
                                }
                                ui.add_space(4.0);
                            }
                        });
                    });

                    dialog::show_sized_frame(ui, content_size, dialog::content_frame(), |ui| {
                        ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                            dialog::section_heading(
                                ui,
                                active_section.label(),
                                active_section.description(),
                            );

                            let content_height = ui.available_height();
                            egui::ScrollArea::vertical()
                                .id_salt(("settings_section", active_section.label()))
                                .max_height(content_height)
                                .auto_shrink([false, false])
                                .show(ui, |ui| match active_section {
                                    SettingsSection::General => {
                                        show_general_settings(ui, &mut draft, &mut changed);
                                    }
                                    SettingsSection::ImageProcessing => {
                                        show_image_processing_settings(
                                            ui,
                                            &mut draft,
                                            &mut changed,
                                        );
                                    }
                                    SettingsSection::Performance => {
                                        show_performance_settings(ui, &mut draft, &mut changed);
                                    }
                                    SettingsSection::Mouse => {
                                        show_mouse_settings(ui, &mut draft, &mut changed);
                                    }
                                });
                        });
                    });
                });
            });

        self.settings_section = active_section;
        self.settings_open = open;
        if changed {
            draft.manual_cache_mb = draft.manual_cache_mb.clamp(64, 2048);
            draft.ai_upscale.ncnn.scale = draft.ai_upscale.ncnn.scale.clamp(2, 4);
            if draft.ai_upscale.ncnn.tile_size != 0 {
                draft.ai_upscale.ncnn.tile_size = draft.ai_upscale.ncnn.tile_size.clamp(32, 2048);
            }
            self.apply_settings(ctx, draft);
        }
    }

    pub(super) fn apply_settings(&mut self, ctx: &egui::Context, settings: AppSettings) {
        let previous_decode = self.decode_options();
        let previous_preview = self.settings.progressive_preview_enabled;
        let previous_prefetch = self.settings.prefetch_enabled;
        let previous_cache_budget = self.cpu_cache_budget_bytes();
        let previous_ai = self.settings.ai_upscale.clone();
        let previous_gpu_effect_mode = self.settings.gpu_effect_mode;
        let previous_display_upscaler = self.settings.display_upscaler;

        self.settings = settings;
        self.transition_effect = self.settings.transition_effect;
        self.store.update_settings(self.settings.clone());
        self.pending_state_save_at = None;
        apply_window_level(ctx, self.settings.always_on_top);

        let decode_changed = previous_decode != self.decode_options();
        let preview_changed = previous_preview != self.settings.progressive_preview_enabled;
        if decode_changed || preview_changed {
            self.decoded_pages.clear();
            self.decoded_bytes = 0;
            self.upscaled_pages.clear();
            self.upscaled_bytes = 0;
            self.textures.clear();
            self.page_errors.clear();
            self.ai_upscale_failures.clear();
        } else if previous_cache_budget != self.cpu_cache_budget_bytes() {
            self.prune_decoded_cache();
            self.prune_upscaled_cache();
        }

        let ai_output_changed = previous_ai.backend != self.settings.ai_upscale.backend
            || previous_ai.ncnn != self.settings.ai_upscale.ncnn;
        let ai_prefetch_changed =
            previous_ai.prefetch_mode != self.settings.ai_upscale.prefetch_mode;
        if ai_output_changed {
            self.upscaled_pages.clear();
            self.upscaled_bytes = 0;
            self.textures.clear();
            self.upscale_generation = self.upscale_generation.wrapping_add(1);
            self.upscale_inflight = None;
            self.ai_upscale_queue.clear();
            self.ai_upscale_failures.clear();
        } else if ai_prefetch_changed {
            self.ai_upscale_queue.clear();
        }
        if previous_gpu_effect_mode != self.settings.gpu_effect_mode
            || previous_display_upscaler != self.settings.display_upscaler
        {
            self.textures.clear();
            ctx.request_repaint();
        }

        if self.source.is_some()
            && (decode_changed
                || preview_changed
                || previous_prefetch != self.settings.prefetch_enabled
                || previous_cache_budget != self.cpu_cache_budget_bytes())
        {
            self.worker.set_page(
                self.current_page,
                self.last_nav_direction,
                self.target_long_edge,
                self.visible_page_count(),
                self.worker_options(),
            );
        }
        if ai_output_changed || ai_prefetch_changed {
            self.refresh_ai_prefetch_queue();
        }
        if !(ai_output_changed || ai_prefetch_changed) || self.upscale_inflight.is_none() {
            self.set_status("Settings saved.");
        }
    }
}

fn show_general_settings(ui: &mut egui::Ui, draft: &mut AppSettings, changed: &mut bool) {
    setting_group(
        ui,
        "기본 동작",
        "앱을 닫거나 파일을 지울 때의 기본 행동입니다.",
        |ui| {
            *changed |= checkbox_with_help(
                ui,
                &mut draft.confirm_delete,
                "파일 삭제 전 확인",
                "삭제 단축키를 눌렀을 때 바로 지우지 않고 확인 창을 먼저 보여줍니다.",
            );
            *changed |= checkbox_with_help(
                ui,
                &mut draft.esc_to_quit,
                "ESC로 프로그램 종료",
                "ESC 키를 눌렀을 때 앱을 종료합니다. 끄면 ESC는 종료 동작에 쓰이지 않습니다.",
            );
            *changed |= checkbox_with_help(
                ui,
                &mut draft.always_on_top,
                "항상 위에 표시 (Ctrl+A)",
                "다른 창을 선택해도 SuiSuiView 창이 앞쪽에 남아 있게 합니다.",
            );
        },
    );

    ui.add_space(8.0);
    setting_group(
        ui,
        "표시와 페이지 끝",
        "창의 보조 표시와 책 끝에서의 동작입니다.",
        |ui| {
            *changed |= checkbox_with_help(
                ui,
                &mut draft.show_status_bar,
                "하단 상태바 표시",
                "창 아래쪽에 현재 상태와 짧은 안내 문구를 표시합니다.",
            );
            ui.add_space(6.0);
            egui::Grid::new("settings_general_grid")
                .num_columns(2)
                .spacing([14.0, 8.0])
                .show(ui, |ui| {
                    grid_label_with_help(
                    ui,
                    "마지막/처음 페이지",
                    "책의 끝이나 처음에서 다음/이전 페이지를 누를 때 어떤 동작을 할지 정합니다.",
                );
                    egui::ComboBox::from_id_salt("edge_page_action")
                        .selected_text(draft.edge_page_action.label())
                        .show_ui(ui, |ui| {
                            for action in EdgePageAction::ALL {
                                *changed |= ui
                                    .selectable_value(
                                        &mut draft.edge_page_action,
                                        action,
                                        action.label(),
                                    )
                                    .changed();
                            }
                        });
                    ui.end_row();
                });
        },
    );
}

fn show_image_processing_settings(ui: &mut egui::Ui, draft: &mut AppSettings, changed: &mut bool) {
    setting_group(
        ui,
        "화면 표시",
        "페이지 전환, 보간, 실시간 업스케일 설정입니다.",
        |ui| {
            *changed |= checkbox_with_help(
                ui,
                &mut draft.transition_effect,
                "페이지 전환 효과",
                "페이지를 넘길 때 짧은 슬라이드/페이드 효과를 사용합니다.",
            );
            ui.add_space(6.0);
            egui::Grid::new("settings_image_processing_grid")
                .num_columns(2)
                .spacing([14.0, 8.0])
                .show(ui, |ui| {
                    grid_label_with_help(
                        ui,
                        "CPU 준비 보간",
                        "표시용 이미지를 미리 만들 때 사용하는 CPU 리사이즈 품질입니다.",
                    );
                    egui::ComboBox::from_id_salt("resize_filter")
                        .selected_text(draft.resize_filter.label())
                        .show_ui(ui, |ui| {
                            for filter in ResizeFilter::ALL {
                                *changed |= ui
                                    .selectable_value(
                                        &mut draft.resize_filter,
                                        filter,
                                        filter.label(),
                                    )
                                    .changed();
                            }
                        });
                    ui.end_row();

                    grid_label_with_help(
                        ui,
                        "실시간 업스케일러",
                        "화면에 그리는 순간 GPU shader로 확대 표시 품질을 보정합니다.",
                    );
                    egui::ComboBox::from_id_salt("display_upscaler")
                        .selected_text(draft.display_upscaler.label())
                        .show_ui(ui, |ui| {
                            for upscaler in DisplayUpscaler::ALL {
                                *changed |= ui
                                    .selectable_value(
                                        &mut draft.display_upscaler,
                                        upscaler,
                                        upscaler.label(),
                                    )
                                    .changed();
                            }
                        });
                    ui.end_row();
                });
            ui.add_space(4.0);
            ui.label(
                RichText::new(
                    "GPU 효과 경로는 자동으로 관리됩니다. 일반적으로 여기서는 보간 품질만 고르면 됩니다.",
                )
                .size(12.0)
                .color(theme::TEXT_MUTED),
            );
        },
    );

    ui.add_space(8.0);
    setting_group(
        ui,
        "이미지 정보 적용",
        "파일 안에 들어 있는 방향과 색상 정보를 표시 결과에 반영합니다.",
        |ui| {
            *changed |= checkbox_with_help(
                ui,
                &mut draft.apply_exif_orientation,
                "EXIF 정보를 이용해서 이미지 돌려보기",
                "카메라나 일부 이미지 파일이 저장한 회전 정보를 읽어서 올바른 방향으로 보여줍니다.",
            );
            *changed |= checkbox_with_help(
            ui,
            &mut draft.apply_embedded_icc,
            "이미지 파일에 포함된 ICC 데이터 적용",
            "이미지에 포함된 색상 프로파일을 적용합니다. 색은 더 정확할 수 있지만 일부 이미지는 읽는 속도가 느려질 수 있습니다.",
        );
            if draft.apply_embedded_icc {
                ui.add_space(4.0);
                ui.label(
                    RichText::new(
                        "ICC 적용 시 색 정확도를 위해 일부 고속 디코딩 경로를 우회합니다.",
                    )
                    .size(12.0)
                    .color(theme::TEXT_MUTED),
                );
            }
        },
    );

    ui.add_space(8.0);
    let ai_enabled = draft.ai_upscale.backend == AiUpscaleBackend::RealEsrganNcnn;
    let ai_header = if ai_enabled {
        format!(
            "외부 AI 업스케일 (실험, {}, {})",
            draft.ai_upscale.backend.label(),
            draft.ai_upscale.prefetch_mode.label()
        )
    } else {
        "외부 AI 업스케일 (실험, 꺼짐)".to_owned()
    };
    egui::CollapsingHeader::new(ai_header)
        .default_open(ai_enabled)
        .show(ui, |ui| {
            show_ai_settings(ui, draft, changed);
        });
}

fn show_performance_settings(ui: &mut egui::Ui, draft: &mut AppSettings, changed: &mut bool) {
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

            if draft.cache_memory_mode == CacheMemoryMode::Auto {
                ui.label(
                    RichText::new(format!(
                        "현재 {:.0} MB",
                        cache_budget_bytes(draft) as f32 / (1024.0 * 1024.0)
                    ))
                    .size(12.0)
                    .color(theme::TEXT_MUTED),
                );
            }
        },
    );
}

fn show_mouse_settings(ui: &mut egui::Ui, draft: &mut AppSettings, changed: &mut bool) {
    setting_group(
        ui,
        "클릭 동작",
        "마우스 버튼으로 창 상태를 바꾸는 설정입니다.",
        |ui| {
            *changed |= checkbox_with_help(
                ui,
                &mut draft.double_click_maximize,
                "더블클릭으로 최대화/복원",
                "뷰어 영역을 두 번 클릭하면 창을 최대화하거나 이전 크기로 되돌립니다.",
            );
            *changed |= checkbox_with_help(
                ui,
                &mut draft.middle_click_fullscreen,
                "가운데 버튼으로 전체화면",
                "마우스 가운데 버튼을 눌렀을 때 전체화면으로 전환합니다.",
            );
        },
    );

    ui.add_space(8.0);
    setting_group(
        ui,
        "이동과 큰 이미지",
        "큰 이미지의 시작 위치와 휠 조작 방식을 정합니다.",
        |ui| {
            egui::Grid::new("settings_mouse_grid")
                .num_columns(2)
                .spacing([14.0, 8.0])
                .show(ui, |ui| {
                    grid_label_with_help(
                        ui,
                        "큰 이미지 시작 위치",
                        "화면보다 큰 이미지를 처음 열 때 어느 위치부터 보여줄지 정합니다.",
                    );
                    egui::ComboBox::from_id_salt("large_image_anchor")
                        .selected_text(draft.large_image_anchor.label())
                        .show_ui(ui, |ui| {
                            for anchor in LargeImageAnchor::ALL {
                                *changed |= ui
                                    .selectable_value(
                                        &mut draft.large_image_anchor,
                                        anchor,
                                        anchor.label(),
                                    )
                                    .changed();
                            }
                        });
                    ui.end_row();

                    grid_label_with_help(
                        ui,
                        "휠 동작",
                        "마우스 휠을 굴렸을 때 페이지를 넘길지, 화면을 움직일지 정합니다.",
                    );
                    egui::ComboBox::from_id_salt("wheel_mode")
                        .selected_text(draft.wheel_mode.label())
                        .show_ui(ui, |ui| {
                            for mode in WheelMode::ALL {
                                *changed |= ui
                                    .selectable_value(&mut draft.wheel_mode, mode, mode.label())
                                    .changed();
                            }
                        });
                    ui.end_row();
                });
        },
    );
}

fn show_ai_settings(ui: &mut egui::Ui, draft: &mut AppSettings, changed: &mut bool) {
    setting_group(
        ui,
        "Real-ESRGAN ncnn-vulkan",
        "실시간 뷰잉용이 아니라 외부 실행 파일로 현재 페이지를 고해상도로 준비하는 실험 기능입니다.",
        |ui| {
            egui::Grid::new("settings_ai_backend_grid")
                .num_columns(2)
                .spacing([14.0, 8.0])
                .show(ui, |ui| {
                    grid_label_with_help(
                        ui,
                        "방식",
                        "AI 업스케일을 끄거나 로컬 Real-ESRGAN ncnn 실행 파일을 사용합니다.",
                    );
                    egui::ComboBox::from_id_salt("ai_upscale_backend")
                        .selected_text(draft.ai_upscale.backend.label())
                        .show_ui(ui, |ui| {
                            for backend in AiUpscaleBackend::ALL {
                                *changed |= ui
                                    .selectable_value(
                                        &mut draft.ai_upscale.backend,
                                        backend,
                                        backend.label(),
                                    )
                                    .changed();
                            }
                        });
                    ui.end_row();

                    grid_label_with_help(
                        ui,
                        "자동 AI 프리페치",
                        "원본을 먼저 보여준 뒤 백그라운드에서 AI 결과를 미리 준비합니다.",
                    );
                    ui.add_enabled_ui(
                        draft.ai_upscale.backend == AiUpscaleBackend::RealEsrganNcnn,
                        |ui| {
                            egui::ComboBox::from_id_salt("ai_upscale_prefetch")
                                .selected_text(draft.ai_upscale.prefetch_mode.label())
                                .show_ui(ui, |ui| {
                                    for mode in AiUpscalePrefetchMode::ALL {
                                        *changed |= ui
                                            .selectable_value(
                                                &mut draft.ai_upscale.prefetch_mode,
                                                mode,
                                                mode.label(),
                                            )
                                            .changed();
                                    }
                                });
                        },
                    );
                    ui.end_row();
                });
        },
    );

    let ai_enabled = draft.ai_upscale.backend == AiUpscaleBackend::RealEsrganNcnn;
    if !ai_enabled {
        ui.add_space(8.0);
        ui.label(
            RichText::new("AI 업스케일은 꺼져 있습니다. 원본 표시와 일반 보간만 사용합니다.")
                .color(theme::TEXT_MUTED),
        );
        return;
    }

    ui.add_space(8.0);
    setting_group(
        ui,
        "Real-ESRGAN 세부 설정",
        "실행 파일, 모델, 출력 형식을 지정합니다.",
        |ui| {
            egui::Grid::new("settings_ai_paths_grid")
            .num_columns(2)
            .spacing([14.0, 8.0])
            .show(ui, |ui| {
                grid_label_with_help(
                    ui,
                    "실행 파일",
                    "realesrgan-ncnn-vulkan 실행 파일 경로입니다. 앱에는 이 외부 도구를 포함하지 않습니다.",
                );
                ui.horizontal(|ui| {
                    let field_width = (ui.available_width() - 58.0).max(140.0);
                    *changed |= ui
                        .add(
                            egui::TextEdit::singleline(&mut draft.ai_upscale.ncnn.executable_path)
                                .desired_width(field_width),
                        )
                        .changed();
                    if ui.button("찾기").clicked() {
                        if let Some(path) = FileDialog::new()
                            .add_filter("Real-ESRGAN", &["exe"])
                            .pick_file()
                        {
                            draft.ai_upscale.ncnn.executable_path = path.display().to_string();
                            *changed = true;
                        }
                    }
                });
                ui.end_row();

                grid_label_with_help(
                    ui,
                    "모델",
                    "Real-ESRGAN에 전달할 모델 이름입니다. 선택한 실행 파일이 지원하는 이름을 사용해야 합니다.",
                );
                *changed |= ui
                    .text_edit_singleline(&mut draft.ai_upscale.ncnn.model_name)
                    .changed();
                ui.end_row();

                grid_label_with_help(
                    ui,
                    "모델 폴더",
                    "모델 파일이 기본 위치가 아닌 곳에 있을 때 지정합니다.",
                );
                ui.horizontal(|ui| {
                    let field_width = (ui.available_width() - 58.0).max(140.0);
                    *changed |= ui
                        .add(
                            egui::TextEdit::singleline(&mut draft.ai_upscale.ncnn.model_path)
                                .desired_width(field_width),
                        )
                        .changed();
                    if ui.button("찾기").clicked() {
                        if let Some(path) = FileDialog::new().pick_folder() {
                            draft.ai_upscale.ncnn.model_path = path.display().to_string();
                            *changed = true;
                        }
                    }
                });
                ui.end_row();

                grid_label_with_help(ui, "배율", "AI 업스케일 결과를 원본 대비 몇 배 크기로 만들지 정합니다.");
                *changed |= ui
                    .add(
                        egui::DragValue::new(&mut draft.ai_upscale.ncnn.scale)
                            .range(2..=4)
                            .speed(1),
                    )
                    .changed();
                ui.end_row();

                grid_label_with_help(
                    ui,
                    "타일",
                    "이미지를 작은 조각으로 나누어 처리하는 크기입니다. 0은 도구 기본값을 사용합니다.",
                );
                *changed |= ui
                    .add(
                        egui::DragValue::new(&mut draft.ai_upscale.ncnn.tile_size)
                            .range(0..=2048)
                            .speed(32),
                    )
                    .changed();
                ui.end_row();

                grid_label_with_help(
                    ui,
                    "출력",
                    "AI 업스케일 결과를 임시로 저장할 이미지 형식입니다.",
                );
                egui::ComboBox::from_id_salt("ai_upscale_output")
                    .selected_text(draft.ai_upscale.ncnn.output_format.as_str())
                    .show_ui(ui, |ui| {
                        for format in ["png", "jpg", "webp"] {
                            if ui
                                .selectable_label(
                                    draft.ai_upscale.ncnn.output_format == format,
                                    format,
                                )
                                .clicked()
                            {
                                draft.ai_upscale.ncnn.output_format = format.to_owned();
                                *changed = true;
                            }
                        }
                    });
                ui.end_row();
            });
        },
    );
}

fn setting_group(
    ui: &mut egui::Ui,
    title: &str,
    description: &str,
    add_contents: impl FnOnce(&mut egui::Ui),
) {
    dialog::setting_card(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(
            RichText::new(title)
                .size(13.5)
                .strong()
                .color(theme::TEXT_PRIMARY),
        );
        ui.label(
            RichText::new(description)
                .size(12.0)
                .color(theme::TEXT_MUTED),
        );
        ui.add_space(8.0);
        add_contents(ui);
    });
}

fn checkbox_with_help(
    ui: &mut egui::Ui,
    value: &mut bool,
    label: &str,
    help: &'static str,
) -> bool {
    let mut changed = false;
    ui.horizontal_wrapped(|ui| {
        changed |= ui.checkbox(value, label).on_hover_text(help).changed();
        info_icon(ui, help);
    });
    changed
}

fn grid_label_with_help(ui: &mut egui::Ui, label: &str, help: &'static str) {
    ui.horizontal(|ui| {
        ui.label(label).on_hover_text(help);
        info_icon(ui, help);
    });
}

fn info_icon(ui: &mut egui::Ui, help: &'static str) {
    ui.label(icons::icon(
        icons::INFO,
        icons::IconStyle::Regular,
        13.0,
        theme::TEXT_MUTED,
    ))
    .on_hover_text(help);
}
