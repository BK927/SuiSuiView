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
fn hidden_display_upscaler_settings_normalize_to_auto() {
    let mut settings = AppSettings {
        display_upscaler: DisplayUpscaler::WgslFsr1Style,
        ..AppSettings::default()
    };

    settings.normalize_product_choices();

    assert_eq!(settings.display_upscaler, DisplayUpscaler::Auto);
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
    assert!(DisplayUpscaler::ALL.contains(&DisplayUpscaler::WgslAnime4kV32CnnX2S));
    assert!(DisplayUpscaler::ALL.contains(&DisplayUpscaler::WgslAnime4kV32CnnX2M));
    assert!(DisplayUpscaler::ALL.contains(&DisplayUpscaler::WgslAcnetF8B4Luma));
    assert!(DisplayUpscaler::ALL.contains(&DisplayUpscaler::WgslAcnetF8B4BoxLuma));
    assert!(DisplayUpscaler::ALL.contains(&DisplayUpscaler::WgslAcnetF8B4HdnLuma));
    assert!(DisplayUpscaler::ALL.contains(&DisplayUpscaler::WgslAcnetF8B4BoxHdnLuma));
    assert!(DisplayUpscaler::ALL.contains(&DisplayUpscaler::CunnyVeryfastNvl));
    assert!(DisplayUpscaler::ALL.contains(&DisplayUpscaler::CunnyVeryfastSoft));
    assert!(DisplayUpscaler::ALL.contains(&DisplayUpscaler::CunnyFasterNvl));
    assert!(DisplayUpscaler::ALL.contains(&DisplayUpscaler::CunnyFasterSoft));
    assert!(DisplayUpscaler::ALL.contains(&DisplayUpscaler::CunnyFasterDs));
    assert!(DisplayUpscaler::ALL.contains(&DisplayUpscaler::CunnyFastNvl));
    assert!(DisplayUpscaler::ALL.contains(&DisplayUpscaler::CunnyFastSoft));
    assert!(DisplayUpscaler::ALL.contains(&DisplayUpscaler::CunnyFastDs));
    assert!(DisplayUpscaler::ALL.contains(&DisplayUpscaler::Cunny2x12Soft));
    assert!(DisplayUpscaler::ALL.contains(&DisplayUpscaler::Cunny2x12Ds));
    assert!(DisplayUpscaler::ALL.contains(&DisplayUpscaler::Cunny3x12Nvl));
    assert!(DisplayUpscaler::ALL.contains(&DisplayUpscaler::Cunny3x12Soft));
    assert!(DisplayUpscaler::ALL.contains(&DisplayUpscaler::Cunny3x12Ds));
    assert!(DisplayUpscaler::ALL.contains(&DisplayUpscaler::Cunny4x12Nvl));
    assert!(DisplayUpscaler::ALL.contains(&DisplayUpscaler::Cunny4x12Soft));
    assert!(DisplayUpscaler::ALL.contains(&DisplayUpscaler::Cunny4x12Ds));
    assert!(DisplayUpscaler::ALL.contains(&DisplayUpscaler::Cunny4x16Nvl));
    assert!(DisplayUpscaler::ALL.contains(&DisplayUpscaler::Cunny4x24Nvl));
    assert!(DisplayUpscaler::ALL.contains(&DisplayUpscaler::Cunny4x24Soft));
    assert!(DisplayUpscaler::ALL.contains(&DisplayUpscaler::Cunny4x24Ds));
    assert!(DisplayUpscaler::ALL.contains(&DisplayUpscaler::Cunny4x32Nvl));
    assert!(DisplayUpscaler::ALL.contains(&DisplayUpscaler::Cunny8x32Nvl));
    assert!(
        DisplayUpscaler::WgslAnime4kV32CnnX2S
            .candidate()
            .product_visible
    );
    assert!(
        DisplayUpscaler::WgslAnime4kV32CnnX2M
            .candidate()
            .product_visible
    );
    assert!(
        DisplayUpscaler::WgslAcnetF8B4BoxHdnLuma
            .candidate()
            .product_visible
    );
    assert!(
        DisplayUpscaler::CunnyVeryfastNvl
            .candidate()
            .product_visible
    );
    assert!(
        DisplayUpscaler::CunnyVeryfastSoft
            .candidate()
            .product_visible
    );
    assert!(DisplayUpscaler::CunnyFasterNvl.candidate().product_visible);
    assert!(DisplayUpscaler::CunnyFasterSoft.candidate().product_visible);
    assert!(DisplayUpscaler::CunnyFasterDs.candidate().product_visible);
    assert_eq!(DisplayUpscaler::CunnyFastNvl.candidate().family, "CuNNy");
    assert_eq!(DisplayUpscaler::CunnyFastSoft.candidate().family, "CuNNy");
    assert_eq!(DisplayUpscaler::CunnyFastDs.candidate().family, "CuNNy");
    assert_eq!(DisplayUpscaler::Cunny2x12Soft.candidate().family, "CuNNy");
    assert_eq!(DisplayUpscaler::Cunny2x12Ds.candidate().family, "CuNNy");
    assert_eq!(DisplayUpscaler::Cunny3x12Soft.candidate().family, "CuNNy");
    assert_eq!(DisplayUpscaler::Cunny3x12Ds.candidate().family, "CuNNy");
    assert_eq!(DisplayUpscaler::Cunny4x12Soft.candidate().family, "CuNNy");
    assert_eq!(DisplayUpscaler::Cunny4x12Ds.candidate().family, "CuNNy");
    assert_eq!(DisplayUpscaler::Cunny4x24Soft.candidate().family, "CuNNy");
    assert_eq!(DisplayUpscaler::Cunny4x24Ds.candidate().family, "CuNNy");
    assert!(DisplayUpscaler::Cunny4x24Soft.candidate().product_visible);
    assert!(DisplayUpscaler::Cunny4x24Ds.candidate().product_visible);
    assert!(DisplayUpscaler::Cunny4x24Soft.product_selectable());
    assert!(DisplayUpscaler::Cunny4x24Ds.product_selectable());
    assert!(!DisplayUpscaler::Cunny4x24Soft.is_benchmark_only());
    assert!(!DisplayUpscaler::Cunny4x24Ds.is_benchmark_only());
    assert_eq!(
        DisplayUpscaler::CunnyVeryfastSoft.label(),
        "CuNNy veryfast SOFT"
    );
    assert_eq!(
        DisplayUpscaler::CunnyFasterSoft.label(),
        "CuNNy faster SOFT"
    );
    assert_eq!(DisplayUpscaler::CunnyFasterDs.label(), "CuNNy faster DS");
    assert_eq!(DisplayUpscaler::Cunny2x12Soft.label(), "CuNNy 2x12 SOFT");
    assert_eq!(DisplayUpscaler::Cunny2x12Ds.label(), "CuNNy 2x12 DS");
    assert_eq!(DisplayUpscaler::Cunny3x12Soft.label(), "CuNNy 3x12 SOFT");
    assert_eq!(DisplayUpscaler::Cunny3x12Ds.label(), "CuNNy 3x12 DS");
    assert_eq!(DisplayUpscaler::Cunny3x12Nvl.label(), "CuNNy 3x12 NVL");
    assert_eq!(DisplayUpscaler::Cunny4x12Soft.label(), "CuNNy 4x12 SOFT");
    assert_eq!(DisplayUpscaler::Cunny4x12Ds.label(), "CuNNy 4x12 DS");
    assert_eq!(DisplayUpscaler::Cunny4x12Nvl.label(), "CuNNy 4x12 NVL");
    assert_eq!(DisplayUpscaler::Cunny4x16Nvl.label(), "CuNNy 4x16 NVL");
    assert_eq!(DisplayUpscaler::Cunny4x24Nvl.label(), "CuNNy 4x24 NVL");
    assert_eq!(DisplayUpscaler::Cunny4x24Soft.label(), "CuNNy 4x24 SOFT");
    assert_eq!(DisplayUpscaler::Cunny4x24Ds.label(), "CuNNy 4x24 DS");
    assert_eq!(DisplayUpscaler::Cunny4x32Nvl.label(), "CuNNy 4x32 NVL");
    assert_eq!(DisplayUpscaler::Cunny8x32Nvl.label(), "CuNNy 8x32 NVL");
}

