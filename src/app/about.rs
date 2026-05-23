use super::image_info::ImageInfoStatus;
use super::ui::{dialog, icons, theme};
use super::{SuiSuiViewApp, ViewMode};
use crate::core::effects::ImageFilter;
use crate::core::image_info::{ColorProfileInfo, ExifInfo, ImageExifTag, ImageInfo};
use crate::core::state::{AiUpscaleBackend, DecodeMode, FitMode, ReadingDirection};
use eframe::egui::{self, RichText};
use std::path::Path;

const THIRD_PARTY_NOTICES: &str = include_str!("../../THIRD_PARTY_NOTICES.txt");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum AboutSection {
    #[default]
    Image,
    App,
    Notices,
}

impl AboutSection {
    const ALL: [Self; 3] = [Self::Image, Self::App, Self::Notices];

    fn label(self) -> &'static str {
        match self {
            Self::Image => "이미지 정보",
            Self::App => "앱 정보",
            Self::Notices => "오픈소스",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::Image => "현재 페이지의 파일, EXIF, 색상 정보를 확인합니다.",
            Self::App => "버전과 라이선스 정보를 짧게 확인합니다.",
            Self::Notices => "번들된 오픈소스 구성 요소와 라이선스 고지입니다.",
        }
    }

    fn icon(self) -> (char, icons::IconStyle) {
        match self {
            Self::Image => (icons::EYE, icons::IconStyle::Regular),
            Self::App => (icons::INFO, icons::IconStyle::Regular),
            Self::Notices => (icons::DOCUMENT, icons::IconStyle::Regular),
        }
    }
}

impl SuiSuiViewApp {
    pub(super) fn open_about_window(&mut self) {
        self.about_section = AboutSection::Image;
        self.about_open = true;
    }

    pub(super) fn show_about_window(&mut self, ctx: &egui::Context) {
        if !self.about_open {
            return;
        }

        let mut open = self.about_open;
        let mut active_section = self.about_section;
        let dialog_size = dialog::bounded_dialog_size(
            ctx,
            dialog::ABOUT_DIALOG_SIZE,
            dialog::MIN_ABOUT_DIALOG_SIZE,
        );

        egui::Window::new("정보")
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
                                RichText::new("정보")
                                    .strong()
                                    .size(15.0)
                                    .color(theme::TEXT_PRIMARY),
                            );
                            ui.add_space(8.0);
                            for section in AboutSection::ALL {
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

                            match active_section {
                                AboutSection::Image => {
                                    egui::ScrollArea::vertical()
                                        .id_salt("about_image_section")
                                        .max_height(ui.available_height())
                                        .auto_shrink([false, false])
                                        .show(ui, |ui| show_image_info(self, ctx, ui));
                                }
                                AboutSection::App => {
                                    egui::ScrollArea::vertical()
                                        .id_salt("about_app_section")
                                        .max_height(ui.available_height())
                                        .auto_shrink([false, false])
                                        .show(ui, show_app_info);
                                }
                                AboutSection::Notices => show_notices(ui),
                            }
                        });
                    });
                });
            });

        self.about_section = active_section;
        self.about_open = open;
    }
}

fn show_image_info(app: &mut SuiSuiViewApp, ctx: &egui::Context, ui: &mut egui::Ui) {
    match app.current_image_info_status(ctx) {
        ImageInfoStatus::Empty => {
            info_card(ui, "이미지 없음", |ui| {
                ui.label("이미지를 열면 정보가 표시됩니다.");
            });
        }
        ImageInfoStatus::Loading => {
            info_card(ui, "분석 중", |ui| {
                ui.label("현재 페이지의 이미지 정보를 읽는 중입니다.");
            });
        }
        ImageInfoStatus::Failed(error) => {
            info_card(ui, "분석 실패", |ui| {
                ui.label(RichText::new(error).color(theme::TEXT_MUTED));
            });
        }
        ImageInfoStatus::Ready(info) => show_image_info_ready(app, ui, info.as_ref()),
    }
}

fn show_image_info_ready(app: &SuiSuiViewApp, ui: &mut egui::Ui, info: &ImageInfo) {
    show_summary_card(ui, info);
    show_exif_card(ui, &info.exif);
    show_color_card(ui, &info.color, app);
    show_detail_tags_card(ui, &info.exif_tags, &info.gps_tags);
    show_file_location_card(ui, app, info);
    show_view_state_card(ui, app);
}

