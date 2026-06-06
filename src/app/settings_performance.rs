use super::cache_budget_bytes;
use super::settings::{checkbox_with_help, grid_label_with_help, info_icon, setting_group};
use super::ui::theme;
use crate::core::i18n::I18n;
use crate::core::state::{
    AppSettings, CacheMemoryMode, DecodeMode, DecoderPreference, MANUAL_CACHE_MB_MAX,
    MANUAL_CACHE_MB_MIN,
};
use eframe::egui::{self, RichText};

const JPEG_DECODER_OPTIONS: &[DecoderPreference] = &[
    DecoderPreference::Default,
    DecoderPreference::ImageCrate,
    DecoderPreference::ZuneJpeg,
];
const PNG_DECODER_OPTIONS: &[DecoderPreference] = &[
    DecoderPreference::Default,
    DecoderPreference::ImageCrate,
    DecoderPreference::PngCrate,
    DecoderPreference::ZunePng,
];
const WEBP_DECODER_OPTIONS: &[DecoderPreference] = &[
    DecoderPreference::Default,
    DecoderPreference::ImageCrate,
    DecoderPreference::ImageWebp,
    DecoderPreference::LibWebp,
];
const GIF_DECODER_OPTIONS: &[DecoderPreference] = &[
    DecoderPreference::Default,
    DecoderPreference::ImageCrate,
    DecoderPreference::GifCrate,
];
const BMP_DECODER_OPTIONS: &[DecoderPreference] = &[
    DecoderPreference::Default,
    DecoderPreference::ImageCrate,
    DecoderPreference::BmpFastPath,
];
const ICO_DECODER_OPTIONS: &[DecoderPreference] = &[
    DecoderPreference::Default,
    DecoderPreference::ImageCrate,
    DecoderPreference::IcoFastPath,
];
const AVIF_DECODER_OPTIONS: &[DecoderPreference] =
    &[DecoderPreference::Default, DecoderPreference::LibAvifDav1d];
const SVG_DECODER_OPTIONS: &[DecoderPreference] =
    &[DecoderPreference::Default, DecoderPreference::Resvg];
const PSD_DECODER_OPTIONS: &[DecoderPreference] =
    &[DecoderPreference::Default, DecoderPreference::ZunePsd];
const AI_DECODER_OPTIONS: &[DecoderPreference] =
    &[DecoderPreference::Default, DecoderPreference::PdfiumAi];

