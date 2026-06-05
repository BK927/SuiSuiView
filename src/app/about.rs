use super::image_info::ImageInfoStatus;
use super::ui::{dialog, icons, theme};
use super::{SuiSuiViewApp, ViewMode};
use crate::core::i18n::I18n;
use crate::core::image_info::{ColorProfileInfo, ExifInfo, ImageExifTag, ImageInfo};
use crate::core::state::AiUpscaleBackend;
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

    fn label(self, i18n: I18n) -> String {
        match self {
            Self::Image => i18n.text("about.section.image"),
            Self::App => i18n.text("about.section.app"),
            Self::Notices => i18n.text("about.section.notices"),
        }
    }

    fn description(self, i18n: I18n) -> String {
        match self {
            Self::Image => i18n.text("about.section.image.desc"),
            Self::App => i18n.text("about.section.app.desc"),
            Self::Notices => i18n.text("about.section.notices.desc"),
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
        self.about_section = if self.has_image_info_target() {
            AboutSection::Image
        } else {
            AboutSection::App
        };
        self.about_open = true;
    }

    pub(super) fn show_about_window(&mut self, ctx: &egui::Context) {
        if !self.about_open {
            return;
        }

        let mut open = self.about_open;
        let mut active_section = self.about_section;
        let i18n = self.i18n();
        let image_section_enabled = self.has_image_info_target();
        if !image_section_enabled && active_section == AboutSection::Image {
            active_section = AboutSection::App;
        }
        let dialog_size = dialog::bounded_dialog_size(
            ctx,
            dialog::ABOUT_DIALOG_SIZE,
            dialog::MIN_ABOUT_DIALOG_SIZE,
        );

        egui::Window::new(i18n.text("about.window"))
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
                                RichText::new(i18n.text("about.nav_title"))
                                    .strong()
                                    .size(15.0)
                                    .color(theme::TEXT_PRIMARY),
                            );
                            ui.add_space(8.0);
                            for section in AboutSection::ALL {
                                let (icon, icon_style) = section.icon();
                                let enabled =
                                    section != AboutSection::Image || image_section_enabled;
                                let response = ui
                                    .add_enabled_ui(enabled, |ui| {
                                        dialog::nav_button(
                                            ui,
                                            active_section == section,
                                            icon,
                                            icon_style,
                                            &section.label(i18n),
                                        )
                                    })
                                    .inner;
                                let response = if enabled {
                                    response
                                } else {
                                    response.on_hover_text(i18n.text("about.image_unavailable"))
                                };
                                if response.clicked() {
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
                                &active_section.label(i18n),
                                &active_section.description(i18n),
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
                                        .show(ui, |ui| show_app_info(ui, i18n));
                                }
                                AboutSection::Notices => show_notices(ui, i18n),
                            }
                        });
                    });
                });
            });

        self.about_section = active_section;
        self.about_open = open;
    }

    fn has_image_info_target(&self) -> bool {
        self.book_id.is_some()
            && self
                .source
                .as_ref()
                .is_some_and(|source| self.current_page < source.page_count())
    }
}

fn show_image_info(app: &mut SuiSuiViewApp, ctx: &egui::Context, ui: &mut egui::Ui) {
    let i18n = app.i18n();
    match app.current_image_info_status(ctx) {
        ImageInfoStatus::Empty => {
            info_card(ui, &i18n.text("about.image.empty.title"), |ui| {
                ui.label(i18n.text("about.image.empty.body"));
            });
        }
        ImageInfoStatus::Loading => {
            info_card(ui, &i18n.text("about.image.loading.title"), |ui| {
                ui.label(i18n.text("about.image.loading.body"));
            });
        }
        ImageInfoStatus::Failed(error) => {
            info_card(ui, &i18n.text("about.image.failed.title"), |ui| {
                ui.label(RichText::new(error).color(theme::TEXT_MUTED));
            });
        }
        ImageInfoStatus::Ready(info) => show_image_info_ready(app, ui, info.as_ref(), i18n),
    }
}

fn show_image_info_ready(app: &SuiSuiViewApp, ui: &mut egui::Ui, info: &ImageInfo, i18n: I18n) {
    show_summary_card(ui, info, i18n);
    show_exif_card(ui, &info.exif, i18n);
    show_color_card(ui, &info.color, app, i18n);
    show_detail_tags_card(ui, &info.exif_tags, &info.gps_tags, i18n);
    show_file_location_card(ui, app, info, i18n);
    show_view_state_card(ui, app, i18n);
}

