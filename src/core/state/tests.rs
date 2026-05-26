use super::{
    AiUpscaleBackend, AiUpscalePrefetchMode, AppSettings, CacheMemoryMode, DecodeMode,
    DisplayUpscaler, EdgePageAction, GpuEffectMode, PersistedState, ResizeFilter, WheelMode,
    WindowPlacement,
};

#[test]
fn old_state_without_settings_loads_defaults() {
    let state: PersistedState = serde_json::from_str(r#"{"version":1,"books":{}}"#).unwrap();

    assert_eq!(state.settings, AppSettings::default());
    assert_eq!(state.window, WindowPlacement::default());
    assert!(state.books.is_empty());
}

#[test]
fn settings_defaults_match_viewer_policy() {
    let settings = AppSettings::default();

    assert!(settings.confirm_delete);
    assert!(settings.esc_to_quit);
    assert!(settings.show_toasts);
    assert!(settings.remember_recent_locations);
    assert!(!settings.single_instance);
    assert_eq!(settings.image_edge_page_action, EdgePageAction::Wrap);
    assert_eq!(settings.archive_edge_page_action, EdgePageAction::Ask);
    assert_eq!(settings.edge_page_action, EdgePageAction::Stop);
    assert_eq!(settings.decode_mode, DecodeMode::AutoFast);
    assert_eq!(settings.resize_filter, ResizeFilter::Bicubic);
    assert_eq!(settings.gpu_effect_mode, GpuEffectMode::Auto);
    assert_eq!(settings.display_upscaler, DisplayUpscaler::Auto);
    assert_eq!(settings.cache_memory_mode, CacheMemoryMode::Auto);
    assert_eq!(settings.manual_cache_mb, 160);
    assert!(settings.apply_exif_orientation);
    assert!(!settings.apply_embedded_icc);
    assert!(settings.auto_save_reading_position);
    assert!(settings.share_state_between_instances);
    assert_eq!(settings.max_remembered_books, 30);
    assert!(settings.remember_archive_page_name);
    assert_eq!(settings.wheel_mode, WheelMode::PageTurn);
    assert_eq!(settings.ai_upscale.backend, AiUpscaleBackend::Off);
    assert_eq!(
        settings.ai_upscale.prefetch_mode,
        AiUpscalePrefetchMode::Off
    );
    assert_eq!(
        settings.ai_upscale.ncnn.model_name,
        "realesrgan-x4plus-anime"
    );
    assert_eq!(settings.ai_upscale.ncnn.scale, 4);
}

#[test]
fn automatic_display_upscaler_only_uses_heavy_shader_for_actual_upscale() {
    assert_eq!(
        DisplayUpscaler::Auto.resolve_for_render([1600, 2400], [800, 1200]),
        None
    );
    assert_eq!(
        DisplayUpscaler::Auto.resolve_for_render([800, 1200], [1600, 2400]),
        Some(DisplayUpscaler::WgslFsr1EasuRcas)
    );
    assert_eq!(
        DisplayUpscaler::Auto.resolve_for_render([1400, 2100], [2000, 3000]),
        Some(DisplayUpscaler::WgslFsr1EasuRcas)
    );
    assert_eq!(
        DisplayUpscaler::WgslNisStyle.resolve_for_render([1600, 2400], [800, 1200]),
        Some(DisplayUpscaler::WgslNisStyle)
    );
}

#[test]
fn product_display_upscalers_hide_style_candidates() {
    assert!(!DisplayUpscaler::ALL.contains(&DisplayUpscaler::WgslFsr1Style));
    assert!(!DisplayUpscaler::ALL.contains(&DisplayUpscaler::WgslNisStyle));
    assert!(!DisplayUpscaler::WgslFsr1Style.candidate().product_visible);
    assert!(!DisplayUpscaler::WgslNisStyle.candidate().product_visible);
    assert!(DisplayUpscaler::ALL.contains(&DisplayUpscaler::WgslFsr1EasuRcas));
    assert!(DisplayUpscaler::ALL.contains(&DisplayUpscaler::CunnyFasterNvl));
    assert!(DisplayUpscaler::ALL.contains(&DisplayUpscaler::CunnyFastNvl));
    assert!(DisplayUpscaler::CunnyFasterNvl.candidate().product_visible);
    assert_eq!(DisplayUpscaler::CunnyFastNvl.candidate().family, "CuNNy");
}

#[test]
fn exact_cunny_variants_render_only_when_upscaling() {
    assert_eq!(
        DisplayUpscaler::CunnyFasterNvl.resolve_for_render([1600, 2400], [800, 1200]),
        None
    );
    assert_eq!(
        DisplayUpscaler::CunnyFasterNvl.resolve_for_render([800, 1200], [1600, 2400]),
        Some(DisplayUpscaler::CunnyFasterNvl)
    );
    assert_eq!(
        DisplayUpscaler::CunnyFastNvl.resolve_for_render([800, 1200], [1600, 2400]),
        Some(DisplayUpscaler::CunnyFastNvl)
    );
}
