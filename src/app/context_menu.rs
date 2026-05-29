use super::{
    commands::{AppCommand, DeleteMode},
    SuiSuiViewApp,
};
use crate::core::effects::ImageFilter;
use crate::core::state::{FitMode, ReadingDirection};
use eframe::egui;

impl SuiSuiViewApp {
    pub(in crate::app) fn show_context_menu(
        &mut self,
        ctx: &egui::Context,
        response: &egui::Response,
    ) {
        response.context_menu(|ui| {
            ui.set_min_width(280.0);
            let has_book = self.source.is_some();

            self.context_action(ui, ctx, "열기", "F2", AppCommand::OpenFile, true);
            self.context_action(ui, ctx, "폴더 열기", "F", AppCommand::OpenFolder, true);
            self.context_action(ui, ctx, "닫기", "F4", AppCommand::CloseBook, has_book);

            ui.separator();
            self.context_filter(ui, ctx, "필터적용 안함", "U", ImageFilter::None, has_book);
            self.context_filter(ui, ctx, "부드럽게", "I", ImageFilter::Smooth, has_book);
            self.context_filter(
                ui,
                ctx,
                "부드럽게+선명하게",
                "S",
                ImageFilter::SmoothSharpen,
                has_book,
            );

            ui.separator();
            ui.menu_button("이미지 이동", |ui| {
                self.context_action(
                    ui,
                    ctx,
                    "다음 이미지",
                    "PgDn",
                    AppCommand::NextPage,
                    has_book,
                );
                self.context_action(
                    ui,
                    ctx,
                    "이전 이미지",
                    "PgUp",
                    AppCommand::PreviousPage,
                    has_book,
                );
                self.context_action(
                    ui,
                    ctx,
                    "맨 처음 이미지",
                    "Home",
                    AppCommand::Home,
                    has_book,
                );
                self.context_action(
                    ui,
                    ctx,
                    "맨 마지막 이미지",
                    "End",
                    AppCommand::End,
                    has_book,
                );
                ui.separator();
                self.context_action(
                    ui,
                    ctx,
                    "다음 10페이지",
                    "Ctrl+PgDn",
                    AppCommand::MovePages(10),
                    has_book,
                );
                self.context_action(
                    ui,
                    ctx,
                    "이전 10페이지",
                    "Ctrl+PgUp",
                    AppCommand::MovePages(-10),
                    has_book,
                );
                self.context_action(
                    ui,
                    ctx,
                    "랜덤하게 다음 페이지",
                    "Ctrl+Alt+PgDn",
                    AppCommand::RandomForward,
                    has_book,
                );
                self.context_action(
                    ui,
                    ctx,
                    "랜덤하게 이전 페이지",
                    "Ctrl+Alt+PgUp",
                    AppCommand::RandomBackward,
                    has_book,
                );
                ui.separator();
                self.context_action(
                    ui,
                    ctx,
                    "다음 폴더/압축파일",
                    "]",
                    AppCommand::NextBook,
                    has_book,
                );
                self.context_action(
                    ui,
                    ctx,
                    "이전 폴더/압축파일",
                    "[",
                    AppCommand::PreviousBook,
                    has_book,
                );
            });

            ui.menu_button("보기 모드", |ui| {
                self.context_fit_mode(ui, ctx, "원본 크기(100%)", "0", FitMode::Original, has_book);
                self.context_fit_mode(
                    ui,
                    ctx,
                    "꽉 차게 보기",
                    "1 / 9 / Z",
                    FitMode::FitPage,
                    has_book,
                );
                self.context_fit_mode(ui, ctx, "폭맞춤", "8", FitMode::FitWidth, has_book);
                ui.separator();
                self.context_action(
                    ui,
                    ctx,
                    "두장 보기(왼쪽→오른쪽)",
                    "7",
                    AppCommand::SetDouble(ReadingDirection::LeftToRight),
                    has_book,
                );
                self.context_action(
                    ui,
                    ctx,
                    "두장 보기(왼쪽←오른쪽)",
                    "6",
                    AppCommand::SetDouble(ReadingDirection::RightToLeft),
                    has_book,
                );
                self.context_action(
                    ui,
                    ctx,
                    "두장 보기 모드 전환",
                    "2",
                    AppCommand::ToggleDouble,
                    has_book,
                );
            });

            ui.menu_button("축소/확대 보기", |ui| {
                self.context_action(ui, ctx, "확대", "+", AppCommand::Zoom(1.1), has_book);
                self.context_action(ui, ctx, "축소", "-", AppCommand::Zoom(0.9), has_book);
                self.context_action(
                    ui,
                    ctx,
                    "1% 크게 보기",
                    "Ctrl++",
                    AppCommand::ZoomFine(0.01),
                    has_book,
                );
                self.context_action(
                    ui,
                    ctx,
                    "1% 작게 보기",
                    "Ctrl+-",
                    AppCommand::ZoomFine(-0.01),
                    has_book,
                );
            });

            ui.menu_button("이미지 돌려보기", |ui| {
                self.context_action(
                    ui,
                    ctx,
                    "돌려보지 않기",
                    "Alt+↑",
                    AppCommand::SetRotation(0),
                    has_book,
                );
                self.context_action(
                    ui,
                    ctx,
                    "왼쪽으로 돌려보기",
                    "Alt+←",
                    AppCommand::SetRotation(3),
                    has_book,
                );
                self.context_action(
                    ui,
                    ctx,
                    "오른쪽으로 돌려보기",
                    "Alt+→",
                    AppCommand::SetRotation(1),
                    has_book,
                );
                self.context_action(
                    ui,
                    ctx,
                    "거꾸로 돌려보기",
                    "Alt+↓",
                    AppCommand::SetRotation(2),
                    has_book,
                );
                ui.separator();
                self.context_action(
                    ui,
                    ctx,
                    "반시계 방향으로 돌려보기",
                    "Ctrl+L",
                    AppCommand::RotateCounterClockwise,
                    has_book,
                );
                self.context_action(
                    ui,
                    ctx,
                    "시계 방향으로 돌려보기",
                    "Ctrl+R",
                    AppCommand::RotateClockwise,
                    has_book,
                );
            });

            ui.menu_button("영상 처리", |ui| {
                self.context_toggle(
                    ui,
                    ctx,
                    "이미지 반전",
                    "Ctrl+I",
                    self.effects.invert_colors,
                    AppCommand::ToggleInvert,
                );
                self.context_toggle(
                    ui,
                    ctx,
                    "감마 보정",
                    "Ctrl+G",
                    self.effects.gamma,
                    AppCommand::ToggleGamma,
                );
                self.context_toggle(
                    ui,
                    ctx,
                    "상하 반전",
                    "Ctrl+F",
                    self.effects.transform.flip_vertical,
                    AppCommand::ToggleFlipVertical,
                );
                self.context_toggle(
                    ui,
                    ctx,
                    "좌우 반전",
                    "Ctrl+M",
                    self.effects.transform.flip_horizontal,
                    AppCommand::ToggleFlipHorizontal,
                );
                ui.separator();
                self.context_action(
                    ui,
                    ctx,
                    "AI 업스케일",
                    "",
                    AppCommand::UpscaleCurrentPage,
                    has_book,
                );
            });

            ui.separator();
            self.context_action(
                ui,
                ctx,
                "윈도우 탐색기 열기",
                "Ctrl+Enter",
                AppCommand::OpenExplorer,
                has_book,
            );
            self.context_action(
                ui,
                ctx,
                "파일 삭제",
                "Del",
                AppCommand::Delete(DeleteMode::Recycle),
                has_book,
            );
            self.context_action(
                ui,
                ctx,
                "파일 완전히 삭제",
                "Shift+Del",
                AppCommand::Delete(DeleteMode::Permanent),
                has_book,
            );
            self.context_action(
                ui,
                ctx,
                "클립보드로 복사하기",
                "Ctrl+C",
                AppCommand::CopyPageImage,
                has_book,
            );
            self.context_action(
                ui,
                ctx,
                "보이는 이미지 복사",
                "Ctrl+Alt+C",
                AppCommand::CopyDisplayImage,
                has_book,
            );
            self.context_action(
                ui,
                ctx,
                "파일 경로 복사",
                "Ctrl+Alt+Shift+C",
                AppCommand::CopyPath,
                has_book,
            );

            ui.separator();
            self.context_action(
                ui,
                ctx,
                "전체화면",
                "F11",
                AppCommand::ToggleFullscreen,
                true,
            );
            self.context_action(
                ui,
                ctx,
                "최대화/복원",
                "M",
                AppCommand::ToggleMaximized,
                true,
            );
            self.context_action(ui, ctx, "최소화", "Q", AppCommand::Minimize, true);
            if context_selectable(
                ui,
                self.settings.always_on_top,
                "항상 위에 표시",
                "Ctrl+A",
                true,
            )
            .clicked()
            {
                self.apply_command(ctx, AppCommand::ToggleAlwaysOnTop);
                ui.close();
            }
            self.context_action(ui, ctx, "환경설정", "F5", AppCommand::OpenSettings, true);
            self.context_action(ui, ctx, "정보", "F1", AppCommand::OpenAbout, true);
            self.context_action(ui, ctx, "종료", "X", AppCommand::Quit, true);
        });
    }

