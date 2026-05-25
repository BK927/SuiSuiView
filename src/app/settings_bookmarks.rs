use super::settings::{checkbox_with_help, setting_group};
use super::SuiSuiViewApp;
use crate::core::state::AppSettings;
use eframe::egui;

pub(in crate::app) fn show_view_settings(
    ui: &mut egui::Ui,
    draft: &mut AppSettings,
    changed: &mut bool,
) {
    setting_group(
        ui,
        "도구막대와 상태 표시",
        "읽는 중에 보이는 보조 UI를 켜고 끕니다.",
        |ui| {
            *changed |= checkbox_with_help(
                ui,
                &mut draft.show_status_bar,
                "하단 상태바 표시",
                "창 아래쪽에 현재 상태와 짧은 안내 문구를 표시합니다.",
            );
            *changed |= checkbox_with_help(
                ui,
                &mut draft.top_bar_pinned,
                "상단 도구막대 고정",
                "끄면 마우스를 창 위쪽으로 가져갈 때만 상단 도구막대가 나타납니다.",
            );
            *changed |= checkbox_with_help(
                ui,
                &mut draft.show_filename_overlay,
                "이미지 위에 파일명 표시",
                "상단 도구막대가 숨겨져 있을 때 현재 파일명을 작게 겹쳐 표시합니다.",
            );
        },
    );

    ui.add_space(8.0);
    setting_group(
        ui,
        "뷰어 표시",
        "이미지 영역의 테두리와 페이지 넘김 보조 표시입니다.",
        |ui| {
            *changed |= checkbox_with_help(
                ui,
                &mut draft.show_main_border,
                "메인 뷰어 테두리 표시",
                "이미지 표시 영역 가장자리를 얇은 선으로 구분합니다.",
            );
            *changed |= checkbox_with_help(
                ui,
                &mut draft.show_page_arrows,
                "좌우 페이지 화살표 표시",
                "마우스를 좌우 가장자리로 가져가면 페이지 이동 화살표를 보여줍니다.",
            );
            *changed |= checkbox_with_help(
                ui,
                &mut draft.remember_zoom_per_book,
                "보기/확대 설정을 책별로 저장",
                "책마다 맞춤 방식과 수동 확대 배율을 기억합니다. 화면 이동 위치는 저장하지 않습니다.",
            );
        },
    );
}

impl SuiSuiViewApp {
    pub(in crate::app) fn show_bookmark_settings(
        &mut self,
        ui: &mut egui::Ui,
        draft: &mut AppSettings,
        changed: &mut bool,
    ) {
        setting_group(
            ui,
            "이어보기",
            "마지막으로 보던 책 위치와 압축파일 내부 위치를 기억합니다.",
            |ui| {
                *changed |= checkbox_with_help(
                    ui,
                    &mut draft.auto_save_reading_position,
                    "보고 있던 이미지 위치를 자동 저장",
                    "페이지 이동 시 현재 책의 마지막 위치를 이어보기 기록으로 저장합니다.",
                );
                *changed |= checkbox_with_help(
                    ui,
                    &mut draft.resume_by_file_identity,
                    "경로가 바뀌어도 같은 파일이면 이어보기",
                    "파일/압축파일/폴더의 식별값이 같으면 위치가 달라져도 마지막으로 보던 페이지를 복원합니다.",
                );
                *changed |= checkbox_with_help(
                    ui,
                    &mut draft.share_state_between_instances,
                    "프로그램이 여러 개 실행중일 때 환경설정/책갈피 공유",
                    "여러 창이 같은 상태 파일을 쓸 때 저장 직전 기록을 다시 읽어 충돌을 줄입니다.",
                );
                ui.horizontal_wrapped(|ui| {
                    ui.label("최대 이어보기 저장 책 수");
                    super::settings::info_icon(
                        ui,
                        "자동 이어보기/최근 기록만 정리합니다. 수동 북마크가 있는 책은 보존됩니다.",
                    );
                    *changed |= ui
                        .add(
                            egui::DragValue::new(&mut draft.max_remembered_books)
                                .range(1..=500)
                                .speed(1),
                        )
                        .changed();
                });
            },
        );

        ui.add_space(8.0);
        setting_group(
            ui,
            "압축파일 내부 위치",
            "ZIP/CBZ 안에서 어떤 내부 이미지까지 봤는지 기억합니다.",
            |ui| {
                *changed |= checkbox_with_help(
                    ui,
                    &mut draft.remember_archive_page_name,
                    "압축된 이미지 파일의 내부 경로 기억",
                    "압축파일 안의 파일 이름을 우선 저장해서 목록 순서가 조금 바뀌어도 같은 이미지로 복원합니다.",
                );
                if ui.button("압축파일 내부 위치 기록 삭제").clicked() {
                    let cleared = self.store.clear_archive_page_names();
                    self.set_status(format!(
                        "압축파일 내부 위치 기록 {cleared}개를 삭제했습니다."
                    ));
                }
            },
        );
    }
}
