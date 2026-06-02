use crate::core::i18n::I18n;
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

    pub fn label_i18n(self, i18n: I18n) -> String {
        match self {
            Self::Wgpu => i18n.text("label.renderer.wgpu"),
            Self::LowMemoryGlow => i18n.text("label.renderer.glow"),
        }
    }
}