    fn context_action(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        label: &str,
        shortcut: &str,
        command: AppCommand,
        enabled: bool,
    ) {
        if context_button(ui, label, shortcut, enabled).clicked() {
            self.apply_command(ctx, command);
            ui.close();
        }
    }

    fn context_filter(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        label: &str,
        shortcut: &str,
        filter: ImageFilter,
        enabled: bool,
    ) {
        if context_selectable(ui, self.effects.filter == filter, label, shortcut, enabled).clicked()
        {
            self.apply_command(ctx, AppCommand::SetFilter(filter));
            ui.close();
        }
    }

    fn context_fit_mode(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        label: &str,
        shortcut: &str,
        mode: FitMode,
        enabled: bool,
    ) {
        if context_selectable(ui, self.fit_mode == mode, label, shortcut, enabled).clicked() {
            self.apply_command(ctx, AppCommand::SetFitMode(mode));
            ui.close();
        }
    }

    fn context_toggle(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        label: &str,
        shortcut: &str,
        selected: bool,
        command: AppCommand,
    ) {
        let enabled = self.source.is_some();
        if context_selectable(ui, selected, label, shortcut, enabled).clicked() {
            self.apply_command(ctx, command);
            ui.close();
        }
    }
}

fn context_button(ui: &mut egui::Ui, label: &str, shortcut: &str, enabled: bool) -> egui::Response {
    ui.add_enabled(
        enabled,
        egui::Button::new(label).shortcut_text(shortcut.to_owned()),
    )
}

fn context_selectable(
    ui: &mut egui::Ui,
    selected: bool,
    label: &str,
    shortcut: &str,
    enabled: bool,
) -> egui::Response {
    ui.add_enabled(
        enabled,
        egui::Button::new(label)
            .selected(selected)
            .shortcut_text(shortcut.to_owned()),
    )
}
