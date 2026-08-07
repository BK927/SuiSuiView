use super::{
    default_key_bindings, default_top_bar_cpu_scale_filters, default_top_bar_wgpu_upscale_methods,
    AppSettings, CacheMemoryMode, CommandId, CpuScaleFilter, DecodeMode, DecoderPreferences,
    EdgePageAction, GpuEffectMode, KeyBinding, KeyCode, KeyShortcut, PageTransitionStyle,
    PersistedState, RendererMode, TopBarItems, WgpuUpscaleMethod, WheelMode, WindowPlacement,
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
        normal_rect_px: None,
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
    assert_eq!(settings.image_edge_page_action, EdgePageAction::Wrap);
    assert_eq!(settings.archive_edge_page_action, EdgePageAction::Ask);
    assert_eq!(settings.edge_page_action, EdgePageAction::Stop);
    assert_eq!(settings.decode_mode, DecodeMode::AutoFast);
    assert_eq!(settings.decoder_preferences, DecoderPreferences::default());
    assert!(settings.fast_sampled_scaled_decode);
    assert_eq!(settings.cpu_upscale_filter, CpuScaleFilter::CatmullRom);
    assert_eq!(settings.gpu_effect_mode, GpuEffectMode::Auto);
    assert_eq!(settings.renderer_mode, RendererMode::LowMemoryGlow);
    assert_eq!(settings.wgpu_upscale_method, WgpuUpscaleMethod::None);
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
}

