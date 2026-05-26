use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RendererMode {
    Wgpu,
    #[default]
    LowMemoryGlow,
}

impl RendererMode {
    pub const ALL: [Self; 2] = [Self::LowMemoryGlow, Self::Wgpu];

    pub fn label(self) -> &'static str {
        match self {
            Self::Wgpu => "고급 GPU 효과 (WGPU)",
            Self::LowMemoryGlow => "저메모리 기본 (OpenGL)",
        }
    }
}
