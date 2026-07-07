use super::scalers::WgpuScaleDirection;
use super::{DecoderPreference, WgpuDownscaleMethod, WgpuScalePlan, WgpuUpscaleMethod};
use crate::core::i18n::{I18n, ResolvedLanguage};

const DEFAULT_MIN_SCALE: f32 = 1.10;

#[test]
fn wgpu_scale_plan_activates_only_the_matching_direction() {
    assert_eq!(
        WgpuScalePlan::resolve(
            [800, 1200],
            [1600, 2400],
            WgpuUpscaleMethod::Auto,
            WgpuDownscaleMethod::Hamming,
            DEFAULT_MIN_SCALE
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
            WgpuDownscaleMethod::Hamming,
            DEFAULT_MIN_SCALE
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
            WgpuDownscaleMethod::Hamming,
            DEFAULT_MIN_SCALE
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
            WgpuDownscaleMethod::PyramidLanczos3,
            DEFAULT_MIN_SCALE
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
        DEFAULT_MIN_SCALE,
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
        DEFAULT_MIN_SCALE,
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
        DEFAULT_MIN_SCALE,
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
            WgpuDownscaleMethod::PyramidLanczos3,
            DEFAULT_MIN_SCALE
        )
        .direction,
        WgpuScaleDirection::Upscale
    );
    assert_eq!(
        WgpuScalePlan::resolve(
            [1000, 1000],
            [1090, 950],
            WgpuUpscaleMethod::None,
            WgpuDownscaleMethod::PyramidLanczos3,
            DEFAULT_MIN_SCALE
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
                WgpuDownscaleMethod::PyramidLanczos3,
                DEFAULT_MIN_SCALE
            )
            .effective_upscale_method,
            WgpuUpscaleMethod::None
        );
        assert_eq!(
            WgpuScalePlan::resolve(
                [1600, 2400],
                [1600, 2400],
                method,
                WgpuDownscaleMethod::PyramidLanczos3,
                DEFAULT_MIN_SCALE
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
        DEFAULT_MIN_SCALE,
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
fn fixed_2x_sr_min_scale_threshold_controls_substitution() {
    // Threshold 1.00 ("honest mode"): a 1.01x-needed upscale keeps the selected fixed-2x
    // model rather than substituting FSR.
    assert_eq!(
        WgpuScalePlan::resolve(
            [1000, 1000],
            [1010, 1010],
            WgpuUpscaleMethod::WgslAnime4kV32CnnX2M,
            WgpuDownscaleMethod::Hamming,
            1.00,
        )
        .effective_upscale_method,
        WgpuUpscaleMethod::WgslAnime4kV32CnnX2M
    );

    // Threshold 1.30: a 1.29x-needed upscale is below the boundary and substitutes FSR.
    assert_eq!(
        WgpuScalePlan::resolve(
            [1000, 1000],
            [1290, 1290],
            WgpuUpscaleMethod::WgslAnime4kV32CnnX2M,
            WgpuDownscaleMethod::Hamming,
            1.30,
        )
        .effective_upscale_method,
        WgpuUpscaleMethod::WgslFsr1EasuRcas
    );

    // Threshold 1.30: a 1.31x-needed upscale meets the boundary and keeps the model.
    assert_eq!(
        WgpuScalePlan::resolve(
            [1000, 1000],
            [1310, 1310],
            WgpuUpscaleMethod::WgslAnime4kV32CnnX2M,
            WgpuDownscaleMethod::Hamming,
            1.30,
        )
        .effective_upscale_method,
        WgpuUpscaleMethod::WgslAnime4kV32CnnX2M
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
