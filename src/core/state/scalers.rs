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
pub enum WgpuDownscaler {
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

impl Default for WgpuDownscaler {
    fn default() -> Self {
        Self::PyramidLanczos3
    }
}

impl WgpuDownscaler {
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

    pub fn resolve_for_render(self, output_size: [usize; 2], target_size: [u32; 2]) -> Self {
        let target_is_smaller =
            target_size[0] < output_size[0] as u32 || target_size[1] < output_size[1] as u32;
        if target_is_smaller {
            self
        } else {
            Self::Bilinear
        }
    }
}
