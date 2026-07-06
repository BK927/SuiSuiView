use super::{
    default_top_bar_cpu_scale_filters, default_top_bar_wgpu_downscale_methods,
    default_top_bar_wgpu_upscale_methods, AppSettings, CacheMemoryMode, CpuScaleFilter, DecodeMode,
    DecoderPreferences, EdgePageAction, GpuEffectMode, PageTransitionStyle, PersistedState,
    RendererMode, TopBarItems, WgpuDownscaleMethod, WgpuUpscaleMethod, WheelMode, WindowPlacement,
    DEFAULT_MANUAL_CACHE_MB,
};
use crate::core::i18n::Language;

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