fn show_summary_card(ui: &mut egui::Ui, info: &ImageInfo) {
    info_card(ui, "이미지 요약", |ui| {
        info_grid(
            ui,
            "image_summary_grid",
            &[
                ("형식", info.summary.format.clone().unwrap_or_else(unknown)),
                (
                    "원본 해상도",
                    format!("{} x {}", info.summary.width, info.summary.height),
                ),
                ("파일 크기", bytes_label(info.summary.file_bytes)),
                ("색상 타입", info.summary.color_type.clone()),
                ("채널 수", info.summary.channel_count.to_string()),
                (
                    "비트 깊이",
                    info.summary
                        .bit_depth
                        .map(|depth| format!("{depth}-bit"))
                        .unwrap_or_else(unknown),
                ),
                (
                    "픽셀 비트 수",
                    format!("{} bpp", info.summary.bits_per_pixel),
                ),
                ("알파 채널", yes_no(info.summary.has_alpha)),
                (
                    "애니메이션",
                    info.summary
                        .animation
                        .clone()
                        .unwrap_or_else(|| "없음".to_owned()),
                ),
            ],
        );
    });
}

fn show_exif_card(ui: &mut egui::Ui, exif: &ExifInfo) {
    info_card(ui, "EXIF", |ui| {
        info_grid(
            ui,
            "image_exif_grid",
            &[
                ("Orientation", value_or_unknown(&exif.orientation)),
                ("촬영 일시", value_or_unknown(&exif.captured_at)),
                ("카메라 제조사", value_or_unknown(&exif.camera_make)),
                ("카메라 모델", value_or_unknown(&exif.camera_model)),
                ("렌즈", value_or_unknown(&exif.lens_model)),
                ("ISO", value_or_unknown(&exif.iso)),
                ("셔터 속도", value_or_unknown(&exif.exposure_time)),
                ("조리개", value_or_unknown(&exif.f_number)),
                ("초점 거리", value_or_unknown(&exif.focal_length)),
                ("노출 보정", value_or_unknown(&exif.exposure_bias)),
                ("플래시", value_or_unknown(&exif.flash)),
            ],
        );
    });
}

fn show_color_card(ui: &mut egui::Ui, color: &ColorProfileInfo, app: &SuiSuiViewApp) {
    let icc_state = match color.icc_profile_bytes {
        Some(bytes) => format!("포함됨 ({})", bytes_label(bytes)),
        None => "없음".to_owned(),
    };
    let icc_read_state = match (color.icc_profile_bytes, color.icc_profile_error.as_ref()) {
        (Some(_), Some(error)) => error.clone(),
        (Some(_), None) => "정상".to_owned(),
        (None, _) => "-".to_owned(),
    };
    info_card(ui, "컬러 / 프로파일", |ui| {
        info_grid(
            ui,
            "image_color_grid",
            &[
                ("ICC 프로파일", icc_state),
                ("ICC 이름", value_or_unknown(&color.icc_profile_name)),
                ("ICC 읽기 상태", icc_read_state),
                ("PNG 색상 타입", value_or_unknown(&color.png_color_type)),
                (
                    "PNG 비트 깊이",
                    color
                        .png_bit_depth
                        .map(|depth| format!("{depth}-bit"))
                        .unwrap_or_else(unknown),
                ),
                ("sRGB", value_or_unknown(&color.png_srgb)),
                ("Gamma", value_or_unknown(&color.png_gamma)),
                ("Chromaticity", value_or_unknown(&color.png_chromaticities)),
                ("Density", value_or_unknown(&color.png_density)),
                ("ICC 적용 설정", on_off(app.settings.apply_embedded_icc)),
            ],
        );
    });
}

fn show_detail_tags_card(ui: &mut egui::Ui, tags: &[ImageExifTag], gps_tags: &[ImageExifTag]) {
    info_card(ui, "상세 태그", |ui| {
        if tags.is_empty() && gps_tags.is_empty() {
            ui.label("EXIF 태그가 없습니다.");
            return;
        }

        egui::CollapsingHeader::new("전체 EXIF 태그")
            .default_open(false)
            .show(ui, |ui| show_tag_grid(ui, "all_exif_tags", tags));

        egui::CollapsingHeader::new("GPS 태그")
            .default_open(false)
            .show(ui, |ui| {
                if gps_tags.is_empty() {
                    ui.label("GPS 태그가 없습니다.");
                } else {
                    show_tag_grid(ui, "gps_exif_tags", gps_tags);
                }
            });
    });
}