fn show_summary_card(ui: &mut egui::Ui, info: &ImageInfo, i18n: I18n) {
    info_card(ui, &i18n.text("about.image.summary.title"), |ui| {
        info_grid(
            ui,
            "image_summary_grid",
            &[
                (
                    &i18n.text("about.image.summary.format"),
                    info.summary.format.clone().unwrap_or_else(|| unknown(i18n)),
                ),
                (
                    &i18n.text("about.image.summary.dimensions"),
                    format!("{} x {}", info.summary.width, info.summary.height),
                ),
                (
                    &i18n.text("about.image.summary.file_size"),
                    bytes_label(info.summary.file_bytes),
                ),
                (
                    &i18n.text("about.image.summary.color_type"),
                    info.summary.color_type.clone(),
                ),
                (
                    &i18n.text("about.image.summary.channels"),
                    info.summary.channel_count.to_string(),
                ),
                (
                    &i18n.text("about.image.summary.bit_depth"),
                    info.summary
                        .bit_depth
                        .map(|depth| format!("{depth}-bit"))
                        .unwrap_or_else(|| unknown(i18n)),
                ),
                (
                    &i18n.text("about.image.summary.bits_per_pixel"),
                    format!("{} bpp", info.summary.bits_per_pixel),
                ),
                (
                    &i18n.text("about.image.summary.alpha"),
                    yes_no(info.summary.has_alpha, i18n),
                ),
                (
                    &i18n.text("about.image.summary.animation"),
                    info.summary
                        .animation
                        .clone()
                        .unwrap_or_else(|| i18n.text("state.none")),
                ),
            ],
        );
    });
}

fn show_exif_card(ui: &mut egui::Ui, exif: &ExifInfo, i18n: I18n) {
    info_card(ui, "EXIF", |ui| {
        info_grid(
            ui,
            "image_exif_grid",
            &[
                ("Orientation", value_or_unknown(&exif.orientation, i18n)),
                (
                    &i18n.text("about.image.exif.captured_at"),
                    value_or_unknown(&exif.captured_at, i18n),
                ),
                (
                    &i18n.text("about.image.exif.camera_make"),
                    value_or_unknown(&exif.camera_make, i18n),
                ),
                (
                    &i18n.text("about.image.exif.camera_model"),
                    value_or_unknown(&exif.camera_model, i18n),
                ),
                (
                    &i18n.text("about.image.exif.lens"),
                    value_or_unknown(&exif.lens_model, i18n),
                ),
                ("ISO", value_or_unknown(&exif.iso, i18n)),
                (
                    &i18n.text("about.image.exif.shutter"),
                    value_or_unknown(&exif.exposure_time, i18n),
                ),
                (
                    &i18n.text("about.image.exif.aperture"),
                    value_or_unknown(&exif.f_number, i18n),
                ),
                (
                    &i18n.text("about.image.exif.focal_length"),
                    value_or_unknown(&exif.focal_length, i18n),
                ),
                (
                    &i18n.text("about.image.exif.exposure_bias"),
                    value_or_unknown(&exif.exposure_bias, i18n),
                ),
                (
                    &i18n.text("about.image.exif.flash"),
                    value_or_unknown(&exif.flash, i18n),
                ),
            ],
        );
    });
}

fn show_color_card(ui: &mut egui::Ui, color: &ColorProfileInfo, app: &SuiSuiViewApp, i18n: I18n) {
    let icc_state = match color.icc_profile_bytes {
        Some(bytes) => i18n.with_vars(
            "about.image.color.included",
            &[("size", bytes_label(bytes))],
        ),
        None => i18n.text("state.none"),
    };
    let icc_read_state = match (color.icc_profile_bytes, color.icc_profile_error.as_ref()) {
        (Some(_), Some(error)) => error.clone(),
        (Some(_), None) => i18n.text("about.image.color.ok"),
        (None, _) => "-".to_owned(),
    };
    info_card(ui, &i18n.text("about.image.color.title"), |ui| {
        info_grid(
            ui,
            "image_color_grid",
            &[
                (&i18n.text("about.image.color.icc_profile"), icc_state),
                (
                    &i18n.text("about.image.color.icc_name"),
                    value_or_unknown(&color.icc_profile_name, i18n),
                ),
                (&i18n.text("about.image.color.icc_state"), icc_read_state),
                (
                    &i18n.text("about.image.color.png_color_type"),
                    value_or_unknown(&color.png_color_type, i18n),
                ),
                (
                    &i18n.text("about.image.color.png_bit_depth"),
                    color
                        .png_bit_depth
                        .map(|depth| format!("{depth}-bit"))
                        .unwrap_or_else(|| unknown(i18n)),
                ),
                ("sRGB", value_or_unknown(&color.png_srgb, i18n)),
                ("Gamma", value_or_unknown(&color.png_gamma, i18n)),
                (
                    "Chromaticity",
                    value_or_unknown(&color.png_chromaticities, i18n),
                ),
                ("Density", value_or_unknown(&color.png_density, i18n)),
                (
                    &i18n.text("about.image.color.icc_setting"),
                    on_off(app.settings.apply_embedded_icc, i18n),
                ),
            ],
        );
    });
}

