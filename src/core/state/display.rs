use crate::core::i18n::I18n;
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GpuEffectMode {
    #[default]
    Auto,
    CpuOnly,
    Wgsl,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WgpuUpscaleMethod {
    Auto,
    #[default]
    None,
    WgslBilinear,
    WgslFsr1Style,
    WgslFsr1EasuRcas,
    WgslNisStyle,
    NvidiaNis,
    WgslAnime4kV32CnnX2S,
    WgslAnime4kV32CnnX2M,
    WgslArtcnnC4F16,
    WgslArtcnnC4F16Dn,
    WgslArtcnnC4F16Ds,
    WgslArtcnnC4F32,
    WgslArtcnnC4F32Dn,
    WgslArtcnnC4F32Ds,
    WgslSrLabSpanX2,
    WgslAcnetF8B4Luma,
    WgslAcnetF8B4BoxLuma,
    WgslAcnetF8B4HdnLuma,
    WgslAcnetF8B4BoxHdnLuma,
    CunnyVeryfastNvl,
    CunnyVeryfastSoft,
    CunnyFasterNvl,
    CunnyFasterSoft,
    CunnyFasterDs,
    CunnyFastNvl,
    CunnyFastSoft,
    CunnyFastDs,
    Cunny2x12Soft,
    Cunny2x12Ds,
    Cunny3x12Nvl,
    Cunny3x12Soft,
    Cunny3x12Ds,
    Cunny4x12Nvl,
    Cunny4x12Soft,
    Cunny4x12Ds,
    Cunny4x16Nvl,
    Cunny4x16Soft,
    Cunny4x16Ds,
    Cunny4x24Nvl,
    Cunny4x24Soft,
    Cunny4x24Ds,
    Cunny4x32Nvl,
    Cunny4x32Soft,
    Cunny4x32Ds,
    Cunny8x32Nvl,
    Cunny8x32Ds,
}

/// Idle-scheduled "refine" (정련) upscaler: a heavy CuNNy tier rendered once per
/// page while the viewer is idle, replacing the displayed page with the higher
/// quality result at zero per-frame cost. `Off` by default; these ports are
/// experimental quality (see the settings help text).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RefineUpscaler {
    #[default]
    Off,
    Cunny4x32Soft,
    Cunny4x32Ds,
    Cunny8x32Nvl,
    Cunny8x32Ds,
}

impl RefineUpscaler {
    /// Offerable tiers. `Cunny4x32Soft`, `Cunny4x32Ds` and `Cunny8x32Ds` are
    /// deliberately absent: measured against a Lanczos3 reference they score
    /// *below* plain bilinear on both line art and illustration, at two scales —
    /// picking one made the page worse than doing nothing. The variants stay in
    /// the enum so a persisted setting still deserializes (and so the CLI bench
    /// can re-check them), but nothing offers them until the port is fixed.
    /// See `selectable()` for the migration of an already-saved choice.
    pub const ALL: [Self; 2] = [Self::Off, Self::Cunny8x32Nvl];

    /// Whether this tier may be offered or kept. A saved setting naming a
    /// withdrawn tier falls back to `Off` rather than silently degrading pages.
    pub fn selectable(self) -> bool {
        Self::ALL.contains(&self)
    }

    /// The concrete WGSL upscaler this refine tier maps to, or `None` when off.
    pub fn method(self) -> Option<WgpuUpscaleMethod> {
        match self {
            Self::Off => None,
            Self::Cunny4x32Soft => Some(WgpuUpscaleMethod::Cunny4x32Soft),
            Self::Cunny4x32Ds => Some(WgpuUpscaleMethod::Cunny4x32Ds),
            Self::Cunny8x32Nvl => Some(WgpuUpscaleMethod::Cunny8x32Nvl),
            Self::Cunny8x32Ds => Some(WgpuUpscaleMethod::Cunny8x32Ds),
        }
    }