fn show_file_location_card(ui: &mut egui::Ui, app: &SuiSuiViewApp, info: &ImageInfo) {
    let Some(source) = app.source.as_ref() else {
        return;
    };
    let page_name = source.page_name(app.current_page).unwrap_or("");
    let display_path = source
        .page_display_path(app.current_page)
        .unwrap_or_else(|| "-".to_owned());
    let page_file_path = source.page_file_path(app.current_page);
    let file_name = page_file_path
        .as_deref()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .or_else(|| {
            Path::new(page_name)
                .file_name()
                .and_then(|name| name.to_str())
        })
        .unwrap_or("-");

    info_card(ui, "파일 / 위치", |ui| {
        info_grid(
            ui,
            "image_file_grid",
            &[
                ("파일명", file_name.to_owned()),
                ("전체 경로", display_path),
                ("책 / 폴더", source.title().to_owned()),
                ("원본 위치", source.source_path().display().to_string()),
                (
                    "압축 내부 경로",
                    if page_file_path.is_some() {
                        "-".to_owned()
                    } else {
                        value_or_dash(page_name)
                    },
                ),
                (
                    "현재 페이지",
                    format!("{} / {}", app.current_page + 1, source.page_count()),
                ),
                ("원본 바이트", bytes_label(info.summary.file_bytes)),
            ],
        );
    });
}

fn show_view_state_card(ui: &mut egui::Ui, app: &SuiSuiViewApp) {
    let transform = app.effects.transform;
    info_card(ui, "현재 보기 상태", |ui| {
        info_grid(
            ui,
            "image_view_state_grid",
            &[
                ("보기 모드", view_mode_label(app.view_mode).to_owned()),
                ("맞춤 / 줌", fit_mode_label(app.fit_mode).to_owned()),
                (
                    "읽기 방향",
                    reading_direction_label(app.reading_direction).to_owned(),
                ),
                (
                    "회전",
                    format!("{}°", u16::from(transform.rotation_quadrants % 4) * 90),
                ),
                ("좌우 반전", on_off(transform.flip_horizontal)),
                ("상하 반전", on_off(transform.flip_vertical)),
                ("색 반전", on_off(app.effects.invert_colors)),
                ("필터", filter_label(app.effects.filter).to_owned()),
                (
                    "EXIF 회전 적용",
                    on_off(app.settings.apply_exif_orientation),
                ),
                (
                    "디코드 모드",
                    decode_mode_label(app.settings.decode_mode).to_owned(),
                ),
                (
                    "리사이즈 필터",
                    app.settings.resize_filter.label().to_owned(),
                ),
                ("AI 보정", ai_state_label(app)),
            ],
        );
    });
}

fn show_app_info(ui: &mut egui::Ui) {
    info_card(ui, "SuiSuiView", |ui| {
        ui.label(
            RichText::new(concat!("SuiSuiView ", env!("CARGO_PKG_VERSION")))
                .size(20.0)
                .strong()
                .color(theme::TEXT_PRIMARY),
        );
        ui.add_space(4.0);
        ui.label("Rust와 egui로 만든 가벼운 이미지/만화 뷰어");
    });

    info_card(ui, "라이선스", |ui| {
        info_grid(
            ui,
            "about_app_grid",
            &[
                ("앱 라이선스", env!("CARGO_PKG_LICENSE").to_owned()),
                (
                    "소스 코드",
                    "GPL-3.0-only 조건에 따라 배포 버전에 대응하는 소스 코드를 제공합니다."
                        .to_owned(),
                ),
                (
                    "배포",
                    "GitHub판은 수동 업데이트, Store판은 Microsoft Store 자동 업데이트를 사용합니다."
                        .to_owned(),
                ),
                (
                    "고지",
                    "오픈소스 탭에서 전체 구성 요소를 확인할 수 있습니다.".to_owned(),
                ),
            ],
        );
    });
}

fn show_notices(ui: &mut egui::Ui) {
    info_card(ui, "오픈소스", |ui| {
        ui.label("이 앱은 아래 오픈소스 구성 요소와 라이선스를 포함합니다.");
    });

    ui.add_space(8.0);
    dialog::setting_card(ui, |ui| {
        egui::ScrollArea::both()
            .id_salt("third_party_notices")
            .max_height((ui.available_height() - 8.0).max(0.0))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add(
                    egui::Label::new(RichText::new(THIRD_PARTY_NOTICES).monospace())
                        .selectable(true),
                );
            });
    });
}