#[test]
fn exact_cunny_variants_render_only_when_upscaling() {
    assert_eq!(
        DisplayUpscaler::CunnyVeryfastNvl.resolve_for_render([1600, 2400], [800, 1200]),
        None
    );
    assert_eq!(
        DisplayUpscaler::CunnyVeryfastNvl.resolve_for_render([800, 1200], [1600, 2400]),
        Some(DisplayUpscaler::CunnyVeryfastNvl)
    );
    assert_eq!(
        DisplayUpscaler::CunnyVeryfastSoft.resolve_for_render([1600, 2400], [800, 1200]),
        None
    );
    assert_eq!(
        DisplayUpscaler::CunnyVeryfastSoft.resolve_for_render([800, 1200], [1600, 2400]),
        Some(DisplayUpscaler::CunnyVeryfastSoft)
    );
    assert_eq!(
        DisplayUpscaler::CunnyFasterNvl.resolve_for_render([1600, 2400], [800, 1200]),
        None
    );
    assert_eq!(
        DisplayUpscaler::CunnyFasterNvl.resolve_for_render([800, 1200], [1600, 2400]),
        Some(DisplayUpscaler::CunnyFasterNvl)
    );
    assert_eq!(
        DisplayUpscaler::CunnyFasterSoft.resolve_for_render([1600, 2400], [800, 1200]),
        None
    );
    assert_eq!(
        DisplayUpscaler::CunnyFasterSoft.resolve_for_render([800, 1200], [1600, 2400]),
        Some(DisplayUpscaler::CunnyFasterSoft)
    );
    assert_eq!(
        DisplayUpscaler::CunnyFasterDs.resolve_for_render([800, 1200], [1600, 2400]),
        Some(DisplayUpscaler::CunnyFasterDs)
    );
    assert_eq!(
        DisplayUpscaler::CunnyFastNvl.resolve_for_render([800, 1200], [1600, 2400]),
        Some(DisplayUpscaler::CunnyFastNvl)
    );
    assert_eq!(
        DisplayUpscaler::CunnyFastSoft.resolve_for_render([800, 1200], [1600, 2400]),
        Some(DisplayUpscaler::CunnyFastSoft)
    );
    assert_eq!(
        DisplayUpscaler::CunnyFastDs.resolve_for_render([800, 1200], [1600, 2400]),
        Some(DisplayUpscaler::CunnyFastDs)
    );
    assert_eq!(
        DisplayUpscaler::Cunny2x12Soft.resolve_for_render([800, 1200], [1600, 2400]),
        Some(DisplayUpscaler::Cunny2x12Soft)
    );
    assert_eq!(
        DisplayUpscaler::Cunny2x12Ds.resolve_for_render([800, 1200], [1600, 2400]),
        Some(DisplayUpscaler::Cunny2x12Ds)
    );
    assert_eq!(
        DisplayUpscaler::Cunny3x12Nvl.resolve_for_render([800, 1200], [1600, 2400]),
        Some(DisplayUpscaler::Cunny3x12Nvl)
    );
    assert_eq!(
        DisplayUpscaler::Cunny3x12Soft.resolve_for_render([800, 1200], [1600, 2400]),
        Some(DisplayUpscaler::Cunny3x12Soft)
    );
    assert_eq!(
        DisplayUpscaler::Cunny3x12Ds.resolve_for_render([800, 1200], [1600, 2400]),
        Some(DisplayUpscaler::Cunny3x12Ds)
    );
    assert_eq!(
        DisplayUpscaler::Cunny4x12Nvl.resolve_for_render([800, 1200], [1600, 2400]),
        Some(DisplayUpscaler::Cunny4x12Nvl)
    );
    assert_eq!(
        DisplayUpscaler::Cunny4x12Soft.resolve_for_render([800, 1200], [1600, 2400]),
        Some(DisplayUpscaler::Cunny4x12Soft)
    );
    assert_eq!(
        DisplayUpscaler::Cunny4x12Ds.resolve_for_render([800, 1200], [1600, 2400]),
        Some(DisplayUpscaler::Cunny4x12Ds)
    );
    assert_eq!(
        DisplayUpscaler::Cunny4x16Nvl.resolve_for_render([800, 1200], [1600, 2400]),
        Some(DisplayUpscaler::Cunny4x16Nvl)
    );
    assert_eq!(
        DisplayUpscaler::Cunny4x24Nvl.resolve_for_render([800, 1200], [1600, 2400]),
        Some(DisplayUpscaler::Cunny4x24Nvl)
    );
    assert_eq!(
        DisplayUpscaler::Cunny4x24Soft.resolve_for_render([800, 1200], [1600, 2400]),
        Some(DisplayUpscaler::Cunny4x24Soft)
    );
    assert_eq!(
        DisplayUpscaler::Cunny4x24Ds.resolve_for_render([800, 1200], [1600, 2400]),
        Some(DisplayUpscaler::Cunny4x24Ds)
    );
    assert_eq!(
        DisplayUpscaler::Cunny4x32Nvl.resolve_for_render([800, 1200], [1600, 2400]),
        Some(DisplayUpscaler::Cunny4x32Nvl)
    );
    assert_eq!(
        DisplayUpscaler::Cunny8x32Nvl.resolve_for_render([800, 1200], [1600, 2400]),
        Some(DisplayUpscaler::Cunny8x32Nvl)
    );
    assert_eq!(
        DisplayUpscaler::WgslAnime4kV32CnnX2S.resolve_for_render([800, 1200], [1600, 2400]),
        Some(DisplayUpscaler::WgslAnime4kV32CnnX2S)
    );
    assert_eq!(
        DisplayUpscaler::WgslAnime4kV32CnnX2M.resolve_for_render([800, 1200], [1600, 2400]),
        Some(DisplayUpscaler::WgslAnime4kV32CnnX2M)
    );
    assert_eq!(
        DisplayUpscaler::WgslAcnetF8B4BoxHdnLuma.resolve_for_render([800, 1200], [1600, 2400]),
        Some(DisplayUpscaler::WgslAcnetF8B4BoxHdnLuma)
    );
}
