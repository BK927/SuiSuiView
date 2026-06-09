use serde::{Deserialize, Serialize};

use super::display::WgpuUpscaleMethod;

pub const FIXED_2X_SR_SMALL_SCALE_MIN: f32 = 1.10;
pub const FIXED_2X_SR_STACK_SCALE_MIN: f32 = 2.25;
pub const FIXED_2X_SR_MAX_STACK_PASSES: usize = 2;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CpuScaleFilter {
    Nearest,
    Box,
    Bilinear,
    Hamming,
    CatmullRom,
    Mitchell,
    Gaussian,
    Lanczos2,
    Lanczos3,
}

impl Default for CpuScaleFilter {
    fn default() -> Self {
        Self::Hamming
    }
}

impl CpuScaleFilter {
    pub const ALL: [Self; 9] = [
        Self::Nearest,
        Self::Box,
        Self::Bilinear,
        Self::Hamming,
        Self::CatmullRom,
        Self::Mitchell,
        Self::Gaussian,
        Self::Lanczos2,
        Self::Lanczos3,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Nearest => "Nearest",
            Self::Box => "Box / Area",
            Self::Bilinear => "Bilinear",
            Self::Hamming => "Hamming",
            Self::CatmullRom => "CatmullRom",
            Self::Mitchell => "Mitchell",
            Self::Gaussian => "Gaussian",
            Self::Lanczos2 => "Lanczos2",
            Self::Lanczos3 => "Lanczos3",
        }
    }

    pub fn token(self) -> &'static str {
        match self {
            Self::Nearest => "nearest",
            Self::Box => "box",
            Self::Bilinear => "bilinear",
            Self::Hamming => "hamming",
            Self::CatmullRom => "catmullrom",
            Self::Mitchell => "mitchell",
            Self::Gaussian => "gaussian",
            Self::Lanczos2 => "lanczos2",
            Self::Lanczos3 => "lanczos3",
        }
    }
}