fn info_card(ui: &mut egui::Ui, title: &str, add_contents: impl FnOnce(&mut egui::Ui)) {
    ui.add_space(8.0);
    dialog::setting_card(ui, |ui| {
        ui.label(RichText::new(title).strong().color(theme::TEXT_PRIMARY));
        ui.add_space(6.0);
        add_contents(ui);
    });
}

fn info_grid(ui: &mut egui::Ui, id: &'static str, rows: &[(&str, String)]) {
    egui::Grid::new(id)
        .num_columns(2)
        .spacing([16.0, 8.0])
        .striped(false)
        .show(ui, |ui| {
            for (label, value) in rows {
                ui.label(RichText::new(*label).color(theme::TEXT_MUTED));
                ui.add(egui::Label::new(value.as_str()).selectable(true));
                ui.end_row();
            }
        });
}

fn show_tag_grid(ui: &mut egui::Ui, id: &'static str, tags: &[ImageExifTag]) {
    if tags.is_empty() {
        ui.label("태그가 없습니다.");
        return;
    }

    egui::Grid::new(id)
        .num_columns(3)
        .spacing([12.0, 6.0])
        .striped(true)
        .show(ui, |ui| {
            for tag in tags {
                ui.label(RichText::new(&tag.ifd).color(theme::TEXT_MUTED));
                ui.label(&tag.tag);
                ui.add(egui::Label::new(tag.value.as_str()).selectable(true));
                ui.end_row();
            }
        });
}

fn bytes_label(bytes: usize) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    let bytes = bytes as f64;
    if bytes >= MB {
        format!("{:.2} MB", bytes / MB)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes / KB)
    } else {
        format!("{bytes:.0} B")
    }
}

fn value_or_unknown(value: &Option<String>) -> String {
    value.clone().unwrap_or_else(unknown)
}

fn value_or_dash(value: &str) -> String {
    if value.trim().is_empty() {
        "-".to_owned()
    } else {
        value.to_owned()
    }
}

fn unknown() -> String {
    "정보 없음".to_owned()
}

fn yes_no(value: bool) -> String {
    if value {
        "있음".to_owned()
    } else {
        "없음".to_owned()
    }
}

fn on_off(value: bool) -> String {
    if value {
        "켜짐".to_owned()
    } else {
        "꺼짐".to_owned()
    }
}

fn view_mode_label(mode: ViewMode) -> &'static str {
    match mode {
        ViewMode::Single => "한 페이지",
        ViewMode::Double => "두 페이지",
    }
}

fn fit_mode_label(mode: FitMode) -> &'static str {
    match mode {
        FitMode::FitPage => "화면 맞춤",
        FitMode::FitWidth => "폭 맞춤",
        FitMode::FitHeight => "높이 맞춤",
        FitMode::Original => "원본 크기",
        FitMode::Manual => "수동 줌",
    }
}

fn reading_direction_label(direction: ReadingDirection) -> &'static str {
    match direction {
        ReadingDirection::LeftToRight => "왼쪽에서 오른쪽",
        ReadingDirection::RightToLeft => "오른쪽에서 왼쪽",
    }
}

fn decode_mode_label(mode: DecodeMode) -> &'static str {
    match mode {
        DecodeMode::AutoFast => "자동 / 빠름",
        DecodeMode::HighQuality => "고품질 / 호환",
    }
}

fn filter_label(filter: ImageFilter) -> &'static str {
    match filter {
        ImageFilter::None => "꺼짐",
        ImageFilter::Smooth => "부드럽게",
        ImageFilter::SmoothSharpen => "부드럽게 + 선명하게",
        ImageFilter::RcasSharpen => "RCAS 선명하게",
    }
}

fn ai_state_label(app: &SuiSuiViewApp) -> String {
    match app.settings.ai_upscale.backend {
        AiUpscaleBackend::Off => "꺼짐".to_owned(),
        AiUpscaleBackend::RealEsrganNcnn if app.use_ai_upscaled_pages => {
            "켜짐 / 표시 사용".to_owned()
        }
        AiUpscaleBackend::RealEsrganNcnn => "켜짐 / 원본 표시".to_owned(),
    }
}
