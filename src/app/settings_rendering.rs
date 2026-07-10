use super::fast_start::{self, FastStartReportAction};
use super::settings::{checkbox_with_help, grid_label_with_help, setting_group};
use super::ui::theme;
use crate::core::deband::DebandStrength;
use crate::core::i18n::I18n;
use crate::core::state::{
    AppSettings, CpuScaleFilter, RefineUpscaler, RendererMode, WgpuUpscaleMethod,
};
use egui::{self, RichText};

// established call surface; a params struct would be pure boilerplate
#[allow(clippy::too_many_arguments)]
pub(in crate::app) fn show_rendering_settings(
    ui: &mut egui::Ui,
    draft: &mut AppSettings,
    pending_gpu_acceleration: &mut Option<bool>,
    fast_start_failure_notice: Option<&crate::core::state::FastStartFailureNotice>,
    fast_start_action: &mut Option<FastStartReportAction>,
    changed: &mut bool,
    i18n: I18n,
) {
    let mut gpu_enabled = matches!(draft.renderer_mode, RendererMode::Wgpu);

    // [렌더러·GPU 가속] — GPU acceleration toggle, split out of the scaling group.
    // The restart-confirm dialog machinery lives in settings.rs and is untouched.
    setting_group(
        ui,
        &i18n.text("settings.rendering.renderer.title"),
        &i18n.text("settings.rendering.renderer.desc"),
        |ui| {
            egui::Grid::new("settings_gpu_toggle_grid")
                .num_columns(2)
                .spacing([14.0, 8.0])
                .show(ui, |ui| {
                    let gpu_help = i18n.text("settings.rendering.gpu.help");
                    grid_label_with_help(ui, &i18n.text("settings.rendering.gpu"), &gpu_help);
                    let gpu_changed = ui
                        .checkbox(&mut gpu_enabled, "")
                        .on_hover_text(gpu_help)
                        .changed();
                    if gpu_changed {
                        *pending_gpu_acceleration = Some(gpu_enabled);
                        ui.ctx().request_repaint();
                        gpu_enabled = matches!(draft.renderer_mode, RendererMode::Wgpu);
                    }
                    ui.end_row();
                });
            fast_start::show_settings_status(
                ui,
                fast_start_failure_notice,
                fast_start_action,
                i18n,
            );
        },
    );

    ui.add_space(8.0);
    // [스케일링] — CPU/GPU scaling controls (the rest of the former upscaler group).
    setting_group(
        ui,
        &i18n.text("settings.rendering.upscaler.title"),
        &i18n.text("settings.rendering.upscaler.desc"),
        |ui| {
            egui::Grid::new("settings_scaler_grid")
                .num_columns(2)
                .spacing([14.0, 8.0])
                .show(ui, |ui| {
                    let fast_decode_help =
                        i18n.text("settings.rendering.fast_sampled_scaled_decode.help");
                    grid_label_with_help(
                        ui,
                        &i18n.text("settings.rendering.fast_sampled_scaled_decode"),
                        &fast_decode_help,
                    );
                    *changed |= ui
                        .checkbox(&mut draft.fast_sampled_scaled_decode, "")
                        .on_hover_text(fast_decode_help)
                        .changed();
                    ui.end_row();

                    grid_label_with_help(
                        ui,
                        &i18n.text("settings.rendering.cpu_upscale_filter"),
                        &i18n.text("settings.rendering.cpu_upscale_filter.help"),
                    );
                    egui::ComboBox::from_id_salt("cpu_upscale_filter")
                        .selected_text(draft.cpu_upscale_filter.label())
                        .show_ui(ui, |ui| {
                            for filter in CpuScaleFilter::ALL {
                                *changed |= ui
                                    .selectable_value(
                                        &mut draft.cpu_upscale_filter,
                                        filter,
                                        filter.label(),
                                    )
                                    .changed();
                            }
                        });
                    ui.end_row();
                });

            ui.add_space(10.0);
            ui.separator();
            ui.add_space(10.0);

            egui::Grid::new("settings_gpu_upscaler_grid")
                .num_columns(2)
                .spacing([14.0, 8.0])
                .show(ui, |ui| {
                    grid_label_with_help(
                        ui,
                        &i18n.text("settings.rendering.gpu_upscaler"),
                        &i18n.text("settings.rendering.gpu_upscaler.help"),
                    );
                    let mut selected_upscaler =
                        if gpu_enabled && draft.wgpu_upscale_method == WgpuUpscaleMethod::None {
                            WgpuUpscaleMethod::Auto
                        } else {
                            draft.wgpu_upscale_method
                        };
                    let mut selected_upscaler_changed = false;
                    let upscaler_response = ui
                        .add_enabled_ui(gpu_enabled, |ui| {
                            egui::ComboBox::from_id_salt("wgpu_upscale_method")
                                .selected_text(selected_upscaler.settings_label_i18n(i18n))
                                .show_ui(ui, |ui| {
                                    for upscaler in WgpuUpscaleMethod::SETTINGS_CHOICES {
                                        if upscaler == WgpuUpscaleMethod::None {
                                            continue;
                                        }
                                        let option_response = ui.selectable_value(
                                            &mut selected_upscaler,
                                            upscaler,
                                            upscaler.settings_label_i18n(i18n),
                                        );
                                        selected_upscaler_changed |= option_response.changed();
                                        if upscaler == WgpuUpscaleMethod::WgslSrLabSpanX2 {
                                            option_response.on_hover_text(
                                                i18n.text("settings.rendering.slow_span.help"),
                                            );
                                        } else if upscaler.experimental_selectable() {
                                            option_response.on_hover_text(i18n.text(
                                                "settings.rendering.experimental_upscaler.help",
                                            ));
                                        }
                                    }
                                });
                        })
                        .response;
                    if gpu_enabled {
                        upscaler_response
                            .on_hover_text(i18n.text("settings.rendering.gpu_upscaler.help"));
                    } else {
                        upscaler_response.on_disabled_hover_text(
                            i18n.text("settings.rendering.gpu_upscaler.disabled"),
                        );
                    }
                    if selected_upscaler_changed {
                        draft.wgpu_upscale_method = selected_upscaler;
                        *changed = true;
                    }
                    ui.end_row();

                    let refine_help = i18n.text("settings.rendering.refine_upscaler.help");
                    grid_label_with_help(
                        ui,
                        &i18n.text("settings.rendering.refine_upscaler"),
                        &refine_help,
                    );
                    let refine_response = ui
                        .add_enabled_ui(gpu_enabled, |ui| {
                            egui::ComboBox::from_id_salt("refine_upscaler")
                                .selected_text(draft.refine_upscaler.label_i18n(i18n))
                                .show_ui(ui, |ui| {
                                    for tier in RefineUpscaler::ALL {
                                        *changed |= ui
                                            .selectable_value(
                                                &mut draft.refine_upscaler,
                                                tier,
                                                tier.label_i18n(i18n),
                                            )
                                            .changed();
                                    }
                                });
                        })
                        .response;
                    if gpu_enabled {
                        refine_response.on_hover_text(refine_help);
                    } else {
                        refine_response.on_disabled_hover_text(
                            i18n.text("settings.rendering.gpu_upscaler.disabled"),
                        );
                    }
                    ui.end_row();

                    grid_label_with_help(
                        ui,
                        &i18n.text("settings.rendering.fixed_2x_min_scale"),
                        &i18n.text("settings.rendering.fixed_2x_min_scale.help"),
                    );
                    ui.add_enabled_ui(gpu_enabled, |ui| {
                        *changed |= ui
                            .add(
                                egui::DragValue::new(&mut draft.fixed_2x_sr_min_scale_pct)
                                    .range(100..=200)
                                    .speed(1)
                                    .suffix("%"),
                            )
                            .changed();
                    });
                    ui.end_row();

                    let deband_help = i18n.text("settings.rendering.deband.help");
                    grid_label_with_help(ui, &i18n.text("settings.rendering.deband"), &deband_help);
                    let deband_response = ui
                        .add_enabled_ui(gpu_enabled, |ui| {
                            egui::ComboBox::from_id_salt("deband_strength")
                                .selected_text(draft.deband.label_i18n(i18n))
                                .show_ui(ui, |ui| {
                                    for level in DebandStrength::ALL {
                                        *changed |= ui
                                            .selectable_value(
                                                &mut draft.deband,
                                                level,
                                                level.label_i18n(i18n),
                                            )
                                            .changed();
                                    }
                                });
                        })
                        .response;
                    if gpu_enabled {
                        deband_response.on_hover_text(deband_help);
                    } else {
                        deband_response.on_disabled_hover_text(
                            i18n.text("settings.rendering.gpu_upscaler.disabled"),
                        );
                    }
                    ui.end_row();

                    let linear_help = i18n.text("settings.rendering.linear_downscale.help");
                    grid_label_with_help(
                        ui,
                        &i18n.text("settings.rendering.linear_downscale"),
                        &linear_help,
                    );
                    let linear_response = ui
                        .add_enabled_ui(gpu_enabled, |ui| {
                            *changed |=
                                ui.checkbox(&mut draft.linear_light_downscale, "").changed();
                        })
                        .response;
                    if gpu_enabled {
                        linear_response.on_hover_text(linear_help);
                    } else {
                        linear_response.on_disabled_hover_text(
                            i18n.text("settings.rendering.gpu_upscaler.disabled"),
                        );
                    }
                    ui.end_row();
                });
            ui.add_space(4.0);
            ui.label(
                RichText::new(i18n.text("settings.rendering.gpu_off_note"))
                    .size(12.0)
                    .color(theme::TEXT_MUTED),
            );
        },
    );

    ui.add_space(8.0);
    setting_group(
        ui,
        &i18n.text("settings.rendering.image_info.title"),
        &i18n.text("settings.rendering.image_info.desc"),
        |ui| {
            *changed |= checkbox_with_help(
                ui,
                &mut draft.apply_exif_orientation,
                &i18n.text("settings.rendering.exif_orientation"),
                &i18n.text("settings.rendering.exif_orientation.help"),
            );
            *changed |= checkbox_with_help(
                ui,
                &mut draft.apply_embedded_icc,
                &i18n.text("settings.rendering.icc"),
                &i18n.text("settings.rendering.icc.help"),
            );
            if draft.apply_embedded_icc {
                ui.add_space(4.0);
                ui.label(
                    RichText::new(i18n.text("settings.rendering.icc.note"))
                        .size(12.0)
                        .color(theme::TEXT_MUTED),
                );
            }
        },
    );
}
