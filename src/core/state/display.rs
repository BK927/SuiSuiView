use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ResizeFilter {
    #[default]
    Bicubic,
    Lanczos3,
    FastTriangle,
    Nearest,
}

impl ResizeFilter {
    pub const ALL: [Self; 4] = [
        Self::Bicubic,
        Self::Lanczos3,
        Self::FastTriangle,
        Self::Nearest,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Bicubic => "Bicubic",
            Self::Lanczos3 => "Lanczos3",
            Self::FastTriangle => "Fast / Triangle",
            Self::Nearest => "Nearest",
        }
    }

    pub fn token(self) -> &'static str {
        match self {
            Self::Bicubic => "bicubic",
            Self::Lanczos3 => "lanczos3",
            Self::FastTriangle => "triangle",
            Self::Nearest => "nearest",
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GpuEffectMode {
    #[default]
    Auto,
    CpuOnly,
    Wgsl,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DisplayUpscaler {
    #[default]
    Auto,
    None,
    WgslBilinear,
    WgslFsr1Style,
    WgslFsr1EasuRcas,
    WgslNisStyle,
    NvidiaNis,
    WgslAnime4kV32CnnX2S,
    WgslAnime4kV32CnnX2M,
    WgslAcnetF8B4Luma,
    WgslAcnetF8B4BoxLuma,
    WgslAcnetF8B4HdnLuma,
    WgslAcnetF8B4BoxHdnLuma,
    CunnyFasterNvl,
    CunnyFastNvl,
}

impl DisplayUpscaler {
    pub const ALL: [Self; 4] = [
        Self::Auto,
        Self::None,
        Self::WgslBilinear,
        Self::WgslFsr1EasuRcas,
    ];

    pub const GPU_METHODS: [Self; 13] = [
        Self::WgslBilinear,
        Self::WgslFsr1Style,
        Self::WgslFsr1EasuRcas,
        Self::WgslNisStyle,
        Self::NvidiaNis,
        Self::WgslAnime4kV32CnnX2S,
        Self::WgslAnime4kV32CnnX2M,
        Self::WgslAcnetF8B4Luma,
        Self::WgslAcnetF8B4BoxLuma,
        Self::WgslAcnetF8B4HdnLuma,
        Self::WgslAcnetF8B4BoxHdnLuma,
        Self::CunnyFasterNvl,
        Self::CunnyFastNvl,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "자동",
            Self::None => "없음",
            Self::WgslBilinear => "WGSL Bilinear",
            Self::WgslFsr1Style => "WGSL FSR-style",
            Self::WgslFsr1EasuRcas => "WGSL FSR1 EASU+RCAS",
            Self::WgslNisStyle => "WGSL NIS-style",
            Self::NvidiaNis => "NVIDIA Image Scaling (NIS)",
            Self::WgslAnime4kV32CnnX2S => "Anime4K v3.2 CNN x2 S",
            Self::WgslAnime4kV32CnnX2M => "Anime4K v3.2 CNN x2 M",
            Self::WgslAcnetF8B4Luma => "ACNet F8B4 Luma",
            Self::WgslAcnetF8B4BoxLuma => "ACNet F8B4 Box Luma",
            Self::WgslAcnetF8B4HdnLuma => "ACNet F8B4 HDN Luma",
            Self::WgslAcnetF8B4BoxHdnLuma => "ACNet F8B4 Box HDN Luma",
            Self::CunnyFasterNvl => "CuNNy faster NVL",
            Self::CunnyFastNvl => "CuNNy fast NVL",
        }
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
            Self::WgslAcnetF8B4Luma => "acnet_f8b4_luma",
            Self::WgslAcnetF8B4BoxLuma => "acnet_f8b4_box_luma",
            Self::WgslAcnetF8B4HdnLuma => "acnet_f8b4_hdn_luma",
            Self::WgslAcnetF8B4BoxHdnLuma => "acnet_f8b4_box_hdn_luma",
            Self::CunnyFasterNvl => "cunny_faster_nvl",
            Self::CunnyFastNvl => "cunny_fast_nvl",
        }
    }

    pub fn is_benchmark_only(self) -> bool {
        matches!(
            self,
            Self::NvidiaNis
                | Self::WgslAnime4kV32CnnX2S
                | Self::WgslAnime4kV32CnnX2M
                | Self::WgslAcnetF8B4Luma
                | Self::WgslAcnetF8B4BoxLuma
                | Self::WgslAcnetF8B4HdnLuma
                | Self::WgslAcnetF8B4BoxHdnLuma
                | Self::CunnyFasterNvl
                | Self::CunnyFastNvl
        )
    }

    pub fn resolve_for_render(
        self,
        output_size: [usize; 2],
        target_size: [u32; 2],
    ) -> Option<Self> {
        let target_is_larger =
            target_size[0] > output_size[0] as u32 || target_size[1] > output_size[1] as u32;
        match self {
            Self::Auto if target_is_larger => Some(Self::WgslFsr1EasuRcas),
            Self::Auto
            | Self::None
            | Self::NvidiaNis
            | Self::WgslAnime4kV32CnnX2S
            | Self::WgslAnime4kV32CnnX2M
            | Self::WgslAcnetF8B4Luma
            | Self::WgslAcnetF8B4BoxLuma
            | Self::WgslAcnetF8B4HdnLuma
            | Self::WgslAcnetF8B4BoxHdnLuma
            | Self::CunnyFasterNvl
            | Self::CunnyFastNvl => None,
            other => Some(other),
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
            | Self::WgslAcnetF8B4Luma
            | Self::WgslAcnetF8B4BoxLuma
            | Self::WgslAcnetF8B4HdnLuma
            | Self::WgslAcnetF8B4BoxHdnLuma
            | Self::CunnyFasterNvl
            | Self::CunnyFastNvl => 0,
        }
    }

    pub fn rcas_shader_method_id(self) -> Option<u32> {
        match self {
            Self::WgslFsr1EasuRcas => Some(5),
            _ => None,
        }
    }
}