pub(in crate::app) fn show_decoder_settings(
    ui: &mut egui::Ui,
    draft: &mut AppSettings,
    changed: &mut bool,
    i18n: I18n,
) {
    setting_group(
        ui,
        &i18n.text("settings.decoder.mode.title"),
        &i18n.text("settings.decoder.mode.desc"),
        |ui| {
            egui::Grid::new("settings_decoder_mode_grid")
                .num_columns(2)
                .spacing([14.0, 8.0])
                .show(ui, |ui| {
                    grid_label_with_help(
                        ui,
                        &i18n.text("settings.decoder.mode.title"),
                        &i18n.text("settings.decoder.mode.help"),
                    );
                    egui::ComboBox::from_id_salt("decode_mode")
                        .selected_text(draft.decode_mode.label_i18n(i18n))
                        .show_ui(ui, |ui| {
                            for mode in DecodeMode::ALL {
                                *changed |= ui
                                    .selectable_value(
                                        &mut draft.decode_mode,
                                        mode,
                                        mode.label_i18n(i18n),
                                    )
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
        &i18n.text("settings.decoder.by_format.title"),
        &i18n.text("settings.decoder.by_format.desc"),
        |ui| {
            let enabled = draft.decode_mode == DecodeMode::Custom;
            let mode_help = (!enabled).then(|| i18n.text("settings.decoder.custom_only"));
            let avif_enabled = enabled && cfg!(feature = "native-avif");
            let avif_help = if cfg!(feature = "native-avif") {
                mode_help.clone()
            } else {
                Some(i18n.text("settings.decoder.native_avif_only"))
            };
            let ai_enabled = enabled && cfg!(feature = "native-ai");
            let ai_help = if cfg!(feature = "native-ai") {
                mode_help.clone()
            } else {
                Some(i18n.text("settings.decoder.native_ai_only"))
            };

            egui::Grid::new("settings_decoder_preferences_grid")
                .num_columns(2)
                .spacing([14.0, 8.0])
                .show(ui, |ui| {
                    decoder_row(
                        ui,
                        changed,
                        enabled,
                        "JPEG",
                        mode_help.clone(),
                        &mut draft.decoder_preferences.jpeg,
                        JPEG_DECODER_OPTIONS,
                        i18n,
                    );
                    decoder_row(
                        ui,
                        changed,
                        enabled,
                        "PNG",
                        mode_help.clone(),
                        &mut draft.decoder_preferences.png,
                        PNG_DECODER_OPTIONS,
                        i18n,
                    );
                    decoder_row(
                        ui,
                        changed,
                        enabled,
                        "WebP",
                        mode_help.clone(),
                        &mut draft.decoder_preferences.webp,
                        WEBP_DECODER_OPTIONS,
                        i18n,
                    );
                    decoder_row(
                        ui,
                        changed,
                        enabled,
                        "GIF",
                        mode_help.clone(),
                        &mut draft.decoder_preferences.gif,
                        GIF_DECODER_OPTIONS,
                        i18n,
                    );
                    decoder_row(
                        ui,
                        changed,
                        enabled,
                        "BMP",
                        mode_help.clone(),
                        &mut draft.decoder_preferences.bmp,
                        BMP_DECODER_OPTIONS,
                        i18n,
                    );
                    decoder_row(
                        ui,
                        changed,
                        enabled,
                        "ICO",
                        mode_help.clone(),
                        &mut draft.decoder_preferences.ico,
                        ICO_DECODER_OPTIONS,
                        i18n,
                    );
                    decoder_row(
                        ui,
                        changed,
                        avif_enabled,
                        "AVIF",
                        avif_help,
                        &mut draft.decoder_preferences.avif,
                        AVIF_DECODER_OPTIONS,
                        i18n,
                    );
                    decoder_row(
                        ui,
                        changed,
                        false,
                        "SVG",
                        Some(i18n.text("settings.decoder.planned")),
                        &mut draft.decoder_preferences.svg,
                        SVG_DECODER_OPTIONS,
                        i18n,
                    );
                    decoder_row(
                        ui,
                        changed,
                        enabled,
                        "PSD",
                        mode_help.clone(),
                        &mut draft.decoder_preferences.psd,
                        PSD_DECODER_OPTIONS,
                        i18n,
                    );
                    decoder_row(
                        ui,
                        changed,
                        ai_enabled,
                        "AI",
                        ai_help,
                        &mut draft.decoder_preferences.ai,
                        AI_DECODER_OPTIONS,
                        i18n,
                    );
                });

            if draft.decode_mode == DecodeMode::Compatibility {
                ui.add_space(4.0);
                ui.label(
                    RichText::new(i18n.text("settings.decoder.compat_note"))
                        .size(12.0)
                        .color(theme::TEXT_MUTED),
                );
            } else if draft.decode_mode == DecodeMode::AutoFast {
                ui.add_space(4.0);
                ui.label(
                    RichText::new(i18n.text("settings.decoder.auto_note"))
                        .size(12.0)
                        .color(theme::TEXT_MUTED),
                );
            }
        },
    );
}

pub(in crate::app) fn show_performance_settings(
    ui: &mut egui::Ui,
    draft: &mut AppSettings,
    changed: &mut bool,
    i18n: I18n,
) {
    setting_group(
        ui,
        &i18n.text("settings.performance.prefetch.title"),
        &i18n.text("settings.performance.prefetch.desc"),
        |ui| {
            *changed |= checkbox_with_help(
                ui,
                &mut draft.prefetch_enabled,
                &i18n.text("settings.performance.prefetch_pages"),
                &i18n.text("settings.performance.prefetch_pages.help"),
            );
            *changed |= checkbox_with_help(
                ui,
                &mut draft.progressive_preview_enabled,
                &i18n.text("settings.performance.preview"),
                &i18n.text("settings.performance.preview.help"),
            );
        },
    );

    ui.add_space(8.0);
    setting_group(
        ui,
        &i18n.text("settings.performance.memory.title"),
        &i18n.text("settings.performance.memory.desc"),
        |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(i18n.text("settings.performance.page_cache"));
                info_icon(ui, &i18n.text("settings.performance.page_cache.help"));
                *changed |= ui
                    .radio_value(
                        &mut draft.cache_memory_mode,
                        CacheMemoryMode::Auto,
                        CacheMemoryMode::Auto.label_i18n(i18n),
                    )
                    .changed();
                *changed |= ui
                    .radio_value(
                        &mut draft.cache_memory_mode,
                        CacheMemoryMode::Manual,
                        CacheMemoryMode::Manual.label_i18n(i18n),
                    )
                    .changed();
                ui.add_enabled_ui(draft.cache_memory_mode == CacheMemoryMode::Manual, |ui| {
                    *changed |= ui
                        .add(
                            egui::DragValue::new(&mut draft.manual_cache_mb)
                                .range(MANUAL_CACHE_MB_MIN..=MANUAL_CACHE_MB_MAX)
                                .speed(16)
                                .suffix(" MB"),
                        )
                        .changed();
                });
            });

            ui.add_space(4.0);
            ui.label(
                RichText::new(i18n.with_vars(
                    "settings.performance.cache_summary",
                    &[
                        ("mode", draft.cache_memory_mode.label_i18n(i18n)),
                        ("cache", format!("{:.0}", mib(cache_budget_bytes(draft)))),
                    ],
                ))
                .size(12.0)
                .color(theme::TEXT_MUTED),
            );
        },
    );
}

fn decoder_row(
    ui: &mut egui::Ui,
    changed: &mut bool,
    enabled: bool,
    format: &str,
    disabled_help: Option<String>,
    value: &mut DecoderPreference,
    options: &[DecoderPreference],
    i18n: I18n,
) {
    ui.label(format);
    let response = ui
        .add_enabled_ui(enabled, |ui| {
            egui::ComboBox::from_id_salt(("decoder_preference", format))
                .selected_text(value.label_i18n(i18n))
                .show_ui(ui, |ui| {
                    for option in options {
                        *changed |= ui
                            .selectable_value(value, *option, option.label_i18n(i18n))
                            .changed();
                    }
                });
        })
        .response;
    if !enabled {
        if let Some(help) = disabled_help {
            response.on_disabled_hover_text(help);
        }
    }
    ui.end_row();
}

fn mib(bytes: usize) -> f32 {
    bytes as f32 / (1024.0 * 1024.0)
}