    /// Combo label: `Off` is localized; the concrete tiers keep their technical
    /// English names (e.g. `CuNNy 4x32 SOFT`).
    pub fn label_i18n(self, i18n: I18n) -> String {
        match self.method() {
            None => i18n.text("state.off"),
            Some(method) => method.label().to_owned(),
        }
    }
}

mod upscaler_choices;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UpscalerCandidate {
    pub family: &'static str,
    pub exact_label: &'static str,
    pub source_version: &'static str,
    pub license: &'static str,
    pub scale: &'static str,
    pub passes: &'static str,
    pub feature_requirements: &'static str,
    pub product_visible: bool,
}

macro_rules! upscaler_candidate {
    (
        $family:expr,
        $exact_label:expr,
        $source_version:expr,
        $license:expr,
        $scale:expr,
        $passes:expr,
        $feature_requirements:expr,
        $product_visible:expr $(,)?
    ) => {
        UpscalerCandidate {
            family: $family,
            exact_label: $exact_label,
            source_version: $source_version,
            license: $license,
            scale: $scale,
            passes: $passes,
            feature_requirements: $feature_requirements,
            product_visible: $product_visible,
        }
    };
}

impl WgpuUpscaleMethod {
    pub fn label(self) -> &'static str {
        self.candidate().exact_label
    }

    pub fn label_i18n(self, i18n: I18n) -> String {
        match self {
            Self::Auto => i18n.text("state.auto"),
            Self::None => i18n.text("state.none"),
            _ => self.label().to_owned(),
        }
    }

    pub fn settings_label_i18n(self, i18n: I18n) -> String {
        match self {
            Self::WgslNisStyle
            | Self::WgslArtcnnC4F16
            | Self::WgslArtcnnC4F16Dn
            | Self::WgslArtcnnC4F16Ds
            | Self::WgslArtcnnC4F32
            | Self::WgslArtcnnC4F32Dn
            | Self::WgslArtcnnC4F32Ds => i18n.experimental_label(self.label()),
            Self::WgslSrLabSpanX2 => i18n.slow_label(self.label()),
            _ => self.label_i18n(i18n),
        }
    }

    pub fn candidate(self) -> UpscalerCandidate {
        match self {
            Self::Auto => upscaler_candidate!(
                "Control",
                "Auto",
                "first-party",
                "license-neutral",
                "auto",
                "auto",
                "wgpu optional",
                true,
            ),
            Self::None => upscaler_candidate!(
                "Control",
                "None",
                "first-party",
                "license-neutral",
                "1x",
                "0",
                "none",
                true,
            ),
            Self::WgslBilinear => upscaler_candidate!(
                "SuiSuiView",
                "WGSL Bilinear",
                "first-party",
                "license-neutral",
                "arbitrary",
                "1",
                "wgpu",
                true,
            ),
            Self::WgslFsr1Style => upscaler_candidate!(
                "SuiSuiView",
                "WGSL FSR-style",
                "first-party style candidate",
                "license-neutral",
                "arbitrary",
                "1",
                "wgpu",
                true,
            ),
            Self::WgslFsr1EasuRcas => upscaler_candidate!(
                "AMD FidelityFX FSR 1",
                "WGSL FSR1 EASU+RCAS",
                "FidelityFX-FSR 1",
                "MIT",
                "arbitrary",
                "2",
                "wgpu",
                true,
            ),
            Self::WgslNisStyle => upscaler_candidate!(
                "SuiSuiView",
                "WGSL NIS-style",
                "first-party style candidate",
                "license-neutral",
                "arbitrary",
                "1",
                "wgpu",
                false,
            ),
            Self::NvidiaNis => upscaler_candidate!(
                "NVIDIA Image Scaling",
                "NVIDIA Image Scaling (NIS)",
                "NVIDIAImageScaling SDK",
                "MIT",
                "arbitrary",
                "1",
                "wgpu compute",
                false,
            ),
            Self::WgslAnime4kV32CnnX2S => upscaler_candidate!(
                "Anime4K",
                "Anime4K v3.2 CNN x2 S",
                "Anime4K v3.2",
                "MIT",
                "2x",
                "multi-pass",
                "wgpu compute",
                true,
            ),
            Self::WgslAnime4kV32CnnX2M => upscaler_candidate!(
                "Anime4K",
                "Anime4K v3.2 CNN x2 M",
                "Anime4K v3.2",
                "MIT",
                "2x",
                "multi-pass",
                "wgpu compute",
                true,
            ),
            method @ (Self::WgslArtcnnC4F16
            | Self::WgslArtcnnC4F16Dn
            | Self::WgslArtcnnC4F16Ds
            | Self::WgslArtcnnC4F32
            | Self::WgslArtcnnC4F32Dn
            | Self::WgslArtcnnC4F32Ds) => upscaler_candidate!(
                "ArtCNN",
                match method {
                    Self::WgslArtcnnC4F16 => "ArtCNN C4F16",
                    Self::WgslArtcnnC4F16Dn => "ArtCNN C4F16 DN",
                    Self::WgslArtcnnC4F16Ds => "ArtCNN C4F16 DS",
                    Self::WgslArtcnnC4F32 => "ArtCNN C4F32",
                    Self::WgslArtcnnC4F32Dn => "ArtCNN C4F32 DN",
                    Self::WgslArtcnnC4F32Ds => "ArtCNN C4F32 DS",
                    _ => unreachable!(),
                },
                "ArtCNN 0263e9c",
                "MIT",
                "2x",
                "8",
                "wgpu compute",
                false,
            ),
            Self::WgslSrLabSpanX2 => upscaler_candidate!(
                "SR Lab",
                "SR Lab SPAN x2",
                "SPAN converted manifest",
                "Apache-2.0 model family / local weights",
                "2x",
                "multi-pass",
                "wgpu compute + local manifest",
                true,
            ),
            Self::WgslAcnetF8B4Luma => upscaler_candidate!(
                "ACNetGLSL",
                "ACNet F8B4 Luma",
                "ACNetGLSL f8b4",
                "MIT",
                "2x",
                "multi-pass",
                "wgpu compute",
                true,
            ),
            Self::WgslAcnetF8B4BoxLuma => upscaler_candidate!(
                "ACNetGLSL",
                "ACNet F8B4 Box Luma",
                "ACNetGLSL f8b4 box",
                "MIT",
                "2x",
                "multi-pass",
                "wgpu compute",
                true,
            ),
            Self::WgslAcnetF8B4HdnLuma => upscaler_candidate!(
                "ACNetGLSL",
                "ACNet F8B4 HDN Luma",
                "ACNetGLSL f8b4 hdn",
                "MIT",
                "2x",
                "multi-pass",
                "wgpu compute",
                true,
            ),
            Self::WgslAcnetF8B4BoxHdnLuma => upscaler_candidate!(
                "ACNetGLSL",
                "ACNet F8B4 Box HDN Luma",
                "ACNetGLSL f8b4 box hdn",
                "MIT",
                "2x",
                "multi-pass",
                "wgpu compute",
                true,
            ),
            Self::CunnyVeryfastNvl => upscaler_candidate!(
                "CuNNy",
                "CuNNy veryfast NVL",
                "funnyplanter/CuNNy magpie normal",
                "LGPL-3.0-or-later / GPL-3.0-or-later effect header",
                "2x",
                "4",
                "wgpu compute",
                true,
            ),
            Self::CunnyVeryfastSoft => upscaler_candidate!(
                "CuNNy",
                "CuNNy veryfast SOFT",
                "funnyplanter/CuNNy mpv soft",
                "LGPL-3.0-or-later",
                "2x",
                "4",
                "wgpu compute",
                true,
            ),
            Self::CunnyFasterNvl => upscaler_candidate!(
                "CuNNy",
                "CuNNy faster NVL",
                "funnyplanter/CuNNy magpie normal",
                "LGPL-3.0-or-later / GPL-3.0-or-later effect header",
                "2x",
                "4",
                "wgpu compute",
                true,
            ),
            Self::CunnyFasterSoft => upscaler_candidate!(
                "CuNNy",
                "CuNNy faster SOFT",
                "funnyplanter/CuNNy mpv soft",
                "LGPL-3.0-or-later",
                "2x",
                "4",
                "wgpu compute",
                true,
            ),
            Self::CunnyFasterDs => upscaler_candidate!(
                "CuNNy",
                "CuNNy faster DS",
                "funnyplanter/CuNNy mpv ds",
                "LGPL-3.0-or-later",
                "2x",
                "4",
                "wgpu compute",
                true,
            ),
            Self::CunnyFastNvl => upscaler_candidate!(
                "CuNNy",
                "CuNNy fast NVL",
                "funnyplanter/CuNNy magpie normal",
                "LGPL-3.0-or-later / GPL-3.0-or-later effect header",
                "2x",
                "4",
                "wgpu compute",
                true,
            ),
            Self::CunnyFastSoft => upscaler_candidate!(
                "CuNNy",
                "CuNNy fast SOFT",
                "funnyplanter/CuNNy mpv soft",
                "LGPL-3.0-or-later",
                "2x",
                "4",
                "wgpu compute",
                true,
            ),
            Self::CunnyFastDs => upscaler_candidate!(
                "CuNNy",
                "CuNNy fast DS",
                "funnyplanter/CuNNy mpv ds",
                "LGPL-3.0-or-later",
                "2x",
                "4",
                "wgpu compute",
                true,
            ),
            Self::Cunny2x12Soft => upscaler_candidate!(
                "CuNNy",
                "CuNNy 2x12 SOFT",
                "funnyplanter/CuNNy mpv soft",
                "LGPL-3.0-or-later",
                "2x",
                "4",
                "wgpu compute",
                true,
            ),
            Self::Cunny2x12Ds => upscaler_candidate!(
                "CuNNy",
                "CuNNy 2x12 DS",
                "funnyplanter/CuNNy mpv ds",
                "LGPL-3.0-or-later",
                "2x",
                "4",
                "wgpu compute",
                true,
            ),
            Self::Cunny3x12Nvl => upscaler_candidate!(
                "CuNNy",
                "CuNNy 3x12 NVL",
                "funnyplanter/CuNNy magpie normal",
                "LGPL-3.0-or-later / GPL-3.0-or-later effect header",
                "2x",
                "5",
                "wgpu compute",
                true,
            ),
            Self::Cunny3x12Soft => upscaler_candidate!(
                "CuNNy",
                "CuNNy 3x12 SOFT",
                "funnyplanter/CuNNy mpv soft",
                "LGPL-3.0-or-later",
                "2x",
                "5",
                "wgpu compute",
                true,
            ),
            Self::Cunny3x12Ds => upscaler_candidate!(
                "CuNNy",
                "CuNNy 3x12 DS",
                "funnyplanter/CuNNy mpv ds",
                "LGPL-3.0-or-later",
                "2x",
                "5",
                "wgpu compute",
                true,
            ),
            Self::Cunny4x12Nvl => upscaler_candidate!(
                "CuNNy",
                "CuNNy 4x12 NVL",
                "funnyplanter/CuNNy magpie normal",
                "LGPL-3.0-or-later / GPL-3.0-or-later effect header",
                "2x",
                "6",
                "wgpu compute",
                true,
            ),
            Self::Cunny4x12Soft => upscaler_candidate!(
                "CuNNy",
                "CuNNy 4x12 SOFT",
                "funnyplanter/CuNNy mpv soft",
                "LGPL-3.0-or-later",
                "2x",
                "6",
                "wgpu compute",
                true,
            ),
            Self::Cunny4x12Ds => upscaler_candidate!(
                "CuNNy",
                "CuNNy 4x12 DS",
                "funnyplanter/CuNNy mpv ds",
                "LGPL-3.0-or-later",
                "2x",
                "6",
                "wgpu compute",
                true,
            ),
            Self::Cunny4x16Nvl => upscaler_candidate!(
                "CuNNy",
                "CuNNy 4x16 NVL",
                "funnyplanter/CuNNy magpie normal",
                "LGPL-3.0-or-later / GPL-3.0-or-later effect header",
                "2x",
                "11",
                "wgpu compute",
                true,
            ),
            Self::Cunny4x16Soft => upscaler_candidate!(
                "CuNNy",
                "CuNNy 4x16 SOFT",
                "funnyplanter/CuNNy mpv soft",
                "LGPL-3.0-or-later",
                "2x",
                "11",
                "wgpu compute",
                true,
            ),
            Self::Cunny4x16Ds => upscaler_candidate!(
                "CuNNy",
                "CuNNy 4x16 DS",
                "funnyplanter/CuNNy mpv ds",
                "LGPL-3.0-or-later",
                "2x",
                "11",
                "wgpu compute",
                true,
            ),
            Self::Cunny4x24Nvl => upscaler_candidate!(
                "CuNNy",
                "CuNNy 4x24 NVL",
                "funnyplanter/CuNNy magpie normal",
                "LGPL-3.0-or-later / GPL-3.0-or-later effect header",
                "2x",
                "11",
                "wgpu compute",
                true,
            ),
            Self::Cunny4x24Soft => upscaler_candidate!(
                "CuNNy",
                "CuNNy 4x24 SOFT",
                "funnyplanter/CuNNy mpv soft",
                "LGPL-3.0-or-later",
                "2x",
                "11",
                "wgpu compute",
                true,
            ),
            Self::Cunny4x24Ds => upscaler_candidate!(
                "CuNNy",
                "CuNNy 4x24 DS",
                "funnyplanter/CuNNy mpv ds",
                "LGPL-3.0-or-later",
                "2x",
                "11",
                "wgpu compute",
                true,
            ),
            Self::Cunny4x32Nvl => upscaler_candidate!(
                "CuNNy",
                "CuNNy 4x32 NVL",
                "funnyplanter/CuNNy magpie normal",
                "LGPL-3.0-or-later / GPL-3.0-or-later effect header",
                "2x",
                "16",
                "wgpu compute",
                true,
            ),
            Self::Cunny4x32Soft => upscaler_candidate!(
                "CuNNy",
                "CuNNy 4x32 SOFT",
                "funnyplanter/CuNNy mpv soft",
                "LGPL-3.0-or-later",
                "2x",
                "16",
                "wgpu compute",
                true,
            ),
            Self::Cunny4x32Ds => upscaler_candidate!(
                "CuNNy",
                "CuNNy 4x32 DS",
                "funnyplanter/CuNNy mpv ds",
                "LGPL-3.0-or-later",
                "2x",
                "16",
                "wgpu compute",
                true,
            ),
            Self::Cunny8x32Nvl => upscaler_candidate!(
                "CuNNy",
                "CuNNy 8x32 NVL",
                "funnyplanter/CuNNy magpie normal",
                "LGPL-3.0-or-later / GPL-3.0-or-later effect header",
                "2x",
                "28",
                "wgpu compute",
                true,
            ),
            Self::Cunny8x32Ds => upscaler_candidate!(
                "CuNNy",
                "CuNNy 8x32 DS",
                "funnyplanter/CuNNy mpv ds",
                "LGPL-3.0-or-later",
                "2x",
                "28",
                "wgpu compute",
                true,
            ),
        }
    }

    pub fn product_selectable(self) -> bool {
        Self::ALL.contains(&self) && self.candidate().product_visible
    }

    pub fn experimental_selectable(self) -> bool {
        Self::EXPERIMENTAL.contains(&self)
    }

    pub fn user_selectable(self) -> bool {
        self.product_selectable() || self.experimental_selectable()
    }

    pub fn token(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::None => "none",
            Self::WgslBilinear => "wgsl_bilinear",
            Self::WgslFsr1Style => "wgsl_fsr1_style",
            Self::WgslFsr1EasuRcas => "wgsl_fsr1_easu_rcas",
            Self::WgslNisStyle => "wgsl_nis_style",
            Self::NvidiaNis => "nvidia_nis",
            Self::WgslAnime4kV32CnnX2S => "anime4k_v32_cnn_x2_s",
            Self::WgslAnime4kV32CnnX2M => "anime4k_v32_cnn_x2_m",
            Self::WgslArtcnnC4F16 => "artcnn_c4f16",
            Self::WgslArtcnnC4F16Dn => "artcnn_c4f16_dn",
            Self::WgslArtcnnC4F16Ds => "artcnn_c4f16_ds",
            Self::WgslArtcnnC4F32 => "artcnn_c4f32",
            Self::WgslArtcnnC4F32Dn => "artcnn_c4f32_dn",
            Self::WgslArtcnnC4F32Ds => "artcnn_c4f32_ds",
            Self::WgslSrLabSpanX2 => "srlab_span_x2",
            Self::WgslAcnetF8B4Luma => "acnet_f8b4_luma",
            Self::WgslAcnetF8B4BoxLuma => "acnet_f8b4_box_luma",
            Self::WgslAcnetF8B4HdnLuma => "acnet_f8b4_hdn_luma",
            Self::WgslAcnetF8B4BoxHdnLuma => "acnet_f8b4_box_hdn_luma",
            Self::CunnyVeryfastNvl => "cunny_veryfast_nvl",
            Self::CunnyVeryfastSoft => "cunny_veryfast_soft",
            Self::CunnyFasterNvl => "cunny_faster_nvl",
            Self::CunnyFasterSoft => "cunny_faster_soft",
            Self::CunnyFasterDs => "cunny_faster_ds",
            Self::CunnyFastNvl => "cunny_fast_nvl",
            Self::CunnyFastSoft => "cunny_fast_soft",
            Self::CunnyFastDs => "cunny_fast_ds",
            Self::Cunny2x12Soft => "cunny_2x12_soft",
            Self::Cunny2x12Ds => "cunny_2x12_ds",
            Self::Cunny3x12Nvl => "cunny_3x12_nvl",
            Self::Cunny3x12Soft => "cunny_3x12_soft",
            Self::Cunny3x12Ds => "cunny_3x12_ds",
            Self::Cunny4x12Nvl => "cunny_4x12_nvl",
            Self::Cunny4x12Soft => "cunny_4x12_soft",
            Self::Cunny4x12Ds => "cunny_4x12_ds",
            Self::Cunny4x16Nvl => "cunny_4x16_nvl",
            Self::Cunny4x16Soft => "cunny_4x16_soft",
            Self::Cunny4x16Ds => "cunny_4x16_ds",
            Self::Cunny4x24Nvl => "cunny_4x24_nvl",
            Self::Cunny4x24Soft => "cunny_4x24_soft",
            Self::Cunny4x24Ds => "cunny_4x24_ds",
            Self::Cunny4x32Nvl => "cunny_4x32_nvl",
            Self::Cunny4x32Soft => "cunny_4x32_soft",
            Self::Cunny4x32Ds => "cunny_4x32_ds",
            Self::Cunny8x32Nvl => "cunny_8x32_nvl",
            Self::Cunny8x32Ds => "cunny_8x32_ds",
        }
    }

    pub fn is_benchmark_only(self) -> bool {
        matches!(self, Self::NvidiaNis) || self.is_artcnn()
    }

    pub fn is_artcnn(self) -> bool {
        matches!(
            self,
            Self::WgslArtcnnC4F16
                | Self::WgslArtcnnC4F16Dn
                | Self::WgslArtcnnC4F16Ds
                | Self::WgslArtcnnC4F32
                | Self::WgslArtcnnC4F32Dn
                | Self::WgslArtcnnC4F32Ds
        )
    }

    pub fn is_cunny(self) -> bool {
        matches!(
            self,
            Self::CunnyVeryfastNvl
                | Self::CunnyVeryfastSoft
                | Self::CunnyFasterNvl
                | Self::CunnyFasterSoft
                | Self::CunnyFasterDs
                | Self::CunnyFastNvl
                | Self::CunnyFastSoft
                | Self::CunnyFastDs
                | Self::Cunny2x12Soft
                | Self::Cunny2x12Ds
                | Self::Cunny3x12Nvl
                | Self::Cunny3x12Soft
                | Self::Cunny3x12Ds
                | Self::Cunny4x12Nvl
                | Self::Cunny4x12Soft
                | Self::Cunny4x12Ds
                | Self::Cunny4x16Nvl
                | Self::Cunny4x16Soft
                | Self::Cunny4x16Ds
                | Self::Cunny4x24Nvl
                | Self::Cunny4x24Soft
                | Self::Cunny4x24Ds
                | Self::Cunny4x32Nvl
                | Self::Cunny4x32Soft
                | Self::Cunny4x32Ds
                | Self::Cunny8x32Nvl
                | Self::Cunny8x32Ds
        )
    }

    pub fn resolve_for_upscale(self) -> Option<Self> {
        match self {
            Self::Auto => Some(Self::WgslFsr1EasuRcas),
            Self::None | Self::NvidiaNis => None,
            method => Some(method),
        }
    }

    pub fn shader_method_id(self) -> u32 {
        match self {
            Self::Auto | Self::None => 0,
            Self::WgslBilinear => 1,
            Self::WgslFsr1Style => 2,
            Self::WgslNisStyle => 3,
            Self::WgslFsr1EasuRcas => 4,
            Self::NvidiaNis
            | Self::WgslAnime4kV32CnnX2S
            | Self::WgslAnime4kV32CnnX2M
            | Self::WgslArtcnnC4F16
            | Self::WgslArtcnnC4F16Dn
            | Self::WgslArtcnnC4F16Ds
            | Self::WgslArtcnnC4F32
            | Self::WgslArtcnnC4F32Dn
            | Self::WgslArtcnnC4F32Ds
            | Self::WgslSrLabSpanX2
            | Self::WgslAcnetF8B4Luma
            | Self::WgslAcnetF8B4BoxLuma
            | Self::WgslAcnetF8B4HdnLuma
            | Self::WgslAcnetF8B4BoxHdnLuma
            | Self::CunnyVeryfastNvl
            | Self::CunnyVeryfastSoft
            | Self::CunnyFasterNvl
            | Self::CunnyFasterSoft
            | Self::CunnyFasterDs
            | Self::CunnyFastNvl
            | Self::CunnyFastSoft
            | Self::CunnyFastDs
            | Self::Cunny2x12Soft
            | Self::Cunny2x12Ds
            | Self::Cunny3x12Nvl
            | Self::Cunny3x12Soft
            | Self::Cunny3x12Ds
            | Self::Cunny4x12Nvl
            | Self::Cunny4x12Soft
            | Self::Cunny4x12Ds
            | Self::Cunny4x16Nvl
            | Self::Cunny4x16Soft
            | Self::Cunny4x16Ds
            | Self::Cunny4x24Nvl
            | Self::Cunny4x24Soft
            | Self::Cunny4x24Ds
            | Self::Cunny4x32Nvl
            | Self::Cunny4x32Soft
            | Self::Cunny4x32Ds
            | Self::Cunny8x32Nvl
            | Self::Cunny8x32Ds => 0,
        }
    }

    pub fn rcas_shader_method_id(self) -> Option<u32> {
        match self {
            Self::WgslFsr1EasuRcas => Some(5),
            _ => None,
        }
    }
}