fn show_detail_tags_card(
    ui: &mut egui::Ui,
    tags: &[ImageExifTag],
    gps_tags: &[ImageExifTag],
    i18n: I18n,
) {
    info_card(ui, &i18n.text("about.image.tags.title"), |ui| {
        if tags.is_empty() && gps_tags.is_empty() {
            ui.label(i18n.text("about.image.tags.empty_exif"));
            return;
        }

        egui::CollapsingHeader::new(i18n.text("about.image.tags.all_exif"))
            .default_open(false)
            .show(ui, |ui| show_tag_grid(ui, "all_exif_tags", tags, i18n));

        egui::CollapsingHeader::new(i18n.text("about.image.tags.gps"))
            .default_open(false)
            .show(ui, |ui| {
                if gps_tags.is_empty() {
                    ui.label(i18n.text("about.image.tags.empty_gps"));
                } else {
                    show_tag_grid(ui, "gps_exif_tags", gps_tags, i18n);
                }
            });
    });
}

fn show_file_location_card(ui: &mut egui::Ui, app: &SuiSuiViewApp, info: &ImageInfo, i18n: I18n) {
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

    info_card(ui, &i18n.text("about.image.file.title"), |ui| {
        info_grid(
            ui,
            "image_file_grid",
            &[
                (&i18n.text("about.image.file.name"), file_name.to_owned()),
                (&i18n.text("about.image.file.full_path"), display_path),
                (
                    &i18n.text("about.image.file.book"),
                    source.title().to_owned(),
                ),
                (
                    &i18n.text("about.image.file.source"),
                    source.source_path().display().to_string(),
                ),
                (
                    &i18n.text("about.image.file.archive_path"),
                    if page_file_path.is_some() {
                        "-".to_owned()
                    } else {
                        value_or_dash(page_name)
                    },
                ),
                (
                    &i18n.text("about.image.file.current_page"),
                    format!("{} / {}", app.current_page + 1, source.page_count()),
                ),
                (
                    &i18n.text("about.image.file.source_bytes"),
                    bytes_label(info.summary.file_bytes),
                ),
            ],
        );
    });
}

fn show_view_state_card(ui: &mut egui::Ui, app: &SuiSuiViewApp, i18n: I18n) {
    let transform = app.effects.transform;
    info_card(ui, &i18n.text("about.image.view.title"), |ui| {
        info_grid(
            ui,
            "image_view_state_grid",
            &[
                (
                    &i18n.text("about.image.view.mode"),
                    view_mode_label(app.view_mode, i18n),
                ),
                (
                    &i18n.text("about.image.view.fit"),
                    app.fit_mode.label_i18n(i18n),
                ),
                (
                    &i18n.text("about.image.view.reading"),
                    app.reading_direction.label_i18n(i18n),
                ),
                (
                    &i18n.text("about.image.view.rotation"),
                    format!("{}°", u16::from(transform.rotation_quadrants % 4) * 90),
                ),
                (
                    &i18n.text("about.image.view.flip_h"),
                    on_off(transform.flip_horizontal, i18n),
                ),
                (
                    &i18n.text("about.image.view.flip_v"),
                    on_off(transform.flip_vertical, i18n),
                ),
                (
                    &i18n.text("about.image.view.invert"),
                    on_off(app.effects.invert_colors, i18n),
                ),
                (
                    &i18n.text("about.image.view.filter"),
                    app.effects.filter.label_i18n(i18n),
                ),
                (
                    &i18n.text("about.image.view.exif_orientation"),
                    on_off(app.settings.apply_exif_orientation, i18n),
                ),
                (
                    &i18n.text("about.image.view.decode_mode"),
                    actual_decode_label(app, i18n),
                ),
                (
                    &i18n.text("about.image.view.resize_filter"),
                    actual_scaler_filter_label(app, i18n),
                ),
                (&i18n.text("about.image.view.ai"), ai_state_label(app, i18n)),
            ],
        );
    });
}

fn show_app_info(ui: &mut egui::Ui, i18n: I18n) {
    info_card(ui, "SuiSuiView", |ui| {
        ui.label(
            RichText::new(concat!("SuiSuiView ", env!("CARGO_PKG_VERSION")))
                .size(20.0)
                .strong()
                .color(theme::TEXT_PRIMARY),
        );
        ui.add_space(4.0);
        ui.label(i18n.text("about.app.tagline"));
    });

    info_card(ui, &i18n.text("about.app.license.title"), |ui| {
        info_grid(
            ui,
            "about_app_grid",
            &[
                (
                    &i18n.text("about.app.license"),
                    env!("CARGO_PKG_LICENSE").to_owned(),
                ),
                (
                    &i18n.text("about.app.source_code"),
                    i18n.text("about.app.source_code.value"),
                ),
                (
                    &i18n.text("about.app.distribution"),
                    i18n.text("about.app.distribution.value"),
                ),
                (
                    &i18n.text("about.app.notices"),
                    i18n.text("about.app.notices.value"),
                ),
            ],
        );
    });
}

