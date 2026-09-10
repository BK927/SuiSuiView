//! Settings-only browsing of the existing upscalers. Selection and Auto policy
//! continue to belong to the existing display state and renderer.
use crate::core::i18n::{I18n, ResolvedLanguage};
use crate::core::state::WgpuUpscaleMethod;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum CatalogView {
    #[default]
    Recommended,
    All,
    Experimental,
}

pub(super) fn show(ui: &mut egui::Ui, selected: &mut WgpuUpscaleMethod, i18n: I18n) -> bool {
    let id = ui.make_persistent_id("upscaler-catalog-view");
    let mut view = ui
        .ctx()
        .data(|data| data.get_temp::<CatalogView>(id).unwrap_or_default());
    ui.horizontal(|ui| {
        for (candidate, ko, en) in [
            (CatalogView::Recommended, "추천", "Recommended"),
            (CatalogView::All, "전체", "All"),
            (CatalogView::Experimental, "실험", "Experimental"),
        ] {
            ui.selectable_value(&mut view, candidate, text(i18n, ko, en));
        }
    });
    ui.ctx().data_mut(|data| data.insert_temp(id, view));
    ui.label(
        egui::RichText::new(text(
            i18n,
            "추천은 자주 쓰는 방식입니다. 전체에서 모든 계열을 비교해 보세요.",
            "Start with the common choices, or browse All to compare every family.",
        ))
        .small(),
    );
    ui.separator();
    let mut changed = false;
    for family in ["Basic", "Anime4K", "ACNet", "CuNNy", "ArtCNN", "SPAN"] {
        let choices: Vec<_> = WgpuUpscaleMethod::SETTINGS_CHOICES
            .into_iter()
            .filter(|method| *method != WgpuUpscaleMethod::None)
            .filter(|method| family_of(*method) == family)
            .filter(|method| match view {
                CatalogView::Recommended => recommended(*method),
                CatalogView::All => true,
                CatalogView::Experimental => method.experimental_selectable(),
            })
            .collect();
        if choices.is_empty() {
            continue;
        }
        ui.label(
            egui::RichText::new(if family == "Basic" {
                text(i18n, "기본", "Basic")
            } else {
                family
            })
            .strong(),
        );
        for method in choices {
            let unavailable = method == WgpuUpscaleMethod::WgslSrLabSpanX2 && !span_configured();
            let response = ui
                .add_enabled_ui(!unavailable, |ui| {
                    ui.selectable_value(selected, method, method.settings_label_i18n(i18n))
                })
                .inner;
            changed |= response.changed();
            if unavailable {
                response.on_disabled_hover_text(text(i18n,
                    "SPAN 모델 구성이 없습니다. 보유한 모델을 연결한 뒤 사용할 수 있습니다. 모델을 자동으로 다운로드하지 않습니다.",
                    "No SPAN model is configured. Connect your existing model to use this option. Models are not downloaded automatically."));
                ui.label(
                    egui::RichText::new(text(
                        i18n,
                        "모델 연결 필요",
                        "Model configuration required",
                    ))
                    .small()
                    .weak(),
                );
            } else if method == WgpuUpscaleMethod::WgslSrLabSpanX2 {
                response.on_hover_text(i18n.text("settings.rendering.slow_span.help"));
            } else if method.experimental_selectable() {
                response.on_hover_text(i18n.text("settings.rendering.experimental_upscaler.help"));
            } else {
                response.on_hover_text(match family {
                    "Anime4K" => text(i18n, "선화용 2배 보정. S는 작은 모델, M은 더 큰 모델입니다.", "2× line-art enhancement. S is the smaller model; M is larger."),
                    "ACNet" => text(i18n, "휘도 중심 2배 보정. Box와 HDN 변형은 질감과 잡티 처리에 차이가 있습니다.", "Luma-focused 2× enhancement. Box and HDN variants differ in texture and artifact treatment."),
                    "CuNNy" => text(i18n, "선화용 2배 보정. 모델 크기와 변형에 따라 속도, 선명도, 질감이 달라집니다.", "2× line-art enhancement. Model sizes and variants trade processing cost against edges and texture."),
                    _ => text(i18n, "선택한 처리 방식을 사용합니다. 자동의 기존 판단 기준은 바뀌지 않습니다.", "Uses the selected processing method. Auto keeps its existing decision rules."),
                });
            }
        }
        ui.add_space(4.0);
    }
    changed
}

fn recommended(method: WgpuUpscaleMethod) -> bool {
    matches!(
        method,
        WgpuUpscaleMethod::Auto
            | WgpuUpscaleMethod::WgslFsr1EasuRcas
            | WgpuUpscaleMethod::WgslAnime4kV32CnnX2M
    )
}

fn family_of(method: WgpuUpscaleMethod) -> &'static str {
    match method {
        WgpuUpscaleMethod::WgslAnime4kV32CnnX2S | WgpuUpscaleMethod::WgslAnime4kV32CnnX2M => {
            "Anime4K"
        }
        WgpuUpscaleMethod::WgslAcnetF8B4Luma
        | WgpuUpscaleMethod::WgslAcnetF8B4BoxLuma
        | WgpuUpscaleMethod::WgslAcnetF8B4HdnLuma
        | WgpuUpscaleMethod::WgslAcnetF8B4BoxHdnLuma => "ACNet",
        WgpuUpscaleMethod::WgslSrLabSpanX2 => "SPAN",
        _ if method.token().contains("cunny") => "CuNNy",
        _ if method.token().contains("artcnn") => "ArtCNN",
        _ => "Basic",
    }
}

fn span_configured() -> bool {
    [
        "SUISUIVIEW_EXPERIMENT_SPAN_MANIFEST",
        "SUISUIVIEW_SR_LAB_SPAN_MANIFEST",
    ]
    .iter()
    .any(|name| std::env::var(name).is_ok_and(|value| !value.trim().is_empty()))
}

fn text<'a>(i18n: I18n, korean: &'a str, english: &'a str) -> &'a str {
    if i18n.language() == ResolvedLanguage::KoKr {
        korean
    } else {
        english
    }
}