#[test]
fn top_bar_scaler_candidates_round_trip() {
    let settings = AppSettings {
        top_bar_cpu_scale_filters: vec![CpuScaleFilter::Nearest, CpuScaleFilter::Lanczos3],
        top_bar_wgpu_upscale_methods: vec![
            WgpuUpscaleMethod::Auto,
            WgpuUpscaleMethod::WgslAnime4kV32CnnX2M,
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
fn settings_normalization_clamps_fixed_2x_sr_min_scale_pct() {
    let mut settings = AppSettings {
        fixed_2x_sr_min_scale_pct: 50,
        ..AppSettings::default()
    };
    settings.normalize_product_choices();
    assert_eq!(settings.fixed_2x_sr_min_scale_pct, 100);

    settings.fixed_2x_sr_min_scale_pct = 999;
    settings.normalize_product_choices();
    assert_eq!(settings.fixed_2x_sr_min_scale_pct, 200);
}

#[test]
fn settings_normalization_clamps_strip_scroll_sensitivities() {
    let mut settings = AppSettings {
        strip_wheel_scroll_pct: 50,
        strip_drag_scroll_pct: 9999,
        ..AppSettings::default()
    };
    settings.normalize_product_choices();
    assert_eq!(settings.strip_wheel_scroll_pct, 100);
    assert_eq!(settings.strip_drag_scroll_pct, 400);

    settings.strip_wheel_scroll_pct = 99999;
    settings.normalize_product_choices();
    assert_eq!(settings.strip_wheel_scroll_pct, 1200);
}

#[test]
fn old_settings_without_strip_scroll_sensitivities_load_defaults() {
    let state: PersistedState =
        serde_json::from_str(r#"{"version":4,"settings":{},"books":{}}"#).unwrap();

    assert_eq!(state.settings.strip_wheel_scroll_pct, 400);
    assert_eq!(state.settings.strip_drag_scroll_pct, 150);
    assert_eq!(state.settings.strip_wheel_scroll_multiplier(), 4.0);
    assert_eq!(state.settings.strip_drag_scroll_multiplier(), 1.5);
}

#[test]
fn settings_normalization_clamps_pixel_grid_min_zoom_pct() {
    let mut settings = AppSettings {
        pixel_grid_min_zoom_pct: 100,
        ..AppSettings::default()
    };
    settings.normalize_product_choices();
    assert_eq!(settings.pixel_grid_min_zoom_pct, 200);

    settings.pixel_grid_min_zoom_pct = 99999;
    settings.normalize_product_choices();
    assert_eq!(settings.pixel_grid_min_zoom_pct, 6400);
}

/// A profile written before a command existed: its bindings list shadows the
/// defaults, and `seen_commands` is empty.
fn legacy_profile_without(command: CommandId) -> AppSettings {
    AppSettings {
        key_bindings: default_key_bindings()
            .into_iter()
            .filter(|binding| binding.command != command)
            .collect(),
        seen_commands: Vec::new(),
        ..AppSettings::default()
    }
}

#[test]
fn legacy_profile_adopts_default_binding_for_new_command() {
    let mut settings = legacy_profile_without(CommandId::ToggleVerticalStrip);

    settings.normalize_product_choices();

    assert!(settings.key_bindings.contains(&KeyBinding {
        command: CommandId::ToggleVerticalStrip,
        shortcut: KeyShortcut::new(KeyCode::Num3),
    }));
    assert_eq!(settings.seen_commands, CommandId::ALL.to_vec());
}

#[test]
fn deliberately_unbound_command_is_not_resurrected() {
    let mut settings = legacy_profile_without(CommandId::ToggleVerticalStrip);
    settings.seen_commands = CommandId::ALL.to_vec();

    settings.normalize_product_choices();

    assert!(!settings
        .key_bindings
        .iter()
        .any(|binding| binding.command == CommandId::ToggleVerticalStrip));
}

#[test]
fn adoption_never_steals_a_shortcut_the_user_already_uses() {
    let mut settings = legacy_profile_without(CommandId::ToggleVerticalStrip);
    settings.key_bindings.push(KeyBinding {
        command: CommandId::NextPage,
        shortcut: KeyShortcut::new(KeyCode::Num3),
    });

    settings.normalize_product_choices();

    assert!(!settings
        .key_bindings
        .iter()
        .any(|binding| binding.command == CommandId::ToggleVerticalStrip));
    // The user's remap survives, and the command counts as seen from now on.
    assert!(settings.key_bindings.contains(&KeyBinding {
        command: CommandId::NextPage,
        shortcut: KeyShortcut::new(KeyCode::Num3),
    }));
    assert_eq!(settings.seen_commands, CommandId::ALL.to_vec());
}

#[test]
fn binding_adoption_is_idempotent() {
    let mut settings = legacy_profile_without(CommandId::ToggleVerticalStrip);

    settings.normalize_product_choices();
    let after_first = settings.key_bindings.clone();
    settings.normalize_product_choices();

    assert_eq!(settings.key_bindings, after_first);
}

#[test]
fn old_settings_without_pixel_grid_load_defaults() {
    let state: PersistedState =
        serde_json::from_str(r#"{"version":1,"settings":{},"books":{}}"#).unwrap();

    assert!(!state.settings.pixel_grid_enabled);
    assert_eq!(state.settings.pixel_grid_min_zoom_pct, 800);
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
fn retired_wgpu_downscale_json_keys_are_ignored() {
    // The WGPU downscaler is no longer user-configurable (fixed to PyramidLanczos3),
    // so `wgpu_downscale_method` and `top_bar_wgpu_downscale_methods` were removed
    // from AppSettings. Old state.json files still carry them; AppSettings has no
    // `deny_unknown_fields`, so they must be ignored on load rather than erroring.
    let mut state: PersistedState = serde_json::from_str(
        r#"{"version":4,"settings":{"wgpu_downscale_method":"Hamming","cpu_downscale_filter":"Nearest","top_bar_wgpu_downscale_methods":["Bilinear","PyramidLanczos3"]},"books":{}}"#,
    )
    .unwrap();

    // Only unknown keys were supplied, so every field falls back to its default.
    // (`seen_commands` deliberately deserializes empty as the legacy-profile
    // marker and only reaches its steady state through load-path normalization.)
    state.settings.normalize_product_choices();
    assert_eq!(state.settings, AppSettings::default());
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
        wgpu_upscale_method: WgpuUpscaleMethod::WgslFsr1EasuRcas,
        ..AppSettings::default()
    })
    .unwrap();
    let object = json.as_object().unwrap();

    assert_eq!(object["fast_sampled_scaled_decode"], false);
    assert_eq!(object["cpu_upscale_filter"], "Lanczos3");
    // The CPU downscale filter is fixed and no longer serialized.
    assert!(!object.contains_key("cpu_downscale_filter"));
    assert_eq!(object["wgpu_upscale_method"], "WgslFsr1EasuRcas");
    // The WGPU downscaler is fixed and no longer serialized.
    assert!(!object.contains_key("wgpu_downscale_method"));
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
fn withdrawn_upscalers_are_not_offered_but_stay_measurable() {
    // Measured below plain bilinear on two content types at two scales: offering
    // them lets a user make the page worse than doing nothing.
    for method in [
        WgpuUpscaleMethod::Cunny4x32Soft,
        WgpuUpscaleMethod::Cunny4x32Ds,
        WgpuUpscaleMethod::Cunny8x32Ds,
    ] {
        assert!(
            !method.user_selectable(),
            "{} must not be offered",
            method.token()
        );
        assert!(
            !WgpuUpscaleMethod::SETTINGS_CHOICES.contains(&method),
            "{} must not be listed in settings",
            method.token()
        );
        // Still reachable from the CLI bench so the port can be re-checked.
        assert!(
            WgpuUpscaleMethod::GPU_METHODS.contains(&method),
            "{} must stay benchmarkable",
            method.token()
        );
    }
    // The healthy 32-feature ports are untouched.
    assert!(WgpuUpscaleMethod::Cunny4x32Nvl.user_selectable());
    assert!(WgpuUpscaleMethod::Cunny8x32Nvl.user_selectable());
}

#[test]
fn withdrawn_upscalers_still_deserialize_from_an_existing_state_file() {
    // The enum variants remain so an existing state.json is never a parse error;
    // `normalize_product_choices` is what migrates the value.
    let settings: AppSettings =
        serde_json::from_str(r#"{"wgpu_upscale_method":"Cunny4x32Soft"}"#).unwrap();
    assert_eq!(
        settings.wgpu_upscale_method,
        WgpuUpscaleMethod::Cunny4x32Soft
    );
}
