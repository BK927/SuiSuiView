use super::ui::{dialog, icons, theme};
use super::{
    apply_window_level, settings_bookmarks, settings_input, settings_performance, SuiSuiViewApp,
};
use crate::core::state::{
    AiUpscaleBackend, AiUpscalePrefetchMode, AppSettings, DisplayUpscaler, EdgePageAction,
    GpuEffectMode, PageTransitionStyle, ResizeFilter,
};
use eframe::egui::{self, RichText};
use rfd::FileDialog;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum SettingsSection {
    #[default]
    General,
    View,
    ImageProcessing,
    Performance,
    Bookmarks,
    Keyboard,
    Mouse,
}

impl SettingsSection {
    const ALL: [Self; 7] = [
        Self::General,
        Self::View,
        Self::ImageProcessing,
        Self::Performance,
        Self::Bookmarks,
        Self::Keyboard,
        Self::Mouse,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::General => "일반",
            Self::View => "보기",
            Self::ImageProcessing => "영상 처리",
            Self::Performance => "성능",
            Self::Bookmarks => "책갈피",
            Self::Keyboard => "키보드",
            Self::Mouse => "마우스",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::General => "삭제 확인, 창 표시, 페이지 끝 동작",
            Self::View => "상단 도구막대, 상태바, 뷰어 보조 표시",
            Self::ImageProcessing => "보간, 실시간 업스케일, 색상 보정",
            Self::Performance => "디코딩, 미리보기, 프리로드, 캐시 정책",
            Self::Bookmarks => "이어보기, 책갈피 저장 범위와 기록 정리",
            Self::Keyboard => "현재 단축키 확인, 추가, 변경, 초기화",
            Self::Mouse => "더블클릭, 가운데 버튼, 휠 조작",
        }
    }

    fn icon(self) -> (char, icons::IconStyle) {
        match self {
            Self::General => (icons::SETTINGS, icons::IconStyle::Regular),
            Self::View => (icons::EYE, icons::IconStyle::Regular),
            Self::ImageProcessing => (icons::WAND, icons::IconStyle::Regular),
            Self::Performance => (icons::DOCUMENT, icons::IconStyle::Regular),
            Self::Bookmarks => (icons::BOOKMARK, icons::IconStyle::Regular),
            Self::Keyboard => (icons::DOCUMENT, icons::IconStyle::Regular),
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
                                    SettingsSection::View => {
                                        settings_bookmarks::show_view_settings(
                                            ui,
                                            &mut draft,
                                            &mut changed,
                                        );
                                    }
                                    SettingsSection::ImageProcessing => {
                                        show_image_processing_settings(
                                            ui,
                                            &mut draft,
                                            &mut changed,
                                        );
                                    }
                                    SettingsSection::Performance => {
                                        settings_performance::show_performance_settings(
                                            ui,
                                            &mut draft,
                                            self.target_long_edge,
                                            self.visible_page_count(),
                                            &mut changed,
                                        );
                                    }
                                    SettingsSection::Bookmarks => {
                                        self.show_bookmark_settings(ui, &mut draft, &mut changed);
                                    }
                                    SettingsSection::Keyboard => {
                                        self.show_keyboard_settings(
                                            ctx,
                                            ui,
                                            &mut draft,
                                            &mut changed,
                                        );
                                    }
                                    SettingsSection::Mouse => {
                                        settings_input::show_mouse_settings(
                                            ui,
                                            &mut draft,
                                            &mut changed,
                                        );
                                    }
                                });
                        });
                    });
                });
            });

        if self.settings_section == SettingsSection::Keyboard
            && active_section != SettingsSection::Keyboard
        {
            self.shortcut_capture = None;
            self.shortcut_conflict = None;
        }
        self.settings_section = active_section;
        self.settings_open = open;
        if !self.settings_open {
            self.shortcut_capture = None;
            self.shortcut_conflict = None;
        }
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
        let previous_max_remembered_books = self.settings.max_remembered_books;

        self.settings = settings;
        self.store.update_settings(self.settings.clone());
        self.refresh_single_instance_listener();
        self.pending_state_save_at = None;
        apply_window_level(ctx, self.settings.always_on_top);

        let decode_changed = previous_decode != self.decode_options();
        let preview_changed = previous_preview != self.settings.progressive_preview_enabled;
        if decode_changed || preview_changed {
            self.decoded_pages.clear();
            self.decoded_bytes = 0;
            if decode_changed {
                self.page_metrics.clear();
            }
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
            self.clear_queued_ai_upscale_pages();
            self.ai_upscale_manual_requests.clear();
            self.ai_upscale_failures.clear();
        } else if ai_prefetch_changed {
            self.clear_queued_ai_upscale_pages();
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
                self.worker_center_page(),
                self.last_nav_direction,
                self.target_long_edge,
                self.visible_page_count(),
                self.worker_options(),
            );
        }
        if ai_output_changed || ai_prefetch_changed {
            self.refresh_ai_prefetch_queue();
        }
        if previous_max_remembered_books != self.settings.max_remembered_books {
            self.store
                .prune_auto_bookmarks(self.settings.max_remembered_books);
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
            *changed |= checkbox_with_help(
                ui,
                &mut draft.remember_recent_locations,
                "최근 위치 저장",
                "열기 메뉴와 파일 대화상자에서 최근 위치를 다시 찾기 쉽게 보관합니다.",
            );
            #[cfg(target_os = "windows")]
            {
                *changed |= checkbox_with_help(
                    ui,
                    &mut draft.single_instance,
                    "한 개의 프로그램만 실행",
                    "새로 실행된 SuiSuiView가 받은 파일을 이미 열린 창으로 전달합니다.",
                );
            }
            #[cfg(not(target_os = "windows"))]
            {
                ui.add_enabled(
                    false,
                    egui::Checkbox::new(&mut draft.single_instance, "한 개의 프로그램만 실행"),
                )
                .on_hover_text("이 옵션은 Windows에서만 사용할 수 있습니다.");
            }
            *changed |= checkbox_with_help(
                ui,
                &mut draft.show_toasts,
                "중요 알림을 화면 구석에 표시",
                "끄면 오류와 작업 결과는 상태바 문구로만 남기고 토스트 알림은 띄우지 않습니다.",
            );
        },
    );

    ui.add_space(8.0);
    setting_group(
        ui,
        "페이지 끝 동작",
        "일반 이미지/폴더와 압축파일에서 끝 페이지를 만났을 때의 동작입니다.",
        |ui| {
            egui::Grid::new("settings_edge_page_grid")
                .num_columns(2)
                .spacing([14.0, 8.0])
                .show(ui, |ui| {
                    grid_label_with_help(
                        ui,
                        "이미지/폴더",
                        "일반 이미지 파일이나 폴더를 볼 때 처음/마지막 페이지에서의 동작입니다.",
                    );
                    egui::ComboBox::from_id_salt("image_edge_page_action")
                        .selected_text(draft.image_edge_page_action.label())
                        .show_ui(ui, |ui| {
                            for action in EdgePageAction::ALL {
                                *changed |= ui
                                    .selectable_value(
                                        &mut draft.image_edge_page_action,
                                        action,
                                        action.label(),
                                    )
                                    .changed();
                            }
                        });
                    ui.end_row();

                    grid_label_with_help(
                        ui,
                        "압축파일",
                        "ZIP/CBZ 안의 이미지를 볼 때 처음/마지막 페이지에서의 동작입니다.",
                    );
                    egui::ComboBox::from_id_salt("archive_edge_page_action")
                        .selected_text(draft.archive_edge_page_action.label())
                        .show_ui(ui, |ui| {
                            for action in EdgePageAction::ALL {
                                *changed |= ui
                                    .selectable_value(
                                        &mut draft.archive_edge_page_action,
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
            egui::Grid::new("settings_transition_grid")
                .num_columns(2)
                .spacing([14.0, 8.0])
                .show(ui, |ui| {
                    grid_label_with_help(
                        ui,
                        "페이지 전환",
                        "페이지를 넘길 때 사용할 가벼운 화면 전환 효과입니다.",
                    );
                    let mut transition_style = draft.effective_page_transition_style();
                    egui::ComboBox::from_id_salt("page_transition_style")
                        .selected_text(transition_style.label())
                        .show_ui(ui, |ui| {
                            for style in PageTransitionStyle::ALL {
                                *changed |= ui
                                    .selectable_value(&mut transition_style, style, style.label())
                                    .changed();
                            }
                        });
                    draft.set_page_transition_style(transition_style);
                    ui.end_row();
                });
            ui.add_space(6.0);
            egui::Grid::new("settings_image_processing_grid")
                .num_columns(2)
                .spacing([14.0, 8.0])
                .show(ui, |ui| {
                    grid_label_with_help(
                        ui,
                        "기본 업스케일러",
                        "캐시에 저장할 표시용 이미지를 준비할 때 쓰는 기본 리사이즈 방식입니다. 큰 이미지를 줄이거나 GPU 가속 업스케일러를 사용할 수 없을 때의 표시 품질을 결정합니다.",
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
                        "GPU 가속 업스케일러",
                        "화면에 확대해서 표시할 때 GPU shader로 추가 보정합니다. 사용할 수 있으면 작은 이미지는 CPU에서 먼저 키우지 않고 GPU가 확대 표시를 맡습니다.",
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
                    "기본 업스케일러는 캐시 준비와 CPU fallback에 쓰이고, GPU 가속 업스케일러는 확대 표시가 필요할 때 화면에서 적용됩니다.",
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

pub(in crate::app) fn setting_group(
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

pub(in crate::app) fn checkbox_with_help(
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

pub(in crate::app) fn grid_label_with_help(ui: &mut egui::Ui, label: &str, help: &'static str) {
    ui.horizontal(|ui| {
        ui.label(label).on_hover_text(help);
        info_icon(ui, help);
    });
}

pub(in crate::app) fn info_icon(ui: &mut egui::Ui, help: &'static str) {
    ui.label(icons::icon(
        icons::INFO,
        icons::IconStyle::Regular,
        13.0,
        theme::TEXT_MUTED,
    ))
    .on_hover_text(help);
}