impl From<ResizeFilter> for CpuScaleFilter {
    fn from(filter: ResizeFilter) -> Self {
        match filter {
            ResizeFilter::Bicubic => Self::CatmullRom,
            ResizeFilter::Lanczos3 => Self::Lanczos3,
            ResizeFilter::FastTriangle => Self::Bilinear,
            ResizeFilter::Nearest => Self::Nearest,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WgpuDownscaleMethod {
    Nearest,
    Bilinear,
    Box,
    Hamming,
    CatmullRom,
    Mitchell,
    Lanczos2,
    Lanczos3,
    HardwareMipmapLinear,
    PyramidBoxTent,
    PyramidHamming,
    PyramidMitchell,
    PyramidLanczos2,
    PyramidLanczos3,
}

impl Default for WgpuDownscaleMethod {
    fn default() -> Self {
        Self::PyramidLanczos3
    }
}

impl WgpuDownscaleMethod {
    pub const ALL: [Self; 14] = [
        Self::Nearest,
        Self::Bilinear,
        Self::Box,
        Self::Hamming,
        Self::CatmullRom,
        Self::Mitchell,
        Self::Lanczos2,
        Self::Lanczos3,
        Self::HardwareMipmapLinear,
        Self::PyramidBoxTent,
        Self::PyramidHamming,
        Self::PyramidMitchell,
        Self::PyramidLanczos2,
        Self::PyramidLanczos3,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Nearest => "Nearest",
            Self::Bilinear => "Bilinear",
            Self::Box => "Box / Area",
            Self::Hamming => "Hamming",
            Self::CatmullRom => "CatmullRom",
            Self::Mitchell => "Mitchell",
            Self::Lanczos2 => "Lanczos2",
            Self::Lanczos3 => "Lanczos3",
            Self::HardwareMipmapLinear => "Hardware Mipmap Linear",
            Self::PyramidBoxTent => "Pyramid Box/Tent",
            Self::PyramidHamming => "Pyramid + Hamming",
            Self::PyramidMitchell => "Pyramid + Mitchell",
            Self::PyramidLanczos2 => "Pyramid + Lanczos2",
            Self::PyramidLanczos3 => "Pyramid + Lanczos3",
        }
    }

    pub fn token(self) -> &'static str {
        match self {
            Self::Nearest => "nearest",
            Self::Bilinear => "bilinear",
            Self::Box => "box",
            Self::Hamming => "hamming",
            Self::CatmullRom => "catmullrom",
            Self::Mitchell => "mitchell",
            Self::Lanczos2 => "lanczos2",
            Self::Lanczos3 => "lanczos3",
            Self::HardwareMipmapLinear => "hardware_mipmap_linear",
            Self::PyramidBoxTent => "pyramid_box_tent",
            Self::PyramidHamming => "pyramid_hamming",
            Self::PyramidMitchell => "pyramid_mitchell",
            Self::PyramidLanczos2 => "pyramid_lanczos2",
            Self::PyramidLanczos3 => "pyramid_lanczos3",
        }
    }

    pub fn shader_method_id(self) -> u32 {
        match self {
            Self::Nearest => 1,
            Self::Bilinear => 2,
            Self::Box => 3,
            Self::Hamming => 4,
            Self::CatmullRom => 5,
            Self::Mitchell => 6,
            Self::Lanczos2 => 7,
            Self::Lanczos3 => 8,
            Self::HardwareMipmapLinear => Self::Bilinear.shader_method_id(),
            Self::PyramidBoxTent => Self::Bilinear.shader_method_id(),
            Self::PyramidHamming => Self::Hamming.shader_method_id(),
            Self::PyramidMitchell => Self::Mitchell.shader_method_id(),
            Self::PyramidLanczos2 => Self::Lanczos2.shader_method_id(),
            Self::PyramidLanczos3 => Self::Lanczos3.shader_method_id(),
        }
    }

    pub fn base_filter(self) -> Self {
        match self {
            Self::HardwareMipmapLinear => Self::Bilinear,
            Self::PyramidBoxTent => Self::Bilinear,
            Self::PyramidHamming => Self::Hamming,
            Self::PyramidMitchell => Self::Mitchell,
            Self::PyramidLanczos2 => Self::Lanczos2,
            Self::PyramidLanczos3 => Self::Lanczos3,
            filter => filter,
        }
    }

    pub fn pyramid_stage_filter(self) -> Self {
        match self {
            Self::PyramidBoxTent => Self::Box,
            _ => self.base_filter(),
        }
    }

    pub fn is_pyramid(self) -> bool {
        matches!(
            self,
            Self::PyramidBoxTent
                | Self::PyramidHamming
                | Self::PyramidMitchell
                | Self::PyramidLanczos2
                | Self::PyramidLanczos3
        )
    }

    pub fn is_hardware_mipmap(self) -> bool {
        matches!(self, Self::HardwareMipmapLinear)
    }

    pub fn resolve_for_downscale(self, output_size: [usize; 2], target_size: [u32; 2]) -> Self {
        if target_is_smaller(output_size, target_size) {
            self
        } else {
            Self::Bilinear
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WgpuScaleDirection {
    Upscale,
    Downscale,
    Mixed,
    Native,
}

impl WgpuUpscaleMethod {
    pub fn is_fixed_2x_sr(self) -> bool {
        matches!(
            self,
            Self::WgslAnime4kV32CnnX2S
                | Self::WgslAnime4kV32CnnX2M
                | Self::WgslSrLabSpanX2
                | Self::WgslAcnetF8B4Luma
                | Self::WgslAcnetF8B4BoxLuma
                | Self::WgslAcnetF8B4HdnLuma
                | Self::WgslAcnetF8B4BoxHdnLuma
        ) || self.is_artcnn()
            || self.is_cunny()
    }

    pub fn fixed_2x_stack_passes(self, output_size: [usize; 2], target_size: [u32; 2]) -> usize {
        if self.is_fixed_2x_sr()
            && wgpu_target_min_scale(output_size, target_size) >= FIXED_2X_SR_STACK_SCALE_MIN
        {
            FIXED_2X_SR_MAX_STACK_PASSES
        } else {
            1
        }
    }

    fn resolve_for_upscale_target(
        self,
        output_size: [usize; 2],
        target_size: [u32; 2],
    ) -> Option<Self> {
        let method = self.resolve_for_upscale()?;
        if method.is_fixed_2x_sr()
            && wgpu_target_min_scale(output_size, target_size) < FIXED_2X_SR_SMALL_SCALE_MIN
        {
            Some(Self::WgslFsr1EasuRcas)
        } else {
            Some(method)
        }
    }
}

impl WgpuScaleDirection {
    pub fn token(self) -> &'static str {
        match self {
            Self::Upscale => "upscale",
            Self::Downscale => "downscale",
            Self::Mixed => "mixed",
            Self::Native => "native",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WgpuScalePlan {
    pub direction: WgpuScaleDirection,
    pub effective_upscale_method: WgpuUpscaleMethod,
    pub effective_downscale_method: WgpuDownscaleMethod,
}

impl WgpuScalePlan {
    pub fn resolve(
        output_size: [usize; 2],
        target_size: [u32; 2],
        requested_upscale: WgpuUpscaleMethod,
        requested_downscale: WgpuDownscaleMethod,
    ) -> Self {
        if target_is_larger(output_size, target_size) {
            return Self {
                direction: WgpuScaleDirection::Upscale,
                effective_upscale_method: requested_upscale
                    .resolve_for_upscale_target(output_size, target_size)
                    .unwrap_or(WgpuUpscaleMethod::None),
                effective_downscale_method: WgpuDownscaleMethod::Bilinear,
            };
        }
        if target_is_smaller(output_size, target_size) {
            return Self {
                direction: WgpuScaleDirection::Downscale,
                effective_upscale_method: WgpuUpscaleMethod::None,
                effective_downscale_method: requested_downscale
                    .resolve_for_downscale(output_size, target_size),
            };
        }
        if target_is_mixed(output_size, target_size) {
            return Self {
                direction: WgpuScaleDirection::Mixed,
                effective_upscale_method: WgpuUpscaleMethod::None,
                effective_downscale_method: WgpuDownscaleMethod::Bilinear,
            };
        }
        Self {
            direction: WgpuScaleDirection::Native,
            effective_upscale_method: WgpuUpscaleMethod::None,
            effective_downscale_method: WgpuDownscaleMethod::Bilinear,
        }
    }
}

fn target_is_larger(output_size: [usize; 2], target_size: [u32; 2]) -> bool {
    let output_width = output_size[0] as u32;
    let output_height = output_size[1] as u32;
    (target_size[0] > output_width || target_size[1] > output_height)
        && target_size[0] >= output_width
        && target_size[1] >= output_height
}

fn wgpu_target_min_scale(output_size: [usize; 2], target_size: [u32; 2]) -> f32 {
    let output_width = output_size[0].max(1) as f32;
    let output_height = output_size[1].max(1) as f32;
    let target_width = target_size[0].max(1) as f32;
    let target_height = target_size[1].max(1) as f32;
    (target_width / output_width).min(target_height / output_height)
}

fn target_is_smaller(output_size: [usize; 2], target_size: [u32; 2]) -> bool {
    let output_width = output_size[0] as u32;
    let output_height = output_size[1] as u32;
    (target_size[0] < output_width || target_size[1] < output_height)
        && target_size[0] <= output_width
        && target_size[1] <= output_height
}

fn target_is_mixed(output_size: [usize; 2], target_size: [u32; 2]) -> bool {
    let output_width = output_size[0] as u32;
    let output_height = output_size[1] as u32;
    (target_size[0] > output_width && target_size[1] < output_height)
        || (target_size[0] < output_width && target_size[1] > output_height)
}
