use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RendererMode {
    #[default]
    Wgpu,
    LowMemoryGlow,
}

impl RendererMode {
    pub const ALL: [Self; 2] = [Self::Wgpu, Self::LowMemoryGlow];

    pub fn label(self) -> &'static str {
        match self {
            Self::Wgpu => "고성능 GPU (WGPU)",
            Self::LowMemoryGlow => "저메모리 OpenGL (Glow)",
        }
    }
}
