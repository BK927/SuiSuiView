use super::scalers::WgpuScaleDirection;
use super::{
    default_top_bar_cpu_scale_filters, default_top_bar_wgpu_downscale_methods,
    default_top_bar_wgpu_upscale_methods, AppSettings, CacheMemoryMode, CpuScaleFilter, DecodeMode,
    DecoderPreference, DecoderPreferences, EdgePageAction, GpuEffectMode, PageTransitionStyle,
    PersistedState, RendererMode, TopBarItems, WgpuDownscaleMethod, WgpuScalePlan,
    WgpuUpscaleMethod, WheelMode, WindowPlacement, DEFAULT_MANUAL_CACHE_MB,
};
use crate::core::i18n::{I18n, Language, ResolvedLanguage};

#[test]
fn old_state_without_settings_loads_defaults() {
    let state: PersistedState = serde_json::from_str(r#"{"version":1,"books":{}}"#).unwrap();

    assert_eq!(state.settings, AppSettings::default());
    assert_eq!(state.window, WindowPlacement::default());
    assert!(state.books.is_empty());
}

#[test]
fn window_placement_without_physical_position_loads_none() {
    let state: PersistedState = serde_json::from_str(
        r#"{"version":1,"window":{"inner_size":[1280.0,820.0],"outer_position":[100.0,120.0],"maximized":false},"books":{}}"#,
    )
    .unwrap();

    assert_eq!(state.window.inner_size, Some([1280.0, 820.0]));
    assert_eq!(state.window.outer_position, Some([100.0, 120.0]));
    assert_eq!(state.window.outer_position_px, None);
}

#[test]
fn window_placement_round_trip_keeps_physical_position() {
    let placement = WindowPlacement {
        inner_size: Some([1280.0, 820.0]),
        outer_position: Some([100.0, 120.0]),
        outer_position_px: Some([150, 180]),
        maximized: false,
    };

    let json = serde_json::to_string(&placement).unwrap();
    let round_trip: WindowPlacement = serde_json::from_str(&json).unwrap();

    assert_eq!(round_trip, placement);
    assert_eq!(round_trip.outer_position_px, Some([150, 180]));
}

#[test]
fn old_settings_without_decoder_preferences_load_defaults() {
    let state: PersistedState =
        serde_json::from_str(r#"{"version":1,"settings":{},"books":{}}"#).unwrap();

    assert_eq!(
        state.settings.decoder_preferences,
        DecoderPreferences::default()
    );
}

#[test]
fn settings_defaults_match_viewer_policy() {
    let settings = AppSettings::default();

    assert_eq!(settings.language, Language::Auto);
    assert!(settings.confirm_delete);
    assert!(settings.esc_to_quit);
    assert!(settings.show_toasts);
    assert!(settings.remember_recent_locations);
    assert!(!settings.single_instance);
    assert_eq!(settings.top_bar_items, TopBarItems::default());
    assert_eq!(
        settings.top_bar_cpu_scale_filters,
        default_top_bar_cpu_scale_filters()
    );
    assert_eq!(
        settings.top_bar_wgpu_upscale_methods,
        default_top_bar_wgpu_upscale_methods()
    );
    assert_eq!(
        settings.top_bar_wgpu_downscale_methods,
        default_top_bar_wgpu_downscale_methods()
    );
    assert_eq!(settings.image_edge_page_action, EdgePageAction::Wrap);
    assert_eq!(settings.archive_edge_page_action, EdgePageAction::Ask);
    assert_eq!(settings.edge_page_action, EdgePageAction::Stop);
    assert_eq!(settings.decode_mode, DecodeMode::AutoFast);
    assert_eq!(settings.decoder_preferences, DecoderPreferences::default());
    assert!(settings.fast_sampled_scaled_decode);
    assert_eq!(settings.cpu_upscale_filter, CpuScaleFilter::CatmullRom);
    assert_eq!(settings.cpu_downscale_filter, CpuScaleFilter::Hamming);
    assert_eq!(settings.gpu_effect_mode, GpuEffectMode::Auto);
    assert_eq!(settings.renderer_mode, RendererMode::LowMemoryGlow);
    assert_eq!(settings.wgpu_upscale_method, WgpuUpscaleMethod::None);
    assert_eq!(
        settings.wgpu_downscale_method,
        WgpuDownscaleMethod::PyramidLanczos3
    );
    assert!(settings.prefetch_enabled);
    assert!(!settings.progressive_preview_enabled);
    assert!(!settings.transition_effect);
    assert_eq!(
        settings.effective_page_transition_style(),
        PageTransitionStyle::None
    );
    assert_eq!(settings.cache_memory_mode, CacheMemoryMode::Auto);
    assert_eq!(settings.manual_cache_mb, DEFAULT_MANUAL_CACHE_MB);
    assert!(settings.apply_exif_orientation);
    assert!(!settings.apply_embedded_icc);
    assert!(settings.auto_save_reading_position);
    assert!(settings.remember_archive_page_name);
    assert_eq!(settings.wheel_mode, WheelMode::PageTurn);
}

#[test]
fn top_bar_items_default_to_visible_for_old_settings() {
    let state: PersistedState =
        serde_json::from_str(r#"{"version":4,"settings":{},"books":{}}"#).unwrap();

    assert_eq!(state.settings.top_bar_items, TopBarItems::default());
}

#[test]
fn top_bar_items_partial_json_defaults_missing_groups_to_visible() {
    let state: PersistedState = serde_json::from_str(
        r#"{"version":4,"settings":{"top_bar_items":{"compare":false}},"books":{}}"#,
    )
    .unwrap();

    assert!(state.settings.top_bar_items.open);
    assert!(state.settings.top_bar_items.page);
    assert!(state.settings.top_bar_items.view);
    assert!(state.settings.top_bar_items.adjust);
    assert!(!state.settings.top_bar_items.compare);
    assert!(state.settings.top_bar_items.bookmarks);
}

#[test]
fn top_bar_items_round_trip() {
    let settings = AppSettings {
        top_bar_items: TopBarItems {
            open: false,
            page: true,
            view: false,
            adjust: true,
            compare: false,
            bookmarks: true,
        },
        ..AppSettings::default()
    };

    let json = serde_json::to_string(&settings).unwrap();
    let round_trip: AppSettings = serde_json::from_str(&json).unwrap();

    assert_eq!(round_trip.top_bar_items, settings.top_bar_items);
}

#[test]
fn top_bar_scaler_candidates_default_for_old_settings() {
    let state: PersistedState =
        serde_json::from_str(r#"{"version":4,"settings":{},"books":{}}"#).unwrap();

    assert_eq!(
        state.settings.top_bar_cpu_scale_filters,
        default_top_bar_cpu_scale_filters()
    );
    assert_eq!(
        state.settings.top_bar_wgpu_upscale_methods,
        default_top_bar_wgpu_upscale_methods()
    );
    assert_eq!(
        state.settings.top_bar_wgpu_downscale_methods,
        default_top_bar_wgpu_downscale_methods()
    );
}

#[test]
fn top_bar_scaler_candidates_round_trip() {
    let settings = AppSettings {
        top_bar_cpu_scale_filters: vec![CpuScaleFilter::Nearest, CpuScaleFilter::Lanczos3],
        top_bar_wgpu_upscale_methods: vec![
            WgpuUpscaleMethod::Auto,
            WgpuUpscaleMethod::WgslAnime4kV32CnnX2M,
        ],
        top_bar_wgpu_downscale_methods: vec![
            WgpuDownscaleMethod::Bilinear,
            WgpuDownscaleMethod::PyramidLanczos3,
        ],
        ..AppSettings::default()
    };

    let json = serde_json::to_string(&settings).unwrap();
    let round_trip: AppSettings = serde_json::from_str(&json).unwrap();

    assert_eq!(
        round_trip.top_bar_cpu_scale_filters,
        settings.top_bar_cpu_scale_filters
    );
    assert_eq!(
        round_trip.top_bar_wgpu_upscale_methods,
        settings.top_bar_wgpu_upscale_methods
    );
    assert_eq!(
        round_trip.top_bar_wgpu_downscale_methods,
        settings.top_bar_wgpu_downscale_methods
    );
}

#[test]
fn settings_normalization_does_not_rewrite_manual_cache_mb() {
    let mut settings = AppSettings {
        cache_memory_mode: CacheMemoryMode::Manual,
        manual_cache_mb: 4096,
        ..AppSettings::default()
    };

    settings.normalize_product_choices();

    assert_eq!(settings.manual_cache_mb, 4096);
}

#[test]
fn legacy_cache_memory_mode_tokens_still_deserialize() {
    let auto: PersistedState =
        serde_json::from_str(r#"{"version":1,"settings":{"cache_memory_mode":"Auto"},"books":{}}"#)
            .unwrap();
    let manual: PersistedState = serde_json::from_str(
        r#"{"version":1,"settings":{"cache_memory_mode":"Manual"},"books":{}}"#,
    )
    .unwrap();

    assert_eq!(auto.settings.cache_memory_mode, CacheMemoryMode::Auto);
    assert_eq!(manual.settings.cache_memory_mode, CacheMemoryMode::Manual);
}

#[test]
fn new_cache_memory_mode_presets_round_trip() {
    for mode in [
        CacheMemoryMode::Auto,
        CacheMemoryMode::Saver,
        CacheMemoryMode::Standard,
        CacheMemoryMode::Ample,
        CacheMemoryMode::Manual,
    ] {
        let json = serde_json::to_string(&mode).unwrap();
        let round_trip: CacheMemoryMode = serde_json::from_str(&json).unwrap();
        assert_eq!(round_trip, mode);
    }

    // The three preset variants use their own serde tokens.
    assert_eq!(
        serde_json::to_string(&CacheMemoryMode::Saver).unwrap(),
        r#""Saver""#
    );
    assert_eq!(
        serde_json::to_string(&CacheMemoryMode::Standard).unwrap(),
        r#""Standard""#
    );
    assert_eq!(
        serde_json::to_string(&CacheMemoryMode::Ample).unwrap(),
        r#""Ample""#
    );
}

#[test]
fn old_settings_without_language_load_auto_default() {
    let state: PersistedState =
        serde_json::from_str(r#"{"version":1,"settings":{},"books":{}}"#).unwrap();

    assert_eq!(state.settings.language, Language::Auto);
}

#[test]
fn persisted_wgpu_downscale_method_values_still_load() {
    let hamming: PersistedState = serde_json::from_str(
        r#"{"version":1,"settings":{"wgpu_downscale_method":"Hamming"},"books":{}}"#,
    )
    .unwrap();
    let lanczos3: PersistedState = serde_json::from_str(
        r#"{"version":1,"settings":{"wgpu_downscale_method":"Lanczos3"},"books":{}}"#,
    )
    .unwrap();

    assert_eq!(
        hamming.settings.wgpu_downscale_method,
        WgpuDownscaleMethod::Hamming
    );
    assert_eq!(
        lanczos3.settings.wgpu_downscale_method,
        WgpuDownscaleMethod::Lanczos3
    );
}

fn serde_variant_name(method: WgpuDownscaleMethod) -> String {
    match serde_json::to_value(method).unwrap() {
        serde_json::Value::String(name) => name,
        other => panic!("unexpected serialization for {method:?}: {other:?}"),
    }
}

fn settings_with_wgpu_downscale_method(variant: &str) -> AppSettings {
    let json = format!(
        r#"{{"version":1,"settings":{{"wgpu_downscale_method":"{variant}"}},"books":{{}}}}"#
    );
    serde_json::from_str::<PersistedState>(&json)
        .unwrap()
        .settings
}

#[test]
fn removed_wgpu_downscale_methods_normalize_to_fallbacks() {
    // token deserializes fine (variant kept), then normalize folds onto SELECTABLE.
    let cases = [
        (WgpuDownscaleMethod::Nearest, WgpuDownscaleMethod::Bilinear),
        (WgpuDownscaleMethod::Box, WgpuDownscaleMethod::Hamming),
        (
            WgpuDownscaleMethod::Mitchell,
            WgpuDownscaleMethod::CatmullRom,
        ),
        (WgpuDownscaleMethod::Lanczos2, WgpuDownscaleMethod::Lanczos3),
        (
            WgpuDownscaleMethod::HardwareMipmapLinear,
            WgpuDownscaleMethod::Bilinear,
        ),
        (
            WgpuDownscaleMethod::PyramidBoxTent,
            WgpuDownscaleMethod::PyramidHamming,
        ),
        (
            WgpuDownscaleMethod::PyramidMitchell,
            WgpuDownscaleMethod::PyramidLanczos3,
        ),
        (
            WgpuDownscaleMethod::PyramidLanczos2,
            WgpuDownscaleMethod::PyramidLanczos3,
        ),
    ];

    for (removed, expected) in cases {
        let mut settings = settings_with_wgpu_downscale_method(&serde_variant_name(removed));
        assert_eq!(settings.wgpu_downscale_method, removed);
        settings.normalize_product_choices();
        assert_eq!(
            settings.wgpu_downscale_method, expected,
            "{removed:?} should fold to {expected:?}"
        );
    }
}

#[test]
fn selectable_wgpu_downscale_methods_survive_normalize_and_round_trip() {
    for kept in WgpuDownscaleMethod::SELECTABLE {
        let mut settings = AppSettings {
            wgpu_downscale_method: kept,
            ..AppSettings::default()
        };
        settings.normalize_product_choices();
        assert_eq!(settings.wgpu_downscale_method, kept);

        let json = serde_json::to_string(&settings).unwrap();
        let restored: AppSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.wgpu_downscale_method, kept);
    }
}

#[test]
fn normalize_dedups_top_bar_wgpu_downscale_methods_in_order() {
    let mut settings = AppSettings {
        top_bar_wgpu_downscale_methods: vec![
            WgpuDownscaleMethod::HardwareMipmapLinear,
            WgpuDownscaleMethod::PyramidMitchell,
            WgpuDownscaleMethod::Bilinear,
        ],
        ..AppSettings::default()
    };
    settings.normalize_product_choices();

    assert_eq!(
        settings.top_bar_wgpu_downscale_methods,
        vec![
            WgpuDownscaleMethod::Bilinear,
            WgpuDownscaleMethod::PyramidLanczos3,
        ]
    );
}

#[test]
fn default_top_bar_wgpu_downscale_methods_are_all_selectable() {
    for method in default_top_bar_wgpu_downscale_methods() {
        assert!(
            WgpuDownscaleMethod::SELECTABLE.contains(&method),
            "{method:?} in defaults is not SELECTABLE"
        );
    }
}

#[test]
fn normalized_wgpu_downscale_method_reserializes_to_kept_token() {
    let mut settings = settings_with_wgpu_downscale_method(&serde_variant_name(
        WgpuDownscaleMethod::PyramidMitchell,
    ));
    settings.normalize_product_choices();

    let value = serde_json::to_value(&settings).unwrap();
    assert_eq!(value["wgpu_downscale_method"], "PyramidLanczos3");
}

#[test]
fn language_setting_round_trips() {
    let json = serde_json::to_string(&AppSettings {
        language: Language::EnUs,
        ..AppSettings::default()
    })
    .unwrap();
    let settings: AppSettings = serde_json::from_str(&json).unwrap();

    assert_eq!(settings.language, Language::EnUs);
}

#[test]
fn app_settings_save_new_scaler_keys() {
    let json = serde_json::to_value(AppSettings {
        fast_sampled_scaled_decode: false,
        cpu_upscale_filter: CpuScaleFilter::Lanczos3,
        cpu_downscale_filter: CpuScaleFilter::Hamming,
        wgpu_upscale_method: WgpuUpscaleMethod::WgslFsr1EasuRcas,
        wgpu_downscale_method: WgpuDownscaleMethod::PyramidLanczos3,
        ..AppSettings::default()
    })
    .unwrap();
    let object = json.as_object().unwrap();

    assert_eq!(object["fast_sampled_scaled_decode"], false);
    assert_eq!(object["cpu_upscale_filter"], "Lanczos3");
    assert_eq!(object["cpu_downscale_filter"], "Hamming");
    assert_eq!(object["wgpu_upscale_method"], "WgslFsr1EasuRcas");
    assert_eq!(object["wgpu_downscale_method"], "PyramidLanczos3");
    assert!(!object.contains_key("resize_filter"));
    assert!(!object.contains_key("display_upscaler"));
    assert!(!object.contains_key("wgpu_downscaler"));
    assert!(!object.contains_key("cpu_upscaler"));
    assert!(!object.contains_key("cpu_downscaler"));
}

#[test]
fn wgpu_upscale_method_settings_normalize_only_unselectable_methods_to_auto() {
    let mut settings = AppSettings {
        wgpu_upscale_method: WgpuUpscaleMethod::NvidiaNis,
        ..AppSettings::default()
    };

    settings.normalize_product_choices();

    assert_eq!(settings.wgpu_upscale_method, WgpuUpscaleMethod::Auto);

    settings.wgpu_upscale_method = WgpuUpscaleMethod::WgslFsr1Style;
    settings.normalize_product_choices();

    assert_eq!(
        settings.wgpu_upscale_method,
        WgpuUpscaleMethod::WgslFsr1Style
    );

    settings.wgpu_upscale_method = WgpuUpscaleMethod::WgslNisStyle;
    settings.normalize_product_choices();

    assert_eq!(
        settings.wgpu_upscale_method,
        WgpuUpscaleMethod::WgslNisStyle
    );

    settings.wgpu_upscale_method = WgpuUpscaleMethod::WgslArtcnnC4F16;
    settings.normalize_product_choices();

    assert_eq!(
        settings.wgpu_upscale_method,
        WgpuUpscaleMethod::WgslArtcnnC4F16
    );

    settings.wgpu_upscale_method = WgpuUpscaleMethod::WgslArtcnnC4F32Ds;
    settings.normalize_product_choices();

    assert_eq!(
        settings.wgpu_upscale_method,
        WgpuUpscaleMethod::WgslArtcnnC4F32Ds
    );

    settings.wgpu_upscale_method = WgpuUpscaleMethod::WgslSrLabSpanX2;
    settings.normalize_product_choices();

    assert_eq!(
        settings.wgpu_upscale_method,
        WgpuUpscaleMethod::WgslSrLabSpanX2
    );
}

#[test]
fn wgpu_scale_plan_activates_only_the_matching_direction() {
    assert_eq!(
        WgpuScalePlan::resolve(
            [800, 1200],
            [1600, 2400],
            WgpuUpscaleMethod::Auto,
            WgpuDownscaleMethod::Hamming
        ),
        WgpuScalePlan {
            direction: WgpuScaleDirection::Upscale,
            effective_upscale_method: WgpuUpscaleMethod::WgslFsr1EasuRcas,
            effective_downscale_method: WgpuDownscaleMethod::Bilinear,
        }
    );
    assert_eq!(
        WgpuScalePlan::resolve(
            [1600, 2400],
            [800, 1200],
            WgpuUpscaleMethod::WgslArtcnnC4F16,
            WgpuDownscaleMethod::Hamming
        ),
        WgpuScalePlan {
            direction: WgpuScaleDirection::Downscale,
            effective_upscale_method: WgpuUpscaleMethod::None,
            effective_downscale_method: WgpuDownscaleMethod::Hamming,
        }
    );
    assert_eq!(
        WgpuScalePlan::resolve(
            [1600, 2400],
            [1600, 2400],
            WgpuUpscaleMethod::WgslArtcnnC4F16,
            WgpuDownscaleMethod::Hamming
        ),
        WgpuScalePlan {
            direction: WgpuScaleDirection::Native,
            effective_upscale_method: WgpuUpscaleMethod::None,
            effective_downscale_method: WgpuDownscaleMethod::Bilinear,
        }
    );
}

#[test]
fn wgpu_scale_plan_uses_bilinear_for_mixed_axis_resize() {
    assert_eq!(
        WgpuScalePlan::resolve(
            [1000, 1000],
            [1200, 800],
            WgpuUpscaleMethod::WgslFsr1EasuRcas,
            WgpuDownscaleMethod::PyramidLanczos3
        ),
        WgpuScalePlan {
            direction: WgpuScaleDirection::Mixed,
            effective_upscale_method: WgpuUpscaleMethod::None,
            effective_downscale_method: WgpuDownscaleMethod::Bilinear,
        }
    );
}

#[test]
fn wgpu_scale_plan_treats_near_one_residual_shrink_as_native() {
    // ~0.95 residual (e.g. 256px-quantized 1536 source drawn to a 1460 rect) stays
    // on the egui sampler instead of taking the WGSL downscale pass.
    let near_native = WgpuScalePlan::resolve(
        [1536, 1536],
        [1460, 1460],
        WgpuUpscaleMethod::None,
        WgpuDownscaleMethod::PyramidLanczos3,
    );
    assert_eq!(near_native.direction, WgpuScaleDirection::Native);
    assert_eq!(
        near_native.effective_downscale_method,
        WgpuDownscaleMethod::Bilinear
    );

    // A real ~0.85 shrink still earns the requested WGSL downscaler.
    let true_downscale = WgpuScalePlan::resolve(
        [1536, 1536],
        [1306, 1306],
        WgpuUpscaleMethod::None,
        WgpuDownscaleMethod::PyramidLanczos3,
    );
    assert_eq!(true_downscale.direction, WgpuScaleDirection::Downscale);
    assert_eq!(
        true_downscale.effective_downscale_method,
        WgpuDownscaleMethod::PyramidLanczos3
    );

    // The 0.90 boundary is inclusive (>= threshold => Native).
    let boundary = WgpuScalePlan::resolve(
        [1000, 1000],
        [900, 900],
        WgpuUpscaleMethod::None,
        WgpuDownscaleMethod::PyramidLanczos3,
    );
    assert_eq!(boundary.direction, WgpuScaleDirection::Native);
    assert_eq!(
        boundary.effective_downscale_method,
        WgpuDownscaleMethod::Bilinear
    );

    // Upscale / mixed directions are untouched by the near-native downscale gate.
    assert_eq!(
        WgpuScalePlan::resolve(
            [1000, 1000],
            [1090, 1090],
            WgpuUpscaleMethod::WgslFsr1EasuRcas,
            WgpuDownscaleMethod::PyramidLanczos3
        )
        .direction,
        WgpuScaleDirection::Upscale
    );
    assert_eq!(
        WgpuScalePlan::resolve(
            [1000, 1000],
            [1090, 950],
            WgpuUpscaleMethod::None,
            WgpuDownscaleMethod::PyramidLanczos3
        )
        .direction,
        WgpuScaleDirection::Mixed
    );
}

#[test]
fn wgpu_scale_plan_keeps_upscalers_out_of_downscale_and_native_paths() {
    for method in [
        WgpuUpscaleMethod::WgslFsr1EasuRcas,
        WgpuUpscaleMethod::WgslBilinear,
        WgpuUpscaleMethod::WgslNisStyle,
        WgpuUpscaleMethod::WgslArtcnnC4F16,
        WgpuUpscaleMethod::WgslArtcnnC4F32Ds,
        WgpuUpscaleMethod::WgslSrLabSpanX2,
    ] {
        assert_eq!(
            WgpuScalePlan::resolve(
                [1600, 2400],
                [800, 1200],
                method,
                WgpuDownscaleMethod::PyramidLanczos3
            )
            .effective_upscale_method,
            WgpuUpscaleMethod::None
        );
        assert_eq!(
            WgpuScalePlan::resolve(
                [1600, 2400],
                [1600, 2400],
                method,
                WgpuDownscaleMethod::PyramidLanczos3
            )
            .effective_upscale_method,
            WgpuUpscaleMethod::None
        );
    }
}

#[test]
fn wgpu_downscale_method_resolves_only_when_page_is_shrunk() {
    assert_eq!(
        WgpuDownscaleMethod::Hamming.resolve_for_downscale([1600, 2400], [800, 1200]),
        WgpuDownscaleMethod::Hamming
    );
    assert_eq!(
        WgpuDownscaleMethod::Hamming.resolve_for_downscale([800, 1200], [1600, 2400]),
        WgpuDownscaleMethod::Bilinear
    );
    assert_eq!(WgpuDownscaleMethod::Lanczos2.shader_method_id(), 7);
    assert_eq!(
        WgpuDownscaleMethod::PyramidLanczos3.resolve_for_downscale([1600, 2400], [800, 1200]),
        WgpuDownscaleMethod::PyramidLanczos3
    );
    assert_eq!(
        WgpuDownscaleMethod::PyramidLanczos3.resolve_for_downscale([800, 1200], [1600, 2400]),
        WgpuDownscaleMethod::Bilinear
    );
    assert_eq!(WgpuDownscaleMethod::PyramidLanczos3.shader_method_id(), 8);
    assert_eq!(
        WgpuDownscaleMethod::PyramidBoxTent.base_filter(),
        WgpuDownscaleMethod::Bilinear
    );
    assert_eq!(
        WgpuDownscaleMethod::PyramidBoxTent.pyramid_stage_filter(),
        WgpuDownscaleMethod::Box
    );
    assert!(WgpuDownscaleMethod::HardwareMipmapLinear.is_hardware_mipmap());
}

#[test]
fn product_wgpu_upscale_methods_keep_experiments_separate() {
    assert!(WgpuUpscaleMethod::ALL.contains(&WgpuUpscaleMethod::WgslFsr1Style));
    assert!(!WgpuUpscaleMethod::ALL.contains(&WgpuUpscaleMethod::WgslNisStyle));
    assert!(!WgpuUpscaleMethod::ALL.contains(&WgpuUpscaleMethod::WgslArtcnnC4F16));
    assert!(!WgpuUpscaleMethod::ALL.contains(&WgpuUpscaleMethod::WgslArtcnnC4F32Ds));
    assert!(WgpuUpscaleMethod::ALL.contains(&WgpuUpscaleMethod::WgslSrLabSpanX2));
    assert!(WgpuUpscaleMethod::SETTINGS_CHOICES.contains(&WgpuUpscaleMethod::WgslFsr1Style));
    assert!(WgpuUpscaleMethod::SETTINGS_CHOICES.contains(&WgpuUpscaleMethod::WgslNisStyle));
    assert!(WgpuUpscaleMethod::SETTINGS_CHOICES.contains(&WgpuUpscaleMethod::WgslArtcnnC4F16));
    assert!(WgpuUpscaleMethod::SETTINGS_CHOICES.contains(&WgpuUpscaleMethod::WgslArtcnnC4F16Dn));
    assert!(WgpuUpscaleMethod::SETTINGS_CHOICES.contains(&WgpuUpscaleMethod::WgslArtcnnC4F16Ds));
    assert!(WgpuUpscaleMethod::SETTINGS_CHOICES.contains(&WgpuUpscaleMethod::WgslArtcnnC4F32));
    assert!(WgpuUpscaleMethod::SETTINGS_CHOICES.contains(&WgpuUpscaleMethod::WgslArtcnnC4F32Dn));
    assert!(WgpuUpscaleMethod::SETTINGS_CHOICES.contains(&WgpuUpscaleMethod::WgslArtcnnC4F32Ds));
    assert!(WgpuUpscaleMethod::SETTINGS_CHOICES.contains(&WgpuUpscaleMethod::WgslSrLabSpanX2));
    assert!(!WgpuUpscaleMethod::SETTINGS_CHOICES.contains(&WgpuUpscaleMethod::NvidiaNis));
    assert!(WgpuUpscaleMethod::WgslFsr1Style.candidate().product_visible);
    assert!(!WgpuUpscaleMethod::WgslNisStyle.candidate().product_visible);
    assert!(
        !WgpuUpscaleMethod::WgslArtcnnC4F16
            .candidate()
            .product_visible
    );
    assert!(
        !WgpuUpscaleMethod::WgslArtcnnC4F32Ds
            .candidate()
            .product_visible
    );
    assert!(
        WgpuUpscaleMethod::WgslSrLabSpanX2
            .candidate()
            .product_visible
    );
    assert!(WgpuUpscaleMethod::WgslFsr1Style.product_selectable());
    assert!(!WgpuUpscaleMethod::WgslNisStyle.product_selectable());
    assert!(!WgpuUpscaleMethod::WgslArtcnnC4F16.product_selectable());
    assert!(WgpuUpscaleMethod::WgslSrLabSpanX2.product_selectable());
    assert!(!WgpuUpscaleMethod::WgslFsr1Style.experimental_selectable());
    assert!(WgpuUpscaleMethod::WgslNisStyle.experimental_selectable());
    assert!(WgpuUpscaleMethod::WgslArtcnnC4F16.experimental_selectable());
    assert!(WgpuUpscaleMethod::WgslArtcnnC4F32Ds.experimental_selectable());
    assert!(!WgpuUpscaleMethod::WgslSrLabSpanX2.experimental_selectable());
    assert!(WgpuUpscaleMethod::WgslFsr1Style.user_selectable());
    assert!(WgpuUpscaleMethod::WgslNisStyle.user_selectable());
    assert!(WgpuUpscaleMethod::WgslArtcnnC4F16.user_selectable());
    assert!(WgpuUpscaleMethod::WgslArtcnnC4F32Ds.user_selectable());
    assert!(WgpuUpscaleMethod::WgslSrLabSpanX2.user_selectable());
    assert!(!WgpuUpscaleMethod::NvidiaNis.user_selectable());
    assert_eq!(
        WgpuUpscaleMethod::WgslFsr1Style
            .settings_label_i18n(I18n::resolved(ResolvedLanguage::KoKr)),
        "WGSL FSR-style"
    );
    assert_eq!(
        WgpuUpscaleMethod::WgslArtcnnC4F16
            .settings_label_i18n(I18n::resolved(ResolvedLanguage::KoKr)),
        "ArtCNN C4F16 (실험)"
    );
    assert_eq!(
        WgpuUpscaleMethod::WgslSrLabSpanX2
            .settings_label_i18n(I18n::resolved(ResolvedLanguage::KoKr)),
        "SR Lab SPAN x2 (느림)"
    );
    assert_eq!(
        WgpuUpscaleMethod::WgslSrLabSpanX2
            .settings_label_i18n(I18n::resolved(ResolvedLanguage::EnUs)),
        "SR Lab SPAN x2 (Slow)"
    );
    assert_eq!(
        WgpuUpscaleMethod::WgslFsr1EasuRcas
            .settings_label_i18n(I18n::resolved(ResolvedLanguage::KoKr)),
        "WGSL FSR1 EASU+RCAS"
    );
    assert_eq!(
        WgpuUpscaleMethod::WgslArtcnnC4F16
            .settings_label_i18n(I18n::resolved(ResolvedLanguage::EnUs)),
        "ArtCNN C4F16 (Experimental)"
    );
    assert_eq!(
        WgpuUpscaleMethod::Auto.label_i18n(I18n::resolved(ResolvedLanguage::KoKr)),
        "자동"
    );
    assert_eq!(
        WgpuUpscaleMethod::Auto.label_i18n(I18n::resolved(ResolvedLanguage::EnUs)),
        "Auto"
    );
    assert_eq!(
        DecoderPreference::Default.label_i18n(I18n::resolved(ResolvedLanguage::KoKr)),
        "기본값"
    );
    assert_eq!(
        DecoderPreference::Default.label_i18n(I18n::resolved(ResolvedLanguage::EnUs)),
        "Default"
    );
    assert!(!WgpuUpscaleMethod::WgslSrLabSpanX2.is_benchmark_only());
    assert!(WgpuUpscaleMethod::GPU_METHODS.contains(&WgpuUpscaleMethod::WgslSrLabSpanX2));
    assert!(WgpuUpscaleMethod::GPU_METHODS.contains(&WgpuUpscaleMethod::WgslArtcnnC4F16));
    assert!(WgpuUpscaleMethod::WgslArtcnnC4F16.is_benchmark_only());
    assert!(WgpuUpscaleMethod::WgslArtcnnC4F32Ds.is_benchmark_only());
    assert!(WgpuUpscaleMethod::ALL.contains(&WgpuUpscaleMethod::WgslFsr1EasuRcas));
    assert!(WgpuUpscaleMethod::ALL.contains(&WgpuUpscaleMethod::WgslAnime4kV32CnnX2S));
    assert!(WgpuUpscaleMethod::ALL.contains(&WgpuUpscaleMethod::WgslAnime4kV32CnnX2M));
    assert!(WgpuUpscaleMethod::ALL.contains(&WgpuUpscaleMethod::WgslAcnetF8B4Luma));
    assert!(WgpuUpscaleMethod::ALL.contains(&WgpuUpscaleMethod::WgslAcnetF8B4BoxLuma));
    assert!(WgpuUpscaleMethod::ALL.contains(&WgpuUpscaleMethod::WgslAcnetF8B4HdnLuma));
    assert!(WgpuUpscaleMethod::ALL.contains(&WgpuUpscaleMethod::WgslAcnetF8B4BoxHdnLuma));
    assert!(WgpuUpscaleMethod::ALL.contains(&WgpuUpscaleMethod::CunnyVeryfastNvl));
    assert!(WgpuUpscaleMethod::ALL.contains(&WgpuUpscaleMethod::CunnyVeryfastSoft));
    assert!(WgpuUpscaleMethod::ALL.contains(&WgpuUpscaleMethod::CunnyFasterNvl));
    assert!(WgpuUpscaleMethod::ALL.contains(&WgpuUpscaleMethod::CunnyFasterSoft));
    assert!(WgpuUpscaleMethod::ALL.contains(&WgpuUpscaleMethod::CunnyFasterDs));
    assert!(WgpuUpscaleMethod::ALL.contains(&WgpuUpscaleMethod::CunnyFastNvl));
    assert!(WgpuUpscaleMethod::ALL.contains(&WgpuUpscaleMethod::CunnyFastSoft));
    assert!(WgpuUpscaleMethod::ALL.contains(&WgpuUpscaleMethod::CunnyFastDs));
    assert!(WgpuUpscaleMethod::ALL.contains(&WgpuUpscaleMethod::Cunny2x12Soft));
    assert!(WgpuUpscaleMethod::ALL.contains(&WgpuUpscaleMethod::Cunny2x12Ds));
    assert!(WgpuUpscaleMethod::ALL.contains(&WgpuUpscaleMethod::Cunny3x12Nvl));
    assert!(WgpuUpscaleMethod::ALL.contains(&WgpuUpscaleMethod::Cunny3x12Soft));
    assert!(WgpuUpscaleMethod::ALL.contains(&WgpuUpscaleMethod::Cunny3x12Ds));
    assert!(WgpuUpscaleMethod::ALL.contains(&WgpuUpscaleMethod::Cunny4x12Nvl));
    assert!(WgpuUpscaleMethod::ALL.contains(&WgpuUpscaleMethod::Cunny4x12Soft));
    assert!(WgpuUpscaleMethod::ALL.contains(&WgpuUpscaleMethod::Cunny4x12Ds));
    assert!(WgpuUpscaleMethod::ALL.contains(&WgpuUpscaleMethod::Cunny4x16Nvl));
    assert!(WgpuUpscaleMethod::ALL.contains(&WgpuUpscaleMethod::Cunny4x16Soft));
    assert!(WgpuUpscaleMethod::ALL.contains(&WgpuUpscaleMethod::Cunny4x16Ds));
    assert!(WgpuUpscaleMethod::ALL.contains(&WgpuUpscaleMethod::Cunny4x24Nvl));
    assert!(WgpuUpscaleMethod::ALL.contains(&WgpuUpscaleMethod::Cunny4x24Soft));
    assert!(WgpuUpscaleMethod::ALL.contains(&WgpuUpscaleMethod::Cunny4x24Ds));
    assert!(WgpuUpscaleMethod::ALL.contains(&WgpuUpscaleMethod::Cunny4x32Nvl));
    assert!(WgpuUpscaleMethod::ALL.contains(&WgpuUpscaleMethod::Cunny4x32Soft));
    assert!(WgpuUpscaleMethod::ALL.contains(&WgpuUpscaleMethod::Cunny4x32Ds));
    assert!(WgpuUpscaleMethod::ALL.contains(&WgpuUpscaleMethod::Cunny8x32Nvl));
    assert!(WgpuUpscaleMethod::ALL.contains(&WgpuUpscaleMethod::Cunny8x32Ds));
    assert!(
        WgpuUpscaleMethod::WgslAnime4kV32CnnX2S
            .candidate()
            .product_visible
    );
    assert!(
        WgpuUpscaleMethod::WgslAnime4kV32CnnX2M
            .candidate()
            .product_visible
    );
    assert!(
        WgpuUpscaleMethod::WgslAcnetF8B4BoxHdnLuma
            .candidate()
            .product_visible
    );
    assert!(
        WgpuUpscaleMethod::CunnyVeryfastNvl
            .candidate()
            .product_visible
    );
    assert!(
        WgpuUpscaleMethod::CunnyVeryfastSoft
            .candidate()
            .product_visible
    );
    assert!(
        WgpuUpscaleMethod::CunnyFasterNvl
            .candidate()
            .product_visible
    );
    assert!(
        WgpuUpscaleMethod::CunnyFasterSoft
            .candidate()
            .product_visible
    );
    assert!(WgpuUpscaleMethod::CunnyFasterDs.candidate().product_visible);
    assert_eq!(WgpuUpscaleMethod::CunnyFastNvl.candidate().family, "CuNNy");
    assert_eq!(WgpuUpscaleMethod::CunnyFastSoft.candidate().family, "CuNNy");
    assert_eq!(WgpuUpscaleMethod::CunnyFastDs.candidate().family, "CuNNy");
    assert_eq!(WgpuUpscaleMethod::Cunny2x12Soft.candidate().family, "CuNNy");
    assert_eq!(WgpuUpscaleMethod::Cunny2x12Ds.candidate().family, "CuNNy");
    assert_eq!(WgpuUpscaleMethod::Cunny3x12Soft.candidate().family, "CuNNy");
    assert_eq!(WgpuUpscaleMethod::Cunny3x12Ds.candidate().family, "CuNNy");
    assert_eq!(WgpuUpscaleMethod::Cunny4x12Soft.candidate().family, "CuNNy");
    assert_eq!(WgpuUpscaleMethod::Cunny4x12Ds.candidate().family, "CuNNy");
    assert_eq!(WgpuUpscaleMethod::Cunny4x16Soft.candidate().family, "CuNNy");
    assert_eq!(WgpuUpscaleMethod::Cunny4x16Ds.candidate().family, "CuNNy");
    assert_eq!(WgpuUpscaleMethod::Cunny4x24Soft.candidate().family, "CuNNy");
    assert_eq!(WgpuUpscaleMethod::Cunny4x24Ds.candidate().family, "CuNNy");
    assert_eq!(WgpuUpscaleMethod::Cunny4x32Soft.candidate().family, "CuNNy");
    assert_eq!(WgpuUpscaleMethod::Cunny4x32Ds.candidate().family, "CuNNy");
    assert_eq!(WgpuUpscaleMethod::Cunny8x32Ds.candidate().family, "CuNNy");
    assert!(WgpuUpscaleMethod::Cunny4x16Soft.candidate().product_visible);
    assert!(WgpuUpscaleMethod::Cunny4x16Ds.candidate().product_visible);
    assert!(WgpuUpscaleMethod::Cunny4x24Soft.candidate().product_visible);
    assert!(WgpuUpscaleMethod::Cunny4x24Ds.candidate().product_visible);
    assert!(WgpuUpscaleMethod::Cunny4x32Soft.candidate().product_visible);
    assert!(WgpuUpscaleMethod::Cunny4x32Ds.candidate().product_visible);
    assert!(WgpuUpscaleMethod::Cunny8x32Ds.candidate().product_visible);
    assert!(WgpuUpscaleMethod::Cunny4x16Soft.product_selectable());
    assert!(WgpuUpscaleMethod::Cunny4x16Ds.product_selectable());
    assert!(WgpuUpscaleMethod::Cunny4x24Soft.product_selectable());
    assert!(WgpuUpscaleMethod::Cunny4x24Ds.product_selectable());
    assert!(WgpuUpscaleMethod::Cunny4x32Soft.product_selectable());
    assert!(WgpuUpscaleMethod::Cunny4x32Ds.product_selectable());
    assert!(WgpuUpscaleMethod::Cunny8x32Ds.product_selectable());
    assert!(!WgpuUpscaleMethod::Cunny4x16Soft.is_benchmark_only());
    assert!(!WgpuUpscaleMethod::Cunny4x16Ds.is_benchmark_only());
    assert!(!WgpuUpscaleMethod::Cunny4x24Soft.is_benchmark_only());
    assert!(!WgpuUpscaleMethod::Cunny4x24Ds.is_benchmark_only());
    assert!(!WgpuUpscaleMethod::Cunny4x32Soft.is_benchmark_only());
    assert!(!WgpuUpscaleMethod::Cunny4x32Ds.is_benchmark_only());
    assert!(!WgpuUpscaleMethod::Cunny8x32Ds.is_benchmark_only());
    assert_eq!(
        WgpuUpscaleMethod::CunnyVeryfastSoft.label(),
        "CuNNy veryfast SOFT"
    );
    assert_eq!(
        WgpuUpscaleMethod::CunnyFasterSoft.label(),
        "CuNNy faster SOFT"
    );
    assert_eq!(WgpuUpscaleMethod::CunnyFasterDs.label(), "CuNNy faster DS");
    assert_eq!(WgpuUpscaleMethod::Cunny2x12Soft.label(), "CuNNy 2x12 SOFT");
    assert_eq!(WgpuUpscaleMethod::Cunny2x12Ds.label(), "CuNNy 2x12 DS");
    assert_eq!(WgpuUpscaleMethod::Cunny3x12Soft.label(), "CuNNy 3x12 SOFT");
    assert_eq!(WgpuUpscaleMethod::Cunny3x12Ds.label(), "CuNNy 3x12 DS");
    assert_eq!(WgpuUpscaleMethod::Cunny3x12Nvl.label(), "CuNNy 3x12 NVL");
    assert_eq!(WgpuUpscaleMethod::Cunny4x12Soft.label(), "CuNNy 4x12 SOFT");
    assert_eq!(WgpuUpscaleMethod::Cunny4x12Ds.label(), "CuNNy 4x12 DS");
    assert_eq!(WgpuUpscaleMethod::Cunny4x12Nvl.label(), "CuNNy 4x12 NVL");
    assert_eq!(WgpuUpscaleMethod::Cunny4x16Nvl.label(), "CuNNy 4x16 NVL");
    assert_eq!(WgpuUpscaleMethod::Cunny4x16Soft.label(), "CuNNy 4x16 SOFT");
    assert_eq!(WgpuUpscaleMethod::Cunny4x16Ds.label(), "CuNNy 4x16 DS");
    assert_eq!(WgpuUpscaleMethod::Cunny4x24Nvl.label(), "CuNNy 4x24 NVL");
    assert_eq!(WgpuUpscaleMethod::Cunny4x24Soft.label(), "CuNNy 4x24 SOFT");
    assert_eq!(WgpuUpscaleMethod::Cunny4x24Ds.label(), "CuNNy 4x24 DS");
    assert_eq!(WgpuUpscaleMethod::Cunny4x32Nvl.label(), "CuNNy 4x32 NVL");
    assert_eq!(WgpuUpscaleMethod::Cunny4x32Soft.label(), "CuNNy 4x32 SOFT");
    assert_eq!(WgpuUpscaleMethod::Cunny4x32Ds.label(), "CuNNy 4x32 DS");
    assert_eq!(WgpuUpscaleMethod::Cunny8x32Nvl.label(), "CuNNy 8x32 NVL");
    assert_eq!(WgpuUpscaleMethod::Cunny8x32Ds.label(), "CuNNy 8x32 DS");
}

fn resolve_wgpu_upscale_for_test(
    method: WgpuUpscaleMethod,
    output_size: [usize; 2],
    target_size: [u32; 2],
) -> Option<WgpuUpscaleMethod> {
    let plan = WgpuScalePlan::resolve(
        output_size,
        target_size,
        method,
        WgpuDownscaleMethod::Hamming,
    );
    (plan.effective_upscale_method != WgpuUpscaleMethod::None)
        .then_some(plan.effective_upscale_method)
}

#[test]
fn fixed_2x_sr_falls_back_for_tiny_display_upscale() {
    assert_eq!(
        resolve_wgpu_upscale_for_test(
            WgpuUpscaleMethod::WgslAnime4kV32CnnX2M,
            [1000, 1000],
            [1090, 1090]
        ),
        Some(WgpuUpscaleMethod::WgslFsr1EasuRcas)
    );
    assert_eq!(
        resolve_wgpu_upscale_for_test(
            WgpuUpscaleMethod::WgslAnime4kV32CnnX2M,
            [1000, 1000],
            [1100, 1100]
        ),
        Some(WgpuUpscaleMethod::WgslAnime4kV32CnnX2M)
    );
}

#[test]
fn fixed_2x_sr_stack_passes_start_at_two_point_two_five_x() {
    assert_eq!(
        WgpuUpscaleMethod::WgslAnime4kV32CnnX2M.fixed_2x_stack_passes([1000, 1000], [2240, 2240]),
        1
    );
    assert_eq!(
        WgpuUpscaleMethod::WgslAnime4kV32CnnX2M.fixed_2x_stack_passes([1000, 1000], [2250, 2250]),
        2
    );
    assert_eq!(
        WgpuUpscaleMethod::WgslFsr1EasuRcas.fixed_2x_stack_passes([1000, 1000], [3000, 3000]),
        1
    );
}

#[test]
fn exact_cunny_variants_render_only_when_upscaling() {
    assert_eq!(
        resolve_wgpu_upscale_for_test(
            WgpuUpscaleMethod::CunnyVeryfastNvl,
            [1600, 2400],
            [800, 1200]
        ),
        None
    );
    assert_eq!(
        resolve_wgpu_upscale_for_test(
            WgpuUpscaleMethod::CunnyVeryfastNvl,
            [800, 1200],
            [1600, 2400]
        ),
        Some(WgpuUpscaleMethod::CunnyVeryfastNvl)
    );
    assert_eq!(
        resolve_wgpu_upscale_for_test(
            WgpuUpscaleMethod::CunnyVeryfastSoft,
            [1600, 2400],
            [800, 1200]
        ),
        None
    );
    assert_eq!(
        resolve_wgpu_upscale_for_test(
            WgpuUpscaleMethod::CunnyVeryfastSoft,
            [800, 1200],
            [1600, 2400]
        ),
        Some(WgpuUpscaleMethod::CunnyVeryfastSoft)
    );
    assert_eq!(
        resolve_wgpu_upscale_for_test(WgpuUpscaleMethod::CunnyFasterNvl, [1600, 2400], [800, 1200]),
        None
    );
    assert_eq!(
        resolve_wgpu_upscale_for_test(WgpuUpscaleMethod::CunnyFasterNvl, [800, 1200], [1600, 2400]),
        Some(WgpuUpscaleMethod::CunnyFasterNvl)
    );
    assert_eq!(
        resolve_wgpu_upscale_for_test(
            WgpuUpscaleMethod::CunnyFasterSoft,
            [1600, 2400],
            [800, 1200]
        ),
        None
    );
    assert_eq!(
        resolve_wgpu_upscale_for_test(
            WgpuUpscaleMethod::CunnyFasterSoft,
            [800, 1200],
            [1600, 2400]
        ),
        Some(WgpuUpscaleMethod::CunnyFasterSoft)
    );
    assert_eq!(
        resolve_wgpu_upscale_for_test(WgpuUpscaleMethod::CunnyFasterDs, [800, 1200], [1600, 2400]),
        Some(WgpuUpscaleMethod::CunnyFasterDs)
    );
    assert_eq!(
        resolve_wgpu_upscale_for_test(WgpuUpscaleMethod::CunnyFastNvl, [800, 1200], [1600, 2400]),
        Some(WgpuUpscaleMethod::CunnyFastNvl)
    );
    assert_eq!(
        resolve_wgpu_upscale_for_test(WgpuUpscaleMethod::CunnyFastSoft, [800, 1200], [1600, 2400]),
        Some(WgpuUpscaleMethod::CunnyFastSoft)
    );
    assert_eq!(
        resolve_wgpu_upscale_for_test(WgpuUpscaleMethod::CunnyFastDs, [800, 1200], [1600, 2400]),
        Some(WgpuUpscaleMethod::CunnyFastDs)
    );
    assert_eq!(
        resolve_wgpu_upscale_for_test(WgpuUpscaleMethod::Cunny2x12Soft, [800, 1200], [1600, 2400]),
        Some(WgpuUpscaleMethod::Cunny2x12Soft)
    );
    assert_eq!(
        resolve_wgpu_upscale_for_test(WgpuUpscaleMethod::Cunny2x12Ds, [800, 1200], [1600, 2400]),
        Some(WgpuUpscaleMethod::Cunny2x12Ds)
    );
    assert_eq!(
        resolve_wgpu_upscale_for_test(WgpuUpscaleMethod::Cunny3x12Nvl, [800, 1200], [1600, 2400]),
        Some(WgpuUpscaleMethod::Cunny3x12Nvl)
    );
    assert_eq!(
        resolve_wgpu_upscale_for_test(WgpuUpscaleMethod::Cunny3x12Soft, [800, 1200], [1600, 2400]),
        Some(WgpuUpscaleMethod::Cunny3x12Soft)
    );
    assert_eq!(
        resolve_wgpu_upscale_for_test(WgpuUpscaleMethod::Cunny3x12Ds, [800, 1200], [1600, 2400]),
        Some(WgpuUpscaleMethod::Cunny3x12Ds)
    );
    assert_eq!(
        resolve_wgpu_upscale_for_test(WgpuUpscaleMethod::Cunny4x12Nvl, [800, 1200], [1600, 2400]),
        Some(WgpuUpscaleMethod::Cunny4x12Nvl)
    );
    assert_eq!(
        resolve_wgpu_upscale_for_test(WgpuUpscaleMethod::Cunny4x12Soft, [800, 1200], [1600, 2400]),
        Some(WgpuUpscaleMethod::Cunny4x12Soft)
    );
    assert_eq!(
        resolve_wgpu_upscale_for_test(WgpuUpscaleMethod::Cunny4x12Ds, [800, 1200], [1600, 2400]),
        Some(WgpuUpscaleMethod::Cunny4x12Ds)
    );
    assert_eq!(
        resolve_wgpu_upscale_for_test(WgpuUpscaleMethod::Cunny4x16Nvl, [800, 1200], [1600, 2400]),
        Some(WgpuUpscaleMethod::Cunny4x16Nvl)
    );
    assert_eq!(
        resolve_wgpu_upscale_for_test(WgpuUpscaleMethod::Cunny4x16Soft, [800, 1200], [1600, 2400]),
        Some(WgpuUpscaleMethod::Cunny4x16Soft)
    );
    assert_eq!(
        resolve_wgpu_upscale_for_test(WgpuUpscaleMethod::Cunny4x16Ds, [800, 1200], [1600, 2400]),
        Some(WgpuUpscaleMethod::Cunny4x16Ds)
    );
    assert_eq!(
        resolve_wgpu_upscale_for_test(WgpuUpscaleMethod::Cunny4x24Nvl, [800, 1200], [1600, 2400]),
        Some(WgpuUpscaleMethod::Cunny4x24Nvl)
    );
    assert_eq!(
        resolve_wgpu_upscale_for_test(WgpuUpscaleMethod::Cunny4x24Soft, [800, 1200], [1600, 2400]),
        Some(WgpuUpscaleMethod::Cunny4x24Soft)
    );
    assert_eq!(
        resolve_wgpu_upscale_for_test(WgpuUpscaleMethod::Cunny4x24Ds, [800, 1200], [1600, 2400]),
        Some(WgpuUpscaleMethod::Cunny4x24Ds)
    );
    assert_eq!(
        resolve_wgpu_upscale_for_test(WgpuUpscaleMethod::Cunny4x32Nvl, [800, 1200], [1600, 2400]),
        Some(WgpuUpscaleMethod::Cunny4x32Nvl)
    );
    assert_eq!(
        resolve_wgpu_upscale_for_test(WgpuUpscaleMethod::Cunny4x32Soft, [800, 1200], [1600, 2400]),
        Some(WgpuUpscaleMethod::Cunny4x32Soft)
    );
    assert_eq!(
        resolve_wgpu_upscale_for_test(WgpuUpscaleMethod::Cunny4x32Ds, [800, 1200], [1600, 2400]),
        Some(WgpuUpscaleMethod::Cunny4x32Ds)
    );
    assert_eq!(
        resolve_wgpu_upscale_for_test(WgpuUpscaleMethod::Cunny8x32Nvl, [800, 1200], [1600, 2400]),
        Some(WgpuUpscaleMethod::Cunny8x32Nvl)
    );
    assert_eq!(
        resolve_wgpu_upscale_for_test(WgpuUpscaleMethod::Cunny8x32Ds, [800, 1200], [1600, 2400]),
        Some(WgpuUpscaleMethod::Cunny8x32Ds)
    );
    assert_eq!(
        resolve_wgpu_upscale_for_test(
            WgpuUpscaleMethod::WgslAnime4kV32CnnX2S,
            [800, 1200],
            [1600, 2400]
        ),
        Some(WgpuUpscaleMethod::WgslAnime4kV32CnnX2S)
    );
    assert_eq!(
        resolve_wgpu_upscale_for_test(
            WgpuUpscaleMethod::WgslAnime4kV32CnnX2M,
            [800, 1200],
            [1600, 2400]
        ),
        Some(WgpuUpscaleMethod::WgslAnime4kV32CnnX2M)
    );
    assert_eq!(
        resolve_wgpu_upscale_for_test(
            WgpuUpscaleMethod::WgslAcnetF8B4BoxHdnLuma,
            [800, 1200],
            [1600, 2400]
        ),
        Some(WgpuUpscaleMethod::WgslAcnetF8B4BoxHdnLuma)
    );
}
