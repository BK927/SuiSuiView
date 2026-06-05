use eframe::egui;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScreenRenderer {
    Glow,
    Wgpu { target_format: wgpu::TextureFormat },
}

impl ScreenRenderer {
    pub(crate) fn wgpu_target_format(self) -> Option<wgpu::TextureFormat> {
        match self {
            Self::Glow => None,
            Self::Wgpu { target_format } => Some(target_format),
        }
    }

    pub(crate) fn supports_wgsl_paint(self) -> bool {
        self.wgpu_target_format().is_some()
    }
}

pub(crate) struct AppRuntime {
    egui_ctx: egui::Context,
    screen_renderer: ScreenRenderer,
}

impl AppRuntime {
    pub(crate) fn new(egui_ctx: egui::Context, screen_renderer: ScreenRenderer) -> Self {
        Self {
            egui_ctx,
            screen_renderer,
        }
    }

    pub(crate) fn egui_ctx(&self) -> &egui::Context {
        &self.egui_ctx
    }

    pub(crate) fn screen_renderer(&self) -> ScreenRenderer {
        self.screen_renderer
    }
}