fn show_notices(ui: &mut egui::Ui, i18n: I18n) {
    info_card(ui, &i18n.text("about.section.notices"), |ui| {
        ui.label(i18n.text("about.notices.intro"));
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
    let value_width = (ui.available_width() - 132.0).max(160.0);
    egui::Grid::new(id)
        .num_columns(2)
        .spacing([16.0, 8.0])
        .striped(false)
        .show(ui, |ui| {
            for (label, value) in rows {
                ui.label(RichText::new(*label).color(theme::TEXT_MUTED));
                wrapped_selectable_label(ui, value, value_width);
                ui.end_row();
            }
        });
}

fn show_tag_grid(ui: &mut egui::Ui, id: &'static str, tags: &[ImageExifTag], i18n: I18n) {
    if tags.is_empty() {
        ui.label(i18n.text("about.image.tags.empty"));
        return;
    }

    let value_width = (ui.available_width() - 220.0).max(160.0);
    egui::Grid::new(id)
        .num_columns(3)
        .spacing([12.0, 6.0])
        .striped(true)
        .show(ui, |ui| {
            for tag in tags {
                ui.label(RichText::new(&tag.ifd).color(theme::TEXT_MUTED));
                ui.label(&tag.tag);
                truncated_selectable_label(ui, &tag.value, value_width);
                ui.end_row();
            }
        });
}

fn wrapped_selectable_label(ui: &mut egui::Ui, value: impl AsRef<str>, max_width: f32) {
    ui.scope(|ui| {
        ui.set_max_width(max_width);
        ui.add(egui::Label::new(value.as_ref()).selectable(true).wrap());
    });
}

fn truncated_selectable_label(ui: &mut egui::Ui, value: &str, max_width: f32) {
    ui.scope(|ui| {
        ui.set_max_width(max_width);
        ui.add(egui::Label::new(value).selectable(true).truncate())
            .on_hover_text(value);
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

fn value_or_unknown(value: &Option<String>, i18n: I18n) -> String {
    value.clone().unwrap_or_else(|| unknown(i18n))
}

fn value_or_dash(value: &str) -> String {
    if value.trim().is_empty() {
        "-".to_owned()
    } else {
        value.to_owned()
    }
}

fn unknown(i18n: I18n) -> String {
    i18n.text("state.unknown")
}

fn yes_no(value: bool, i18n: I18n) -> String {
    if value {
        i18n.text("state.yes")
    } else {
        i18n.text("state.no")
    }
}

fn on_off(value: bool, i18n: I18n) -> String {
    if value {
        i18n.text("state.on")
    } else {
        i18n.text("state.off")
    }
}

fn view_mode_label(mode: ViewMode, i18n: I18n) -> String {
    match mode {
        ViewMode::Single => i18n.text("label.view.single"),
        ViewMode::DoubleLeftToRight => i18n.text("label.view.double_ltr"),
        ViewMode::DoubleRightToLeft => i18n.text("label.view.double_rtl"),
        ViewMode::SmartDoubleLeftToRight => i18n.text("label.view.smart_ltr"),
        ViewMode::SmartDoubleRightToLeft => i18n.text("label.view.smart_rtl"),
    }
}

fn actual_decode_label(app: &SuiSuiViewApp, i18n: I18n) -> String {
    app.current_view_state
        .as_ref()
        .map(|state| state.decode_backend.label().to_owned())
        .unwrap_or_else(|| unknown(i18n))
}

fn actual_scaler_filter_label(app: &SuiSuiViewApp, i18n: I18n) -> String {
    let Some(state) = app.current_view_state.as_ref() else {
        return unknown(i18n);
    };
    format!(
        "Prepare: {} / Display: {}",
        state.cpu_scale.label(),
        state.wgpu_scale.label()
    )
}

fn ai_state_label(app: &SuiSuiViewApp, i18n: I18n) -> String {
    match app.settings.ai_upscale.backend {
        AiUpscaleBackend::Off => AiUpscaleBackend::Off.label_i18n(i18n),
        AiUpscaleBackend::RealEsrganNcnn
            if app
                .current_view_state
                .as_ref()
                .is_some_and(|state| state.upscaled) =>
        {
            i18n.text("about.image.ai.on_display")
        }
        AiUpscaleBackend::RealEsrganNcnn => i18n.text("about.image.ai.on_original"),
    }
}
